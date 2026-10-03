//! Versioned C ABI. Opaque integer handles never expose Rust layouts.
use e2em_runtime::runtime::{
    scheduler::{Pending, Scheduler},
    *,
};
use std::{
    collections::HashMap,
    panic::{AssertUnwindSafe, catch_unwind},
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex, OnceLock},
};
struct Client {
    scheduler: Arc<Scheduler>,
    principal: String,
}
enum Output {
    Pending(Pending),
    Ready(Vec<u8>),
}
#[derive(Default)]
struct Registry {
    clients: HashMap<u64, Arc<Client>>,
    outputs: HashMap<u64, Output>,
}
static REGISTRY: OnceLock<Mutex<Registry>> = OnceLock::new();
static NEXT: AtomicU64 = AtomicU64::new(1);
fn registry() -> &'static Mutex<Registry> {
    REGISTRY.get_or_init(Mutex::default)
}
fn guarded<T: Default>(f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_default()
}
fn ready(reply: Reply) -> Output {
    Output::Ready(serde_json::to_vec(&reply).expect("reply serializes"))
}
fn error(code: ErrorCode) -> Output {
    ready(Reply::Error { error_code: code })
}
#[unsafe(no_mangle)]
pub extern "C" fn e2em_abi_version() -> u32 {
    1
}
#[unsafe(no_mangle)]
pub extern "C" fn e2em_open(abi: u32) -> u64 {
    guarded(|| {
        if abi != 1 {
            return 0;
        }
        let mut registry = registry().lock().expect("registry lock");
        if registry.clients.len() >= 64 {
            return 0;
        }
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        registry.clients.insert(
            id,
            Arc::new(Client {
                scheduler: Arc::new(Scheduler::default()),
                principal: format!("ffi-{id}"),
            }),
        );
        id
    })
}
/// Open a provisioned native model store without downloading assets in the SDK.
/// # Safety
/// `config_path` must point to `len` readable UTF-8 bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e2em_open_models(abi: u32, config_path: *const u8, len: usize) -> u64 {
    guarded(|| {
        if abi != 1 || config_path.is_null() || len == 0 || len > 4096 {
            return 0;
        }
        // SAFETY: documented caller precondition; pointer/length checked above.
        let bytes = unsafe { std::slice::from_raw_parts(config_path, len) };
        let Ok(path) = std::str::from_utf8(bytes) else {
            return 0;
        };
        let path = std::path::PathBuf::from(path);
        if !path.is_absolute() {
            return 0;
        }
        let Ok(scheduler) = Scheduler::with_models(
            std::time::Duration::from_secs(300),
            models::Manager::new(path),
        ) else {
            return 0;
        };
        let mut registry = registry().lock().expect("registry lock");
        if registry.clients.len() >= 64 {
            return 0;
        }
        let id = NEXT.fetch_add(1, Ordering::Relaxed);
        registry.clients.insert(
            id,
            Arc::new(Client {
                scheduler: Arc::new(scheduler),
                principal: format!("ffi-{id}"),
            }),
        );
        id
    })
}
/// # Safety
/// `input` must point to `len` readable bytes for the duration of this call.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e2em_call(client: u64, input: *const u8, len: usize) -> u64 {
    guarded(|| {
        if input.is_null() || len == 0 || len > MAX_FRAME {
            return 0;
        }
        let (client, id) = {
            let mut registry = registry().lock().expect("registry lock");
            let Some(client) = registry.clients.get(&client).cloned() else {
                return 0;
            };
            if registry.outputs.len() >= 256 {
                return 0;
            }
            let id = NEXT.fetch_add(1, Ordering::Relaxed);
            registry.outputs.insert(id, error(ErrorCode::InternalError));
            (client, id)
        };
        // SAFETY: documented caller precondition, null and size checked above.
        let input = unsafe { std::slice::from_raw_parts(input, len) };
        let output = match decode_call(input) {
            Err(failure) => error(failure.error_code),
            Ok(call) if call.api_version != API_VERSION => error(ErrorCode::UnsupportedVersion),
            Ok(call) if !identifier(&call.call_id) => error(ErrorCode::InvalidRequest),
            Ok(call) => match call.operation {
                Operation::Models | Operation::InstallModel { .. } => {
                    error(ErrorCode::UnsupportedPolicy)
                }
                Operation::Capabilities => ready(Reply::Capabilities {
                    capabilities: client.scheduler.capabilities(),
                }),
                Operation::ValidatePolicy { policy } => {
                    match client.scheduler.validate_policy(&client.principal, policy) {
                        Ok(policy_ref) => ready(Reply::Policy { policy_ref }),
                        Err(code) => error(code),
                    }
                }
                Operation::Assess { request } => {
                    match client.scheduler.submit(&client.principal, *request) {
                        Ok(pending) => Output::Pending(pending),
                        Err(code) => error(code),
                    }
                }
                Operation::Cancel { request_id } => ready(Reply::Cancelled {
                    accepted: client.scheduler.cancel(&client.principal, &request_id),
                }),
            },
        };
        let mut registry = registry().lock().expect("registry lock");
        registry.outputs.insert(id, output);
        id
    })
}
/// Returns 1 ready, 0 pending, -1 invalid. May be called concurrently.
#[unsafe(no_mangle)]
pub extern "C" fn e2em_poll(result: u64) -> i32 {
    catch_unwind(AssertUnwindSafe(|| {
        let mut registry = registry().lock().expect("registry lock");
        let Some(output) = registry.outputs.get_mut(&result) else {
            return -1;
        };
        match output {
            Output::Ready(_) => 1,
            Output::Pending(pending) => match pending.try_recv() {
                Ok(None) => 0,
                Ok(Some(assessment)) => {
                    *output = ready(Reply::Assessment { assessment });
                    1
                }
                Err(code) => {
                    *output = error(code);
                    1
                }
            },
        }
    }))
    .unwrap_or(-1)
}
/// Returns required length (no terminator); copies only if capacity suffices.
/// # Safety
/// A non-null `buffer` must point to `capacity` writable bytes.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn e2em_result_read(result: u64, buffer: *mut u8, capacity: usize) -> usize {
    guarded(|| {
        let registry = registry().lock().expect("registry lock");
        let Some(Output::Ready(bytes)) = registry.outputs.get(&result) else {
            return 0;
        };
        if !buffer.is_null() && capacity >= bytes.len() {
            // SAFETY: documented caller precondition; copy contains bytes.len() bytes.
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buffer, bytes.len()) };
        }
        bytes.len()
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn e2em_result_free(result: u64) {
    guarded(|| {
        registry()
            .lock()
            .expect("registry lock")
            .outputs
            .remove(&result);
    });
}
#[unsafe(no_mangle)]
pub extern "C" fn e2em_close(client: u64) {
    guarded(|| {
        let client = registry()
            .lock()
            .expect("registry lock")
            .clients
            .remove(&client);
        if let Some(client) = client {
            client.scheduler.disconnect(&client.principal);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_abi_and_disposed_handles() {
        assert_eq!(e2em_open(2), 0);
        let client = e2em_open(1);
        assert_ne!(client, 0);
        let input = br#"{"call_id":"c","api_version":"0.1","operation":{"op":"capabilities"}}"#;
        let output = unsafe { e2em_call(client, input.as_ptr(), input.len()) };
        assert_eq!(e2em_poll(output), 1);
        assert!(unsafe { e2em_result_read(output, std::ptr::null_mut(), 0) } > 0);
        e2em_result_free(output);
        assert_eq!(e2em_poll(output), -1);
        e2em_close(client);
        e2em_close(client);
        assert_eq!(unsafe { e2em_call(client, input.as_ptr(), input.len()) }, 0);
    }
    #[test]
    fn concurrent_calls_and_disposal() {
        let client = e2em_open(1);
        let threads: Vec<_> = (0..8)
            .map(|_| {
                std::thread::spawn(move || {
                    let input =
                        br#"{"call_id":"c","api_version":"0.1","operation":{"op":"capabilities"}}"#;
                    for _ in 0..20 {
                        let result = unsafe { e2em_call(client, input.as_ptr(), input.len()) };
                        if result != 0 {
                            assert_eq!(e2em_poll(result), 1);
                            e2em_result_free(result);
                        }
                    }
                })
            })
            .collect();
        e2em_close(client);
        for thread in threads {
            thread.join().unwrap();
        }
    }
}
