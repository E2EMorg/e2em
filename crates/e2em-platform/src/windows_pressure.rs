//! Kernel event waits, coalesced until memory availability recovers. No polling.
use std::{io, ptr, sync::Arc, thread};
use tokio::sync::mpsc;
use windows_sys::Win32::{
    Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0},
    System::{
        Memory::{
            CreateMemoryResourceNotification, HighMemoryResourceNotification,
            LowMemoryResourceNotification,
        },
        Threading::{CreateEventW, INFINITE, SetEvent, WaitForMultipleObjects},
    },
};
struct KernelHandle(HANDLE);
// SAFETY: kernel event handles may be waited on/signaled across threads. Arc
// retains each owned handle until every wait and signal has finished.
unsafe impl Send for KernelHandle {}
unsafe impl Sync for KernelHandle {}
impl Drop for KernelHandle {
    fn drop(&mut self) {
        // SAFETY: this object owns one valid handle, and all borrowers are gone.
        unsafe {
            CloseHandle(self.0);
        }
    }
}
impl KernelHandle {
    fn checked(handle: HANDLE) -> io::Result<Arc<Self>> {
        if handle.is_null() {
            Err(io::Error::last_os_error())
        } else {
            Ok(Arc::new(Self(handle)))
        }
    }
    fn event() -> io::Result<Arc<Self>> {
        // SAFETY: unnamed, manual-reset event with no inherited handle.
        Self::checked(unsafe { CreateEventW(ptr::null(), 1, 0, ptr::null()) })
    }
    fn signal(&self) {
        // SAFETY: the live owned handle is a signalable event.
        unsafe {
            SetEvent(self.0);
        }
    }
}

pub struct PressureMonitor {
    receiver: mpsc::Receiver<()>,
    stop: Arc<KernelHandle>,
    worker: Option<thread::JoinHandle<()>>,
}
impl PressureMonitor {
    pub fn open() -> io::Result<Self> {
        // SAFETY: both enum values are documented resource-notification types.
        let low = KernelHandle::checked(unsafe {
            CreateMemoryResourceNotification(LowMemoryResourceNotification)
        })?;
        let high = KernelHandle::checked(unsafe {
            CreateMemoryResourceNotification(HighMemoryResourceNotification)
        })?;
        Self::from_events(low, high)
    }
    fn from_events(low: Arc<KernelHandle>, high: Arc<KernelHandle>) -> io::Result<Self> {
        let stop = KernelHandle::event()?;
        let shutdown = stop.clone();
        let (sender, receiver) = mpsc::channel(1);
        let worker = thread::Builder::new()
            .name("e2em-pressure".into())
            .spawn(move || {
                let mut awaiting_recovery = false;
                loop {
                    let handles = [shutdown.0, if awaiting_recovery { high.0 } else { low.0 }];
                    // SAFETY: both handles remain owned throughout the kernel wait.
                    // Index zero gives shutdown priority when multiple events fire.
                    let event = unsafe { WaitForMultipleObjects(2, handles.as_ptr(), 0, INFINITE) };
                    if event == WAIT_OBJECT_0 {
                        break;
                    }
                    if event != WAIT_OBJECT_0 + 1 {
                        break;
                    }
                    if !awaiting_recovery {
                        let _ = sender.try_send(());
                    }
                    // A level-triggered low-memory event can remain signaled. Wait
                    // for recovery before rearming, rather than spinning/reloading.
                    awaiting_recovery = !awaiting_recovery;
                }
            })?;
        Ok(Self {
            receiver,
            stop,
            worker: Some(worker),
        })
    }
    pub async fn notified(&mut self) -> io::Result<()> {
        self.receiver
            .recv()
            .await
            .ok_or_else(|| io::Error::other("Windows pressure event source stopped"))
    }
}
impl Drop for PressureMonitor {
    fn drop(&mut self) {
        self.stop.signal();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use windows_sys::Win32::System::Threading::ResetEvent;

    #[test]
    fn pressure_coalesces_rearms_and_shutdown_interrupts_both_waits() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            for stage in 0..3 {
                let low = KernelHandle::event().unwrap();
                // Auto-reset models a recovery transition without an impossible
                // simultaneous sustained high+low memory condition.
                let high =
                    KernelHandle::checked(unsafe { CreateEventW(ptr::null(), 0, 0, ptr::null()) })
                        .unwrap();
                let mut monitor = PressureMonitor::from_events(low.clone(), high.clone()).unwrap();
                assert!(
                    tokio::time::timeout(Duration::from_millis(20), monitor.notified())
                        .await
                        .is_err()
                );
                if stage == 0 {
                    let start = std::time::Instant::now();
                    drop(monitor);
                    assert!(start.elapsed() < Duration::from_secs(1));
                    continue;
                }
                low.signal();
                tokio::time::timeout(Duration::from_secs(1), monitor.notified())
                    .await
                    .unwrap()
                    .unwrap();
                assert!(
                    tokio::time::timeout(Duration::from_millis(20), monitor.notified())
                        .await
                        .is_err(),
                    "sustained pressure must not spin"
                );
                if stage == 2 {
                    // SAFETY: low is an owned synthetic manual-reset event.
                    unsafe {
                        ResetEvent(low.0);
                    }
                    high.signal();
                    assert!(
                        tokio::time::timeout(Duration::from_millis(20), monitor.notified())
                            .await
                            .is_err()
                    );
                    low.signal();
                    tokio::time::timeout(Duration::from_secs(1), monitor.notified())
                        .await
                        .unwrap()
                        .unwrap();
                }
                let start = std::time::Instant::now();
                drop(monitor);
                assert!(start.elapsed() < Duration::from_secs(1));
            }
        });
    }
}
