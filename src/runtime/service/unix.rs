use super::super::{scheduler::Scheduler, *};
use super::common::{Grants, connection_loop};
use std::{
    os::unix::fs::{FileTypeExt, MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::{UnixListener, UnixStream},
    sync::Semaphore,
};

pub fn read_grants(path: &Path) -> std::io::Result<Grants> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.mode() & 0o077 != 0
        || metadata.len() > 65_536
    {
        return Err(std::io::Error::other(
            "grant file must be an owned private regular file",
        ));
    }
    let grants: Grants =
        serde_json::from_slice(&std::fs::read(path)?).map_err(std::io::Error::other)?;
    if !identifier(&grants.provider)
        || grants.grants.len() > 64
        || grants.grants.iter().any(|g| {
            !identifier(&g.principal)
                || g.secret.len() != 64
                || !g.secret.bytes().all(|b| b.is_ascii_hexdigit())
        })
    {
        return Err(std::io::Error::other("invalid grants"));
    }
    let unique: std::collections::HashSet<_> = grants.grants.iter().map(|g| &g.principal).collect();
    if unique.len() != grants.grants.len() {
        return Err(std::io::Error::other("duplicate principal"));
    }
    Ok(grants)
}
struct Endpoint {
    path: PathBuf,
    inode: u64,
}
impl Drop for Endpoint {
    fn drop(&mut self) {
        if std::fs::symlink_metadata(&self.path).is_ok_and(|m| m.ino() == self.inode) {
            let _ = std::fs::remove_file(&self.path);
        }
    }
}
/// Refuses existing endpoints. Service managers remove an owned stale socket
/// explicitly after verifying the old process has stopped.
pub async fn serve(socket: &Path, grants_path: &Path, idle: Duration) -> std::io::Result<()> {
    read_grants(grants_path)?;
    let parent = socket
        .parent()
        .ok_or_else(|| std::io::Error::other("missing socket directory"))?;
    let metadata = std::fs::symlink_metadata(parent)?;
    if !metadata.is_dir()
        || metadata.uid() != rustix::process::getuid().as_raw()
        || metadata.mode() & 0o077 != 0
    {
        return Err(std::io::Error::other(
            "socket directory must be owned and mode 0700",
        ));
    }
    if socket.exists() {
        let stale = std::fs::symlink_metadata(socket)?;
        if !stale.file_type().is_socket()
            || stale.uid() != rustix::process::getuid().as_raw()
            || stale.mode() & 0o077 != 0
        {
            return Err(std::io::Error::other(
                "refusing an unowned or non-private endpoint",
            ));
        }
        match UnixStream::connect(socket).await {
            Ok(_) => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::AddrInUse,
                    "provider already running",
                ));
            }
            Err(error) if error.kind() == std::io::ErrorKind::ConnectionRefused => {
                if std::fs::symlink_metadata(socket)?.ino() != stale.ino() {
                    return Err(std::io::Error::other("endpoint changed"));
                }
                std::fs::remove_file(socket)?;
            }
            Err(error) => return Err(error),
        }
    }
    let listener = UnixListener::bind(socket)?;
    std::fs::set_permissions(socket, std::fs::Permissions::from_mode(0o600))?;
    let _endpoint = Endpoint {
        path: socket.into(),
        inode: std::fs::symlink_metadata(socket)?.ino(),
    };
    let scheduler = Arc::new(Scheduler::new(idle));
    let connections = Arc::new(Semaphore::new(32));
    let mut termination =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut pressure =
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::user_defined1())?;
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    let mut os_pressure = match super::pressure::Monitor::open() {
        Ok(monitor) => Some(monitor),
        Err(error) => {
            eprintln!(
                "automatic memory-pressure monitor unavailable: {error}; idle eviction and SIGUSR1 remain enabled"
            );
            None
        }
    };
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let os_pressure = None;
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = termination.recv() => break,
            _ = pressure.recv() => { scheduler.memory_pressure(); },
            event = super::pressure::wait_optional(&os_pressure) => {
                match event {
                    Ok(()) => { scheduler.memory_pressure(); },
                    Err(error) => {
                        eprintln!("automatic memory-pressure monitor stopped: {error}");
                        #[cfg(any(target_os = "linux", target_os = "macos"))]
                        { os_pressure = None; }
                    }
                }
            },
            connection = listener.accept() => {
                let (stream, _) = connection?;
                let Ok(permit) = connections.clone().try_acquire_owned() else { continue; };
                let scheduler = scheduler.clone(); let grants_path = grants_path.to_owned();
                tokio::spawn(async move {
                    let _permit = permit;
                    if let Ok(peer) = stream.peer_cred() {
                        let _ = connection_loop(stream, peer.uid(), read_grants, grants_path, scheduler).await;
                    }
                });
            }
        }
    }
    Ok(())
}
