//! Event-driven Linux PSI notifications. No timer or periodic pressure reads.
#[cfg(target_os = "linux")]
use std::{fs::File, io::Write, os::fd::AsRawFd};
#[cfg(target_os = "linux")]
use tokio::io::{Interest, unix::AsyncFd};

#[cfg(target_os = "linux")]
pub(super) struct Monitor(AsyncFd<File>);
#[cfg(target_os = "linux")]
impl Monitor {
    pub(super) fn open() -> std::io::Result<Self> {
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/proc/pressure/memory")?;
        // Unprivileged PSI monitors require a window that is a multiple of 2s.
        // Request eviction after 300ms of partial memory stalls in that window.
        file.write_all(b"some 300000 2000000\0")?;
        Ok(Self(AsyncFd::with_interest(file, Interest::PRIORITY)?))
    }
}
#[cfg(target_os = "linux")]
async fn wait_notification<T: AsRawFd>(fd: &AsyncFd<T>) -> std::io::Result<()> {
    let mut event = fd.ready(Interest::PRIORITY).await?;
    let ready = event.ready();
    event.clear_ready();
    if ready.is_error() || ready.is_read_closed() {
        return Err(std::io::Error::other("pressure event source closed"));
    }
    Ok(())
}
#[cfg(target_os = "macos")]
pub(super) struct Monitor(tokio::sync::Mutex<e2em_platform::PressureMonitor>);
#[cfg(target_os = "macos")]
impl Monitor {
    pub(super) fn open() -> std::io::Result<Self> {
        Ok(Self(tokio::sync::Mutex::new(
            e2em_platform::PressureMonitor::open()?,
        )))
    }
}
#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) struct Monitor;

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
pub(super) async fn wait_optional(_: &Option<Monitor>) -> std::io::Result<()> {
    std::future::pending().await
}

#[cfg(target_os = "linux")]
pub(super) async fn wait_optional(monitor: &Option<Monitor>) -> std::io::Result<()> {
    match monitor {
        Some(monitor) => wait_notification(&monitor.0).await,
        None => std::future::pending().await,
    }
}

#[cfg(all(test, target_os = "linux"))]
mod tests {
    use super::*;
    use std::{
        net::{TcpListener, TcpStream},
        time::Duration,
    };

    #[tokio::test]
    async fn priority_notifications_sleep_rearm_and_survive_cancelled_waits() {
        // TCP urgent data produces the same EPOLLPRI readiness as PSI, without
        // putting the host under memory pressure or requiring kernel privileges.
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let sender = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let (receiver, _) = listener.accept().unwrap();
        receiver.set_nonblocking(true).unwrap();
        let fd = AsyncFd::with_interest(receiver, Interest::PRIORITY).unwrap();
        assert!(
            tokio::time::timeout(Duration::from_millis(20), wait_notification(&fd))
                .await
                .is_err()
        );
        for _ in 0..2 {
            rustix::net::send(&sender, b"x", rustix::net::SendFlags::OOB).unwrap();
            tokio::time::timeout(Duration::from_secs(1), wait_notification(&fd))
                .await
                .unwrap()
                .unwrap();
            let mut byte = [0u8];
            rustix::net::recv(fd.get_ref(), &mut byte, rustix::net::RecvFlags::OOB).unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(20), wait_notification(&fd))
                    .await
                    .is_err()
            );
        }
    }

    #[tokio::test]
    async fn unavailable_monitor_remains_asleep() {
        assert!(
            tokio::time::timeout(Duration::from_millis(20), wait_optional(&None))
                .await
                .is_err()
        );
    }
}

#[cfg(target_os = "macos")]
pub(super) async fn wait_optional(monitor: &Option<Monitor>) -> std::io::Result<()> {
    match monitor {
        Some(monitor) => monitor.0.lock().await.notified().await,
        None => std::future::pending().await,
    }
}
