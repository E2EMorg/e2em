//! Dispatch source callbacks; cancellation owns final context reclamation.
use std::{
    ffi::{c_char, c_void},
    io, ptr,
};
use tokio::sync::mpsc;

#[repr(C)]
struct SourceType {
    _opaque: [u8; 0],
}
#[link(name = "System")]
unsafe extern "C" {
    static _dispatch_source_type_memorypressure: SourceType;
    fn dispatch_queue_create(label: *const c_char, attributes: *const c_void) -> *mut c_void;
    fn dispatch_source_create(
        kind: *const SourceType,
        handle: usize,
        mask: usize,
        queue: *mut c_void,
    ) -> *mut c_void;
    fn dispatch_set_context(object: *mut c_void, context: *mut c_void);
    fn dispatch_source_set_event_handler_f(
        source: *mut c_void,
        handler: unsafe extern "C" fn(*mut c_void),
    );
    fn dispatch_source_set_cancel_handler_f(
        source: *mut c_void,
        handler: unsafe extern "C" fn(*mut c_void),
    );
    fn dispatch_resume(object: *mut c_void);
    fn dispatch_source_cancel(source: *mut c_void);
    fn dispatch_release(object: *mut c_void);
}
struct Context {
    sender: mpsc::Sender<()>,
    #[cfg(test)]
    cancelled: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
}
unsafe extern "C" fn notify(context: *mut c_void) {
    // SAFETY: this context is installed before resume and remains allocated
    // until the serial source's cancellation handler runs after its last event.
    let context = unsafe { &*context.cast::<Context>() };
    let _ = context.sender.try_send(());
}
unsafe extern "C" fn cancelled(context: *mut c_void) {
    // SAFETY: libdispatch invokes the cancellation handler exactly once after
    // event handlers finish; it receives the Box pointer installed on the source.
    let context = unsafe { Box::from_raw(context.cast::<Context>()) };
    #[cfg(test)]
    if let Some(flag) = &context.cancelled {
        flag.store(true, std::sync::atomic::Ordering::Release);
    }
    drop(context);
}
struct Source(*mut c_void);
// SAFETY: dispatch source control functions are thread-safe. The source is owned
// once; callback context lifetime is delegated to its serial cancellation handler.
unsafe impl Send for Source {}
impl Drop for Source {
    fn drop(&mut self) {
        // SAFETY: this owns a resumed source. Cancellation is asynchronous;
        // releasing our reference does not free context while callbacks run.
        unsafe {
            dispatch_source_cancel(self.0);
            dispatch_release(self.0);
        }
    }
}
pub struct PressureMonitor {
    _source: Source,
    receiver: mpsc::Receiver<()>,
}
impl PressureMonitor {
    pub fn open() -> io::Result<Self> {
        Self::create(
            &raw const _dispatch_source_type_memorypressure,
            0x02 | 0x04,
            None,
        )
    }
    fn create(
        kind: *const SourceType,
        mask: usize,
        _cancelled: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    ) -> io::Result<Self> {
        // SAFETY: the static label is NUL-terminated; NULL attributes select a
        // serial queue. All handlers are installed before the source is resumed.
        let queue =
            unsafe { dispatch_queue_create(c"org.e2em.memory-pressure".as_ptr(), ptr::null()) };
        if queue.is_null() {
            return Err(io::Error::other("cannot create pressure dispatch queue"));
        }
        // SAFETY: kind is a libdispatch source-type address; queue is live.
        let source = unsafe { dispatch_source_create(kind, 0, mask, queue) };
        // SAFETY: a successfully created source retains its target queue.
        unsafe {
            dispatch_release(queue);
        }
        if source.is_null() {
            return Err(io::Error::other(
                "cannot create memory-pressure dispatch source",
            ));
        }
        let (sender, receiver) = mpsc::channel(1);
        let context = Box::into_raw(Box::new(Context {
            sender,
            #[cfg(test)]
            cancelled: _cancelled,
        }))
        .cast();
        // SAFETY: source owns this context until its cancellation handler.
        unsafe {
            dispatch_set_context(source, context);
            dispatch_source_set_event_handler_f(source, notify);
            dispatch_source_set_cancel_handler_f(source, cancelled);
            dispatch_resume(source);
        }
        Ok(Self {
            _source: Source(source),
            receiver,
        })
    }
    pub async fn notified(&mut self) -> io::Result<()> {
        self.receiver
            .recv()
            .await
            .ok_or_else(|| io::Error::other("macOS pressure event source stopped"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::Duration,
    };
    unsafe extern "C" {
        static _dispatch_source_type_data_add: SourceType;
        fn dispatch_source_merge_data(source: *mut c_void, data: usize);
    }
    #[test]
    fn dispatch_callbacks_rearm_after_cancelled_waits_and_reclaim_context() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let cancelled = Arc::new(AtomicBool::new(false));
            // A user-data source exercises the same callback/cancellation path
            // without exhausting system memory to generate real OS pressure.
            let mut monitor = PressureMonitor::create(
                &raw const _dispatch_source_type_data_add,
                0,
                Some(cancelled.clone()),
            )
            .unwrap();
            assert!(
                tokio::time::timeout(Duration::from_millis(20), monitor.notified())
                    .await
                    .is_err()
            );
            for _ in 0..2 {
                // SAFETY: the live source is a DATA_ADD source supporting merge.
                unsafe {
                    dispatch_source_merge_data(monitor._source.0, 1);
                }
                tokio::time::timeout(Duration::from_secs(1), monitor.notified())
                    .await
                    .unwrap()
                    .unwrap();
                assert!(
                    tokio::time::timeout(Duration::from_millis(20), monitor.notified())
                        .await
                        .is_err()
                );
            }
            drop(monitor);
            tokio::time::timeout(Duration::from_secs(1), async {
                while !cancelled.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(1)).await;
                }
            })
            .await
            .unwrap();
        });
    }
}
