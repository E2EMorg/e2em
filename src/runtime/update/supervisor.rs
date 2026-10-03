//! Stable launcher, readiness probation and crash recovery across platforms.
use super::{Candidate, Config, State, Store, UPDATE_EXIT};
use std::{
    ffi::OsString,
    fs, io,
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::process::{Child, Command};

/// The original executable stays owned by its installer. Only verified payloads
/// under the private update directory are selected by this launcher.
pub async fn run(config: Config, arguments: Vec<OsString>) -> io::Result<()> {
    let mut signals = Signals::new()?;
    let store = Store::open(&config.directory)?;
    let lock = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(store.root.join("supervisor.lock"))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        lock.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    lock.try_lock()
        .map_err(|e| io::Error::other(format!("update supervisor already running: {e}")))?;
    let original = std::env::current_exe()?.canonicalize()?;
    let mut state = store.state()?;
    if state.trial {
        reject(&mut state);
        store.write("state.json", &state)?;
    }
    // A package upgrade takes precedence over an older downloaded overlay.
    if state.active.as_ref().is_some_and(|c| !newer_than_base(c)) {
        state.active = None;
        state.previous = None;
        store.write("state.json", &state)?;
    }
    loop {
        store.cleanup(&state)?;
        let candidate = state.pending.clone().or_else(|| state.active.clone());
        let trial = state.pending.is_some();
        let executable = match candidate.as_ref() {
            Some(candidate) => match store.verify(candidate) {
                Ok(path) => path,
                Err(error) => {
                    eprintln!("cached update rejected: {error}");
                    rollback(&store, &mut state, trial)?;
                    continue;
                }
            },
            None => original.clone(),
        };
        if let Some(candidate) = &candidate
            && let Err(error) = probe(&executable, candidate).await
        {
            eprintln!("update executable rejected: {error}");
            rollback(&store, &mut state, trial)?;
            continue;
        }
        if trial {
            state.trial = true;
            store.write("state.json", &state)?;
        }
        let ready = tempfile::Builder::new()
            .prefix(".e2em-update-")
            .tempfile_in(&store.root)?;
        let spawned = Command::new(&executable)
            .args(&arguments)
            .arg("--update-child")
            .arg("--update-parent")
            .arg(std::process::id().to_string())
            .arg("--update-ready-file")
            .arg(ready.path())
            .kill_on_drop(true)
            .spawn();
        let mut child = match spawned {
            Ok(child) => child,
            Err(error) if candidate.is_some() => {
                eprintln!("cannot launch update: {error}");
                rollback(&store, &mut state, trial)?;
                continue;
            }
            Err(error) => return Err(error),
        };
        let startup = await_startup(&mut child, ready.path(), trial, &mut signals).await;
        match startup {
            Ok(true) => {}
            Ok(false) => {
                return Ok(());
            } // owner requested shutdown
            Err(error) => {
                let _ = child.kill().await;
                eprintln!("runtime startup failed: {error}");
                if candidate.is_some() {
                    rollback(&store, &mut state, trial)?;
                    continue;
                }
                return Err(error);
            }
        }
        if trial {
            state.previous = state.active.take();
            state.active = state.pending.take();
            state.trial = false;
            store.write("state.json", &state)?;
            eprintln!(
                "background update active: {}",
                state.active.as_ref().unwrap().version
            );
        }
        let exit = loop {
            tokio::select! {
                result = child.wait() => break Some(result?),
                event = signals.wait() => match event {
                    Event::Shutdown => { stop_child(&mut child).await; break None; },
                    Event::Pressure => forward_pressure(child.id()),
                },
            }
        };
        let Some(exit) = exit else {
            return Ok(());
        };
        if exit.code() == Some(UPDATE_EXIT) {
            state = store.state()?;
            if state.pending.is_none() {
                return Err(io::Error::other("update restart has no staged candidate"));
            }
            continue;
        }
        if exit.success() {
            return Ok(());
        }
        if state.active.is_some() {
            eprintln!("updated runtime exited unexpectedly; restoring previous runtime");
            rollback(&store, &mut state, false)?;
            continue;
        }
        return Err(io::Error::other(format!("runtime exited with {exit}")));
    }
}
fn newer_than_base(candidate: &Candidate) -> bool {
    semver::Version::parse(&candidate.version)
        .is_ok_and(|v| v > semver::Version::parse(env!("CARGO_PKG_VERSION")).unwrap())
}
fn reject(state: &mut State) {
    if let Some(candidate) = state.pending.take() {
        record_rejection(state, candidate.version);
    }
    state.trial = false;
}
fn record_rejection(state: &mut State, version: String) {
    let old = state
        .rejected
        .as_ref()
        .and_then(|v| semver::Version::parse(v).ok());
    let new = semver::Version::parse(&version).ok();
    if old.is_none() || new > old {
        state.rejected = Some(version);
    }
}
fn rollback(store: &Store, state: &mut State, trial: bool) -> io::Result<()> {
    if trial {
        reject(state);
    } else {
        if let Some(candidate) = state.active.take() {
            record_rejection(state, candidate.version);
        }
        state.active = state.previous.take();
    }
    store.write("state.json", state)?;
    let mut status = store
        .read::<super::Status>("status.json")
        .unwrap_or_default();
    status.stage = "rolled_back".into();
    status.error = Some(
        "candidate failed verification, startup or runtime health; restored previous runtime"
            .into(),
    );
    store.write("status.json", &status)
}
async fn probe(path: &Path, candidate: &Candidate) -> io::Result<()> {
    let mut command = Command::new(path);
    command.arg("--version").kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(10), command.output()).await??;
    if !output.status.success()
        || output.stdout != format!("e2emd {}\n", candidate.version).as_bytes()
    {
        return Err(io::Error::other(
            "downloaded daemon version does not match release",
        ));
    }
    Ok(())
}
async fn await_startup(
    child: &mut Child,
    ready: &Path,
    trial: bool,
    signals: &mut Signals,
) -> io::Result<bool> {
    let pid = child.id();
    let startup = async {
        let mut tick = tokio::time::interval(Duration::from_millis(100));
        loop {
            if child.try_wait()?.is_some() {
                return Err(io::Error::other("runtime exited before becoming ready"));
            }
            if fs::metadata(ready)?.len() > 0 {
                break;
            }
            tick.tick().await;
        }
        if trial {
            tokio::select! {
                exit = child.wait() => { return Err(io::Error::other(format!("candidate exited during probation: {}", exit?))); },
                _ = tokio::time::sleep(Duration::from_secs(10)) => {},
            }
        }
        Ok(true)
    };
    let result = {
        let startup = tokio::time::timeout(Duration::from_secs(30), startup);
        tokio::pin!(startup);
        loop {
            tokio::select! {
                result = &mut startup => break Some(result.map_err(io::Error::other)?),
                event = signals.wait() => match event {
                    Event::Shutdown => break None,
                    Event::Pressure => forward_pressure(pid),
                },
            }
        }
    };
    match result {
        Some(result) => result,
        None => {
            stop_child(child).await;
            Ok(false)
        }
    }
}
#[allow(dead_code)] // Pressure events exist only on Unix.
enum Event {
    Shutdown,
    Pressure,
}
#[cfg(unix)]
struct Signals {
    interrupt: tokio::signal::unix::Signal,
    terminate: tokio::signal::unix::Signal,
    pressure: tokio::signal::unix::Signal,
}
#[cfg(windows)]
struct Signals {
    interrupt: tokio::signal::windows::CtrlC,
    terminate: tokio::signal::windows::CtrlBreak,
}
impl Signals {
    fn new() -> io::Result<Self> {
        #[cfg(unix)]
        {
            use tokio::signal::unix::{SignalKind, signal};
            Ok(Self {
                interrupt: signal(SignalKind::interrupt())?,
                terminate: signal(SignalKind::terminate())?,
                pressure: signal(SignalKind::user_defined1())?,
            })
        }
        #[cfg(windows)]
        {
            Ok(Self {
                interrupt: tokio::signal::windows::ctrl_c()?,
                terminate: tokio::signal::windows::ctrl_break()?,
            })
        }
    }
    async fn wait(&mut self) -> Event {
        #[cfg(unix)]
        {
            tokio::select! { _ = self.interrupt.recv() => Event::Shutdown, _ = self.terminate.recv() => Event::Shutdown, _ = self.pressure.recv() => Event::Pressure }
        }
        #[cfg(windows)]
        {
            tokio::select! { _ = self.interrupt.recv() => Event::Shutdown, _ = self.terminate.recv() => Event::Shutdown }
        }
    }
}
fn forward_pressure(pid: Option<u32>) {
    #[cfg(unix)]
    if let Some(pid) = pid.and_then(|id| rustix::process::Pid::from_raw(id as i32)) {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::USR1);
    }
    #[cfg(not(unix))]
    let _ = pid;
}
async fn stop_child(child: &mut Child) {
    #[cfg(unix)]
    if let Some(pid) = child
        .id()
        .and_then(|id| rustix::process::Pid::from_raw(id as i32))
    {
        let _ = rustix::process::kill_process(pid, rustix::process::Signal::TERM);
    }
    // Windows console events reach both processes. Allow normal endpoint cleanup.
    if tokio::time::timeout(Duration::from_secs(10), child.wait())
        .await
        .is_err()
    {
        let _ = child.kill().await;
    }
}

