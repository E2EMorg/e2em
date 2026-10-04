//! Explicit disposable native worker for scorers whose allocators retain memory.
//! The executable and arguments are trusted provisioning, never assessment input.
use super::{API_VERSION, MAX_FRAME, model::Metadata};
use crate::{BackendError, PolicyScorer, RequestControl};
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    metadata: Option<Metadata>,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ScoreRequest {
    message: String,
    context: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    policy: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    policies: Option<Vec<String>>,
}
#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
enum ScoreReply {
    Score {
        probability: f64,
    },
    Scores {
        probabilities: Vec<f64>,
        target_complete: bool,
        context_complete: bool,
    },
    Unavailable,
}
struct WorkerScores {
    values: Vec<f64>,
    coverage: (bool, bool),
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
    reply: mpsc::SyncSender<Result<WorkerScores, BackendError>>,
}
struct Process {
    child: Option<Child>,
    sender: Option<mpsc::SyncSender<Work>>,
    reader: Option<thread::JoinHandle<()>>,
    timeout: Duration,
    control: Option<RequestControl>,
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
    metadata: Metadata,
    coverage: Mutex<(bool, bool)>,
}
impl NativeProcessScorer {
    pub fn spawn(
        executable: &Path,
        arguments: &[OsString],
        timeout: Duration,
    ) -> Result<Self, BackendError> {
        if timeout > Duration::from_secs(5) {
            return Err(error("legacy worker timeout exceeds five seconds"));
        }
        Self::spawn_model(executable, arguments, timeout)
    }
    pub fn spawn_model(
        executable: &Path,
        arguments: &[OsString],
        timeout: Duration,
    ) -> Result<Self, BackendError> {
        Self::spawn_controlled(executable, arguments, timeout, None)
    }
    pub fn spawn_controlled(
        executable: &Path,
        arguments: &[OsString],
        timeout: Duration,
        control: Option<RequestControl>,
    ) -> Result<Self, BackendError> {
        Self::spawn_inner(executable, arguments, timeout, control, None)
    }
    /// Use the trusted installation's library directory for native provider dependencies.
    pub fn spawn_with_libraries(
        executable: &Path,
        arguments: &[OsString],
        timeout: Duration,
        control: Option<RequestControl>,
        library_directory: &Path,
    ) -> Result<Self, BackendError> {
        if !library_directory.is_absolute() || !library_directory.is_dir() {
            return Err(error(
                "native library directory must be absolute and provisioned",
            ));
        }
        Self::spawn_inner(
            executable,
            arguments,
            timeout,
            control,
            Some(library_directory),
        )
    }
    fn spawn_inner(
        executable: &Path,
        arguments: &[OsString],
        timeout: Duration,
        control: Option<RequestControl>,
        _library_directory: Option<&Path>,
    ) -> Result<Self, BackendError> {
        if !executable.is_absolute()
            || !executable.is_file()
            || timeout.is_zero()
            || timeout > Duration::from_secs(30)
        {
            return Err(error(
                "native worker requires an absolute provisioned executable and a positive timeout of at most thirty seconds",
            ));
        }
        let mut command = Command::new(executable);
        #[cfg(target_os = "linux")]
        if let Some(directory) = _library_directory {
            let mut paths = vec![directory.to_path_buf()];
            if let Some(existing) = std::env::var_os("LD_LIBRARY_PATH") {
                paths.extend(std::env::split_paths(&existing));
            }
            command.env(
                "LD_LIBRARY_PATH",
                std::env::join_paths(paths)
                    .map_err(|_| error("invalid native library search path"))?,
            );
        }
        let child = command
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
            control: None,
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
                                Ok(ready.metadata.unwrap_or_default())
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
                                    Ok(WorkerScores {
                                        values: vec![probability],
                                        coverage: (true, true),
                                    })
                                }
                                ScoreReply::Scores {
                                    probabilities,
                                    target_complete,
                                    context_complete,
                                } if !probabilities.is_empty()
                                    && probabilities.len() <= 64
                                    && probabilities
                                        .iter()
                                        .all(|p| p.is_finite() && (0.0..=1.0).contains(p)) =>
                                {
                                    Ok(WorkerScores {
                                        values: probabilities,
                                        coverage: (target_complete, context_complete),
                                    })
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
        let deadline = std::time::Instant::now() + timeout;
        let metadata = loop {
            if std::time::Instant::now() >= deadline
                || control.as_ref().is_some_and(RequestControl::stopped)
            {
                return Err(error("native worker startup cancelled or timed out"));
            }
            match startup.recv_timeout(Duration::from_millis(25)) {
                Ok(result) => break result?,
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(error("native worker startup failed"));
                }
            }
        };
        Ok(Self {
            process: Mutex::new(process),
            metadata,
            coverage: Mutex::new((true, true)),
        })
    }
    pub fn process_id(&self) -> Option<u32> {
        self.process
            .lock()
            .ok()
            .and_then(|process| process.child.as_ref().map(Child::id))
    }
    pub fn metadata(&self) -> &Metadata {
        &self.metadata
    }
    fn exchange(&self, request: ScoreRequest) -> Result<Vec<f64>, BackendError> {
        let frame = encode_frame(&request).map_err(|_| error("native worker request invalid"))?;
        let mut process = self
            .process
            .lock()
            .map_err(|_| error("native worker lock failed"))?;
        let (reply, receive) = mpsc::sync_channel(1);
        let Some(sender) = &process.sender else {
            return Err(error("native worker unavailable"));
        };
        if process
            .control
            .as_ref()
            .is_some_and(RequestControl::stopped)
        {
            process.stop();
            return Err(error("native worker request expired"));
        }
        if sender.send(Work { frame, reply }).is_err() {
            process.stop();
            return Err(error("native worker unavailable"));
        }
        let deadline = process.control.as_ref().map_or_else(
            || std::time::Instant::now() + process.timeout,
            |c| c.deadline,
        );
        loop {
            if process
                .control
                .as_ref()
                .is_some_and(RequestControl::stopped)
                || std::time::Instant::now() >= deadline
            {
                process.stop();
                return Err(error("native worker response cancelled or timed out"));
            }
            match receive.recv_timeout(
                Duration::from_millis(25)
                    .min(deadline.saturating_duration_since(std::time::Instant::now())),
            ) {
                Ok(Ok(scores)) => {
                    let mut coverage = self
                        .coverage
                        .lock()
                        .map_err(|_| error("worker coverage lock failed"))?;
                    coverage.0 &= scores.coverage.0;
                    coverage.1 &= scores.coverage.1;
                    return Ok(scores.values);
                }
                Ok(Err(error)) => {
                    process.stop();
                    return Err(error);
                }
                Err(mpsc::RecvTimeoutError::Timeout) => continue,
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    process.stop();
                    return Err(error("native worker response unavailable"));
                }
            }
        }
    }
}
impl PolicyScorer for NativeProcessScorer {
    fn begin_request(
        &self,
        model: Option<&str>,
        control: RequestControl,
        _needs_model: bool,
    ) -> Result<(), BackendError> {
        if model.is_some() {
            return Err(error("worker does not select model packages"));
        }
        self.process
            .lock()
            .map_err(|_| error("worker lock failed"))?
            .control = Some(control);
        *self
            .coverage
            .lock()
            .map_err(|_| error("worker coverage lock failed"))? = (true, true);
        Ok(())
    }
    fn supports_model_categories(&self) -> bool {
        self.metadata.model_categories
    }
    fn supports_custom_policies(&self) -> bool {
        self.metadata.custom_policies
    }
    fn model_version(&self) -> String {
        if self.metadata.model.is_empty() {
            "none".into()
        } else {
            self.metadata.model.clone()
        }
    }
    fn tokenizer_version(&self) -> String {
        if self.metadata.tokenizer.is_empty() {
            "none".into()
        } else {
            self.metadata.tokenizer.clone()
        }
    }
    fn max_tokens(&self) -> Option<usize> {
        self.metadata.max_tokens
    }
    fn action_threshold(&self, policy: &str) -> Option<f64> {
        self.metadata
            .thresholds
            .iter()
            .find(|t| t.wording == policy)
            .map(|t| t.action)
    }
    fn review_threshold(&self, policy: &str) -> Option<f64> {
        self.metadata
            .thresholds
            .iter()
            .find(|t| t.wording == policy)
            .map(|t| t.review)
    }
    fn coverage(&self) -> (bool, bool) {
        self.coverage.lock().map_or((false, false), |c| *c)
    }
    fn score(
        &self,
        message: &str,
        context: Option<&str>,
        policy: &str,
    ) -> Result<f64, BackendError> {
        if message
            .len()
            .saturating_add(context.map_or(0, str::len))
            .saturating_add(policy.len())
            > MAX_FRAME
        {
            return Err(error("native worker request too large"));
        }
        let values = self.exchange(ScoreRequest {
            message: message.into(),
            context: context.map(str::to_owned),
            policy: Some(policy.into()),
            policies: None,
        })?;
        if values.len() != 1 {
            return Err(error("worker returned an invalid score count"));
        }
        Ok(values[0])
    }
    fn score_many(
        &self,
        message: &str,
        context: Option<&str>,
        policies: &[String],
    ) -> Result<Vec<f64>, BackendError> {
        if !self.metadata.model_categories {
            return policies
                .iter()
                .map(|p| self.score(message, context, p))
                .collect();
        }
        if policies.is_empty() || policies.len() > 64 {
            return Err(error("invalid worker policy count"));
        }
        self.exchange(ScoreRequest {
            message: message.into(),
            context: context.map(str::to_owned),
            policy: None,
            policies: Some(policies.to_vec()),
        })
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
        metadata: Some(Metadata {
            model: scorer.model_version(),
            tokenizer: scorer.tokenizer_version(),
            max_tokens: scorer.max_tokens(),
            model_categories: scorer.supports_model_categories(),
            custom_policies: scorer.supports_custom_policies(),
            thresholds: vec![],
        }),
    })?)?;
    output.flush()?;
    loop {
        let request: ScoreRequest = match read_frame(&mut input) {
            Ok(request) => request,
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => return Ok(()),
            Err(error) => return Err(error),
        };
        let reply = if let (None, Some(policies)) = (&request.policy, &request.policies) {
            if policies.is_empty() || policies.len() > 64 {
                ScoreReply::Unavailable
            } else {
                match scorer.score_many(&request.message, request.context.as_deref(), policies) {
                    Ok(probabilities)
                        if probabilities.len() == policies.len()
                            && probabilities
                                .iter()
                                .all(|p| p.is_finite() && (0.0..=1.0).contains(p)) =>
                    {
                        let (target_complete, context_complete) = scorer.coverage();
                        ScoreReply::Scores {
                            probabilities,
                            target_complete,
                            context_complete,
                        }
                    }
                    _ => ScoreReply::Unavailable,
                }
            }
        } else if let (Some(policy), None) = (&request.policy, &request.policies) {
            match scorer.score(&request.message, request.context.as_deref(), policy) {
                Ok(probability)
                    if probability.is_finite() && (0.0..=1.0).contains(&probability) =>
                {
                    ScoreReply::Score { probability }
                }
                _ => ScoreReply::Unavailable,
            }
        } else {
            ScoreReply::Unavailable
        };
        output.write_all(&encode_frame(&reply)?)?;
        output.flush()?;
    }
}
