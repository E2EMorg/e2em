//! Explicit disposable native worker for scorers whose allocators retain memory.
//! The executable and arguments are trusted provisioning, never assessment input.
use super::{API_VERSION, MAX_FRAME};
use crate::{BackendError, PolicyScorer};
use serde::{Deserialize, Serialize};
use std::{
    ffi::OsString,
    io::{self, Read, Write},
    path::Path,
    process::{Child, Command, Stdio},
    sync::{Mutex, mpsc},
    thread,
    time::Duration,
};

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Ready {
    kind: String,
    api_version: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScoreRequest {
    message: String,
    context: Option<String>,
    policy: String,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ScoreReply {
    Score { probability: f64 },
    Unavailable,
}
fn error(message: &'static str) -> BackendError {
    BackendError::new(message)
}
fn read_frame<T: for<'de> Deserialize<'de>>(reader: &mut impl Read) -> io::Result<T> {
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    let size = u32::from_be_bytes(header) as usize;
    if size == 0 || size > MAX_FRAME {
        return Err(io::Error::other("invalid native worker frame"));
    }
    let mut bytes = vec![0; size];
    reader.read_exact(&mut bytes)?;
    serde_json::from_slice(&bytes).map_err(|_| io::Error::other("invalid native worker response"))
}
struct BoundedJson(Vec<u8>);
impl Write for BoundedJson {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > MAX_FRAME.saturating_sub(self.0.len()) {
            return Err(io::Error::other("oversized native worker value"));
        }
        self.0.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn encode_frame(value: &impl Serialize) -> io::Result<Vec<u8>> {
    let mut output = BoundedJson(Vec::new());
    serde_json::to_writer(&mut output, value)
        .map_err(|_| io::Error::other("invalid or oversized native worker value"))?;
    let bytes = output.0;
    if bytes.is_empty() {
        return Err(io::Error::other("empty native worker value"));
    }
    let mut frame = (bytes.len() as u32).to_be_bytes().to_vec();
    frame.extend(bytes);
    Ok(frame)
}
struct Work {
    frame: Vec<u8>,
    reply: mpsc::SyncSender<Result<f64, BackendError>>,
}
struct Process {
    child: Option<Child>,
    sender: Option<mpsc::SyncSender<Work>>,
    reader: Option<thread::JoinHandle<()>>,
    timeout: Duration,
}
impl Process {
    fn stop(&mut self) {
        self.sender.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        if let Some(reader) = self.reader.take() {
            let _ = reader.join();
        }
    }
}
impl Drop for Process {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Each loaded instance owns one child and kills/reaps it on eviction or failure.
/// Workers must use [`serve_worker`]'s bounded protocol and must not spawn child
/// processes that inherit its pipes. No inference-time asset provisioning occurs.
pub struct NativeProcessScorer {
    process: Mutex<Process>,
}
impl NativeProcessScorer {
    pub fn spawn(
        executable: &Path,
        arguments: &[OsString],
        timeout: Duration,
    ) -> Result<Self, BackendError> {
        if !executable.is_absolute()
            || !executable.is_file()
            || timeout.is_zero()
            || timeout > Duration::from_secs(5)
        {
            return Err(error(
                "native worker requires an absolute provisioned executable and a positive timeout of at most five seconds",
            ));
        }
        let child = Command::new(executable)
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| error("native worker could not start"))?;
        let mut process = Process {
            child: Some(child),
            sender: None,
            reader: None,
            timeout,
        };
        let child = process.child.as_mut().expect("newly owned child");
        let mut stdin = child
            .stdin
            .take()
            .ok_or_else(|| error("native worker input unavailable"))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| error("native worker output unavailable"))?;
        let (sender, requests) = mpsc::sync_channel::<Work>(1);
        let (started, startup) = mpsc::sync_channel(1);
        process.sender = Some(sender);
        process.reader = Some(
            thread::Builder::new()
                .name("e2em-native-worker".into())
                .spawn(move || {
                    let ready = read_frame::<Ready>(&mut stdout)
                        .map_err(|_| error("native worker startup response invalid"))
                        .and_then(|ready| {
                            if ready.kind == "ready" && ready.api_version == API_VERSION {
                                Ok(())
                            } else {
                                Err(error("native worker version incompatible"))
                            }
                        });
                    let valid = ready.is_ok();
                    if started.send(ready).is_err() || !valid {
                        return;
                    }
                    while let Ok(work) = requests.recv() {
                        let result = stdin
                            .write_all(&work.frame)
                            .and_then(|_| stdin.flush())
                            .and_then(|_| read_frame::<ScoreReply>(&mut stdout))
                            .map_err(|_| error("native worker transport failed"))
                            .and_then(|reply| match reply {
                                ScoreReply::Score { probability }
                                    if probability.is_finite()
                                        && (0.0..=1.0).contains(&probability) =>
                                {
                                    Ok(probability)
                                }
                                _ => Err(error("native worker score unavailable")),
                            });
                        let failed = result.is_err();
                        let _ = work.reply.send(result);
                        if failed {
                            break;
                        }
                    }
                })
                .map_err(|_| error("native worker reader could not start"))?,
        );
        startup
            .recv_timeout(timeout)
            .map_err(|_| error("native worker startup timed out"))??;
        Ok(Self {
            process: Mutex::new(process),
        })
    }
    pub fn process_id(&self) -> Option<u32> {
        self.process
            .lock()
            .ok()
            .and_then(|process| process.child.as_ref().map(Child::id))
    }
}
impl PolicyScorer for NativeProcessScorer {
    fn score(
        &self,
        message: &str,
        context: Option<&str>,
        policy: &str,
    ) -> Result<f64, BackendError> {
        // Bound owned input copies; the serializer separately caps escaped JSON.
        if message
            .len()
            .saturating_add(context.map_or(0, str::len))
            .saturating_add(policy.len())
            > MAX_FRAME
        {
            return Err(error("native worker request too large"));
        }
        let frame = encode_frame(&ScoreRequest {
            message: message.into(),
            context: context.map(str::to_owned),
            policy: policy.into(),
        })
        .map_err(|_| error("native worker request invalid"))?;
        let mut process = self
            .process
            .lock()
            .map_err(|_| error("native worker lock failed"))?;
        let (reply, receive) = mpsc::sync_channel(1);
        let Some(sender) = &process.sender else {
            return Err(error("native worker unavailable"));
        };
        if sender.send(Work { frame, reply }).is_err() {
            process.stop();
            return Err(error("native worker unavailable"));
        }
        match receive.recv_timeout(process.timeout) {
            Ok(Ok(score)) => Ok(score),
            Ok(Err(error)) => {
                process.stop();
                Err(error)
            }
            Err(_) => {
                process.stop();
                Err(error("native worker response timed out"))
            }
        }
    }
}

/// Run a provisioned native scorer on private stdin/stdout. Results and errors do
/// not echo text; stderr is suppressed by the parent. No remote transport exists.
pub fn serve_worker(scorer: &impl PolicyScorer) -> io::Result<()> {
    let mut input = io::stdin().lock();
    let mut output = io::stdout().lock();
    output.write_all(&encode_frame(&Ready {
        kind: "ready".into(),
        api_version: API_VERSION.into(),
    })?)?;
    output.flush()?;
    loop {
        let request: ScoreRequest = match read_frame(&mut input) {
            Ok(request) => request,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        };
        let reply = match scorer.score(
            &request.message,
            request.context.as_deref(),
            &request.policy,
        ) {
            Ok(probability) if probability.is_finite() && (0.0..=1.0).contains(&probability) => {
                ScoreReply::Score { probability }
            }
            _ => ScoreReply::Unavailable,
        };
        output.write_all(&encode_frame(&reply)?)?;
        output.flush()?;
    }
}
