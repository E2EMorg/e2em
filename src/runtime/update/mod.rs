//! Idle background updater for the standalone daemon. Embedded hosts never update.
mod activity;
mod release;
pub mod store;
pub mod supervisor;
pub use activity::{Activity, Work};
use serde::{Deserialize, Serialize};
use std::{
    io,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
pub use store::Store;

pub const MAX_BINARY: u64 = 128 * 1024 * 1024;
pub const UPDATE_EXIT: i32 = 75;
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize, clap::ValueEnum)]
#[serde(rename_all = "snake_case")]
pub enum Channel {
    #[default]
    Stable,
    Preview,
}
#[derive(Clone)]
pub struct Config {
    pub directory: PathBuf,
    pub channel: Channel,
    pub interval: Duration,
    pub idle: Duration,
    pub ready_file: Option<PathBuf>,
    pub parent: Option<u32>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Candidate {
    pub version: String,
    pub sha256: String,
    pub size: u64,
}
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct State {
    pub active: Option<Candidate>,
    pub previous: Option<Candidate>,
    pub pending: Option<Candidate>,
    pub trial: bool,
    pub rejected: Option<String>,
}
#[derive(Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Status {
    pub stage: String,
    pub running_version: String,
    pub channel: Channel,
    pub last_check: u64,
    pub next_check: u64,
    pub failures: u32,
    pub error: Option<String>,
    pub available_version: Option<String>,
}
pub fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