/// Root used by the setup tools; no dependence on system temp or a shared cache.
pub fn directory(grants: &Path) -> io::Result<PathBuf> {
    Ok(grants
        .canonicalize()?
        .parent()
        .ok_or_else(|| io::Error::other("missing grants directory"))?
        .join("updates"))
}

#[cfg(test)]
mod tests {
    use super::*;
    fn candidate(version: &str) -> Candidate {
        Candidate {
            version: version.into(),
            sha256: "a".repeat(64),
            size: 1,
        }
    }
    #[test]
    fn interrupted_trial_rejects_only_pending_candidate() {
        let mut state = State {
            active: Some(candidate("0.2.0")),
            pending: Some(candidate("0.3.0")),
            trial: true,
            ..State::default()
        };
        reject(&mut state);
        assert_eq!(state.active.unwrap().version, "0.2.0");
        assert_eq!(state.rejected.as_deref(), Some("0.3.0"));
        assert!(state.pending.is_none());
        assert!(!state.trial);
    }
    #[test]
    fn repeated_rollbacks_preserve_the_newest_rejected_version() {
        let mut state = State::default();
        record_rejection(&mut state, "0.3.0".into());
        record_rejection(&mut state, "0.2.0".into());
        assert_eq!(state.rejected.as_deref(), Some("0.3.0"));
    }
}