type CheckResult = io::Result<Option<Candidate>>;
pub struct Updater {
    config: Config,
    store: Store,
    pub activity: Arc<Activity>,
    status: Status,
    receiver: Option<mpsc::Receiver<CheckResult>>,
    stop: Arc<AtomicBool>,
    ready: Option<Candidate>,
}
impl Updater {
    pub fn parent_alive(&self) -> bool {
        let Some(parent) = self.config.parent else {
            return true;
        };
        #[cfg(unix)]
        {
            rustix::process::getppid().is_some_and(|pid| pid.as_raw_pid() as u32 == parent)
        }
        #[cfg(windows)]
        {
            e2em_platform::process_alive(parent).unwrap_or(false)
        }
        #[cfg(not(any(unix, windows)))]
        {
            let _ = parent;
            true
        }
    }
    pub fn new(config: Config, activity: Arc<Activity>) -> io::Result<Self> {
        let store = Store::open(&config.directory)?;
        let mut status = match store.read::<Status>("status.json") {
            Ok(status) => status,
            Err(e) if e.kind() == io::ErrorKind::NotFound => Status::default(),
            Err(e) => return Err(e),
        };
        if status.channel != config.channel {
            status.next_check = 0;
        }
        // Clamp persisted wall-clock deadlines after clock adjustments.
        status.next_check = status
            .next_check
            .min(now().saturating_add(config.interval.as_secs()));
        status.stage = "waiting_for_idle".into();
        status.running_version = env!("CARGO_PKG_VERSION").into();
        status.channel = config.channel;
        let ready = store
            .state()?
            .pending
            .filter(|c| c.version != env!("CARGO_PKG_VERSION"));
        Ok(Self {
            config,
            store,
            activity,
            status,
            receiver: None,
            stop: Arc::new(AtomicBool::new(false)),
            ready,
        })
    }
    pub fn mark_ready(&self) -> io::Result<()> {
        self.save_status()?;
        if let Some(path) = &self.config.ready_file {
            std::fs::write(path, b"ready\n")?;
        }
        Ok(())
    }
    /// Poll from the service loop. All network and bulk file IO stays on a worker.
    /// Returns true only after maintenance admission is closed and state is durable.
    pub fn poll(&mut self) -> bool {
        match self.step() {
            Ok(update) => update,
            Err(error) => {
                eprintln!("background updater: {error}");
                self.status.error = Some(error.to_string());
                self.status.next_check = now() + 300;
                false
            }
        }
    }
    fn save_status(&self) -> io::Result<()> {
        self.store.write("status.json", &self.status)
    }
    fn step(&mut self) -> io::Result<bool> {
        // The supervisor owns pending/active transitions during probation.
        if self.store.state()?.trial {
            return Ok(false);
        }
        match self.store.read::<u64>("check-request.json") {
            Ok(_) => {
                std::fs::remove_file(self.store.root.join("check-request.json"))?;
                self.status.next_check = 0;
                self.save_status()?;
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => return Err(e),
        }
        if let Some(receiver) = &self.receiver {
            let result = match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err(io::Error::other("update worker stopped")))
                }
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                self.receiver = None;
                match result {
                    Ok(candidate) => {
                        self.status.failures = 0;
                        self.status.error = None;
                        self.status.available_version =
                            candidate.as_ref().map(|c| c.version.clone());
                        self.ready = candidate;
                        if let Some(candidate) = &self.ready {
                            let mut state = self.store.state()?;
                            state.pending = Some(candidate.clone());
                            self.store.write("state.json", &state)?;
                        }
                        self.status.stage = if self.ready.is_some() {
                            "staged"
                        } else {
                            "up_to_date"
                        }
                        .into();
                        self.status.next_check =
                            now().saturating_add(self.config.interval.as_secs());
                    }
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => {
                        self.status.stage = "waiting_for_idle".into();
                        self.status.next_check = 0;
                    }
                    Err(e) => {
                        self.status.failures = self.status.failures.saturating_add(1);
                        self.status.error = Some(e.to_string());
                        self.status.stage = "retrying".into();
                        self.status.next_check =
                            now() + retry_delay(self.status.failures, self.config.interval);
                    }
                }
                self.save_status()?;
            }
        }
        if let Some(candidate) = &self.ready {
            if !self.activity.idle_for(self.config.idle) {
                return Ok(false);
            }
            let mut state = self.store.state()?;
            state.pending = Some(candidate.clone());
            self.store.write("state.json", &state)?;
            self.status.stage = "restarting".into();
            self.save_status()?;
            // IO can fail before this point without closing foreground admission.
            return Ok(self.activity.begin_maintenance(self.config.idle));
        }
        if self.receiver.is_none()
            && now() >= self.status.next_check
            && self.activity.idle_for(self.config.idle)
        {
            self.status.stage = "checking_and_downloading".into();
            self.status.last_check = now();
            self.save_status()?;
            let (tx, rx) = mpsc::channel();
            self.receiver = Some(rx);
            let config = self.config.clone();
            let store = self.store.clone();
            let activity = self.activity.clone();
            let stop = self.stop.clone();
            tokio::task::spawn_blocking(move || {
                let _ = tx.send(release::check_and_stage(&config, &store, &activity, &stop));
            });
        }
        Ok(false)
    }
}
impl Drop for Updater {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
    }
}
fn retry_delay(failures: u32, interval: Duration) -> u64 {
    300u64
        .saturating_mul(1u64 << failures.saturating_sub(1).min(10))
        .min(interval.as_secs().max(300))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backoff_is_bounded() {
        let interval = Duration::from_secs(21_600);
        assert_eq!(retry_delay(1, interval), 300);
        assert_eq!(retry_delay(2, interval), 600);
        assert_eq!(retry_delay(u32::MAX, interval), 21_600);
    }
    #[cfg(unix)]
    fn updater() -> (tempfile::TempDir, Updater) {
        let root = tempfile::tempdir().unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(root.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
        let config = Config {
            directory: root.path().join("updates"),
            channel: Channel::Preview,
            interval: Duration::from_secs(21_600),
            idle: Duration::ZERO,
            ready_file: None,
            parent: None,
        };
        let updater = Updater::new(config, Arc::default()).unwrap();
        (root, updater)
    }
    #[test]
    #[cfg(unix)]
    fn staged_update_waits_for_reply_and_closes_admission_before_restart() {
        let (_root, mut updater) = updater();
        let reply = updater.activity.enter().unwrap();
        let (tx, rx) = mpsc::channel();
        updater.receiver = Some(rx);
        let candidate = Candidate {
            version: "0.2.0".into(),
            sha256: "a".repeat(64),
            size: 6,
        };
        tx.send(Ok(Some(candidate.clone()))).unwrap();
        assert!(!updater.poll());
        assert_eq!(updater.store.state().unwrap().pending, Some(candidate));
        drop(reply);
        assert!(updater.poll());
        assert!(updater.activity.enter().is_none());
    }
    #[test]
    #[cfg(unix)]
    fn failed_or_disconnected_workers_schedule_retry_without_stopping_service() {
        let (_root, mut updater) = updater();
        for disconnected in [false, true] {
            let (tx, rx) = mpsc::channel();
            updater.receiver = Some(rx);
            if !disconnected {
                tx.send(Err(io::Error::other("offline"))).unwrap();
            }
            drop(tx);
            assert!(!updater.poll());
            assert!(updater.status.next_check > now());
            assert_eq!(updater.status.stage, "retrying");
            assert!(updater.activity.enter().is_some());
        }
    }
}
