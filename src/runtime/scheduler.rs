//! Bounded fair single-worker executor. Deadlines start at admission.
use super::*;
use serde::{Deserialize, Serialize};
use std::{
    collections::{HashMap, VecDeque},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum State {
    Unloaded,
    Loading,
    Ready,
    Assessing,
    Evicting,
    Failed,
}
struct Job {
    principal: String,
    request: Request,
    deadline: Instant,
    cancelled: Arc<AtomicBool>,
    reply: mpsc::Sender<Assessment>,
    wake: Arc<Mutex<Option<std::task::Waker>>>,
}
struct Inner {
    queues: HashMap<String, VecDeque<Job>>,
    order: VecDeque<String>,
    active: HashMap<(String, String), Arc<AtomicBool>>,
    state: State,
    stopping: bool,
    evict_requested: bool,
}
struct Shared {
    inner: Mutex<Inner>,
    changed: Condvar,
    engine: Engine<SharedScorer>,
    idle: Duration,
    backend: Arc<LoadedBackend>,
}
pub type Backend = Box<dyn crate::PolicyScorer + Send + Sync>;
struct LoadedBackend {
    scorer: Mutex<Option<Backend>>,
    factory: Box<dyn Fn() -> Result<Backend, crate::BackendError> + Send + Sync>,
}
impl LoadedBackend {
    fn load(&self) -> Result<(), crate::BackendError> {
        let mut scorer = self
            .scorer
            .lock()
            .map_err(|_| crate::BackendError::new("loader failed"))?;
        if scorer.is_none() {
            *scorer = Some((self.factory)()?);
        }
        Ok(())
    }
    fn unload(&self) {
        let mut scorer = self
            .scorer
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        *scorer = None;
    }
    fn loaded(&self) -> bool {
        self.scorer.lock().is_ok_and(|s| s.is_some())
    }
}
struct SharedScorer(Arc<LoadedBackend>);
impl crate::PolicyScorer for SharedScorer {
    fn supports_model_categories(&self) -> bool {
        self.0.scorer.lock().is_ok_and(|scorer| {
            scorer
                .as_ref()
                .is_some_and(|scorer| scorer.supports_model_categories())
        })
    }
    fn model_version(&self) -> String {
        self.0
            .scorer
            .lock()
            .ok()
            .and_then(|scorer| scorer.as_ref().map(|scorer| scorer.model_version()))
            .unwrap_or_else(|| "none".into())
    }
    fn score(
        &self,
        message: &str,
        context: Option<&str>,
        policy: &str,
    ) -> Result<f64, crate::BackendError> {
        let scorer = self
            .0
            .scorer
            .lock()
            .map_err(|_| crate::BackendError::new("scorer failed"))?;
        scorer
            .as_ref()
            .ok_or_else(|| crate::BackendError::new("backend unloaded"))?
            .score(message, context, policy)
    }
}
pub struct Scheduler {
    shared: Arc<Shared>,
    worker: Option<thread::JoinHandle<()>>,
}
pub struct Pending {
    reply: mpsc::Receiver<Assessment>,
    wake: Arc<Mutex<Option<std::task::Waker>>>,
    cancelled: Arc<AtomicBool>,
}
impl Pending {
    pub fn wait(self) -> Result<Assessment, ErrorCode> {
        self.reply.recv().map_err(|_| ErrorCode::ModelUnavailable)
    }
    pub fn try_recv(&self) -> Result<Option<Assessment>, ErrorCode> {
        match self.reply.try_recv() {
            Ok(a) => Ok(Some(a)),
            Err(mpsc::TryRecvError::Empty) => Ok(None),
            Err(_) => Err(ErrorCode::ModelUnavailable),
        }
    }
}
impl Drop for Pending {
    fn drop(&mut self) {
        self.cancelled.store(true, Ordering::Release);
    }
}
impl std::future::Future for Pending {
    type Output = Result<Assessment, ErrorCode>;
    fn poll(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Self::Output> {
        match self.try_recv() {
            Ok(Some(result)) => return std::task::Poll::Ready(Ok(result)),
            Err(error) => return std::task::Poll::Ready(Err(error)),
            Ok(None) => (),
        }
        if let Ok(mut wake) = self.wake.lock() {
            *wake = Some(cx.waker().clone());
        }
        // Close the completion-before-registration race.
        match self.try_recv() {
            Ok(Some(result)) => std::task::Poll::Ready(Ok(result)),
            Err(error) => std::task::Poll::Ready(Err(error)),
            Ok(None) => std::task::Poll::Pending,
        }
    }
}
impl Default for Scheduler {
    fn default() -> Self {
        Self::new(Duration::from_secs(300))
    }
}
impl Scheduler {
    pub fn new(idle: Duration) -> Self {
        Self::with_factory(idle, || Ok(Box::new(crate::RulesScorer::new())))
    }
    /// Factories must be local, bounded and data-only. Model backends opt into
    /// arbitrary named category scoring; evaluation ratings do not gate access.
    pub fn with_factory(
        idle: Duration,
        factory: impl Fn() -> Result<Backend, crate::BackendError> + Send + Sync + 'static,
    ) -> Self {
        let backend = Arc::new(LoadedBackend {
            scorer: Mutex::new(None),
            factory: Box::new(factory),
        });
        let shared = Arc::new(Shared {
            inner: Mutex::new(Inner {
                queues: HashMap::new(),
                order: VecDeque::new(),
                active: HashMap::new(),
                state: State::Unloaded,
                stopping: false,
                evict_requested: false,
            }),
            changed: Condvar::new(),
            engine: Engine::new(SharedScorer(backend.clone())),
            backend,
            idle,
        });
        let executor = shared.clone();
        let worker = thread::spawn(move || run(executor));
        Self {
            shared,
            worker: Some(worker),
        }
    }
    pub fn capabilities(&self) -> Capabilities {
        let mut capabilities = self.shared.engine.capabilities();
        capabilities.runtime_state = self.state();
        capabilities.backend_ready = capabilities.runtime_state != State::Failed;
        capabilities
    }
    pub fn validate_policy(&self, principal: &str, policy: Policy) -> Result<PolicyRef, ErrorCode> {
        self.shared.engine.validate_policy(principal, policy)
    }
    pub fn submit(&self, principal: &str, request: Request) -> Result<Pending, ErrorCode> {
        let admitted = Instant::now();
        validate_request(&request)?;
        // Validate authority before admission, not only when inference starts.
        self.shared.engine.resolve(principal, &request)?;
        let deadline = admitted + Duration::from_millis(request.options.deadline_ms);
        let mut inner = self
            .shared
            .inner
            .lock()
            .map_err(|_| ErrorCode::InternalError)?;
        if inner.stopping || inner.state == State::Failed {
            return Err(ErrorCode::ModelUnavailable);
        }
        if inner.evict_requested
            || inner.state == State::Evicting
            || inner.active.len() >= 64
            || inner
                .active
                .keys()
                .filter(|(owner, _)| owner == principal)
                .count()
                >= 8
        {
            return Err(ErrorCode::ResourceExhausted);
        }
        let key = (principal.to_owned(), request.request_id.clone());
        if inner.active.contains_key(&key) {
            return Err(ErrorCode::InvalidRequest);
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        let (reply, receive) = mpsc::channel();
        let wake = Arc::new(Mutex::new(None));
        inner.active.insert(key, cancelled.clone());
        let queue = inner.queues.entry(principal.into()).or_default();
        let empty = queue.is_empty();
        queue.push_back(Job {
            principal: principal.into(),
            request,
            deadline,
            cancelled: cancelled.clone(),
            reply,
            wake: wake.clone(),
        });
        if empty {
            inner.order.push_back(principal.into());
        }
        self.shared.changed.notify_one();
        Ok(Pending {
            reply: receive,
            wake,
            cancelled,
        })
    }
    pub fn cancel(&self, principal: &str, request_id: &str) -> bool {
        let Ok(inner) = self.shared.inner.lock() else {
            return false;
        };
        if let Some(flag) = inner.active.get(&(principal.into(), request_id.into())) {
            flag.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }
    pub fn disconnect(&self, principal: &str) {
        if let Ok(inner) = self.shared.inner.lock() {
            for ((owner, _), flag) in &inner.active {
                if owner == principal {
                    flag.store(true, Ordering::Release);
                }
            }
        }
    }
    pub fn revoke(&self, principal: &str) {
        self.disconnect(principal);
        self.shared.engine.revoke(principal);
    }
    pub fn state(&self) -> State {
        self.shared
            .inner
            .lock()
            .map(|i| i.state)
            .unwrap_or(State::Failed)
    }
    /// Eviction cannot free assets under an active assessment. Rules preview has
    /// no model weights; its scorer allocation is released by the same loader.
    pub fn memory_pressure(&self) -> bool {
        let Ok(mut inner) = self.shared.inner.lock() else {
            return false;
        };
        if inner.state == State::Failed {
            self.shared.backend.unload();
            return true;
        }
        if !inner.active.is_empty() {
            inner.evict_requested = true;
            return false;
        }
        inner.state = State::Evicting;
        self.shared.backend.unload();
        inner.evict_requested = false;
        inner.state = State::Unloaded;
        true
    }
}
impl Drop for Scheduler {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.shared.inner.lock() {
            inner.stopping = true;
            for flag in inner.active.values() {
                flag.store(true, Ordering::Release);
            }
            self.shared.changed.notify_all();
        }
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn run(shared: Arc<Shared>) {
    let mut last_completed = Instant::now();
    loop {
        let job = {
            let Ok(mut inner) = shared.inner.lock() else {
                return;
            };
            while inner.order.is_empty() {
                if inner.stopping {
                    return;
                }
                if inner.state == State::Failed {
                    inner = match shared.changed.wait(inner) {
                        Ok(i) => i,
                        Err(_) => return,
                    };
                    continue;
                }
                let remaining = shared.idle.saturating_sub(last_completed.elapsed());
                if inner.state == State::Unloaded || remaining.is_zero() {
                    inner.state = State::Evicting;
                    shared.backend.unload();
                    inner.state = State::Unloaded;
                    inner = match shared.changed.wait(inner) {
                        Ok(i) => i,
                        Err(_) => return,
                    };
                } else {
                    inner = match shared.changed.wait_timeout(inner, remaining) {
                        Ok((i, _)) => i,
                        Err(_) => return,
                    };
                }
            }
            let owner = inner.order.pop_front().expect("nonempty queue");
            let queue = inner.queues.get_mut(&owner).expect("queue owner");
            let job = queue.pop_front().expect("queued job");
            if queue.is_empty() {
                inner.queues.remove(&owner);
            } else {
                inner.order.push_back(owner);
            }
            if inner.state == State::Unloaded {
                inner.state = State::Loading;
            }
            job
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if job.cancelled.load(Ordering::Acquire) || Instant::now() >= job.deadline {
                base_result(&job.request)
            } else {
                if shared.backend.load().is_err() {
                    let mut result = base_result(&job.request);
                    fail(&mut result, ErrorCode::ModelUnavailable);
                    return result;
                }
                if let Ok(mut inner) = shared.inner.lock() {
                    inner.state = State::Assessing;
                }
                if Instant::now() >= job.deadline {
                    let mut result = base_result(&job.request);
                    fail(&mut result, ErrorCode::DeadlineExceeded);
                    result
                } else {
                    shared.engine.assess(&job.principal, &job.request)
                }
            }
        }));
        let mut result = match outcome {
            Ok(result) => result,
            Err(_) => {
                let mut result = base_result(&job.request);
                fail(&mut result, ErrorCode::InternalError);
                result
            }
        };
        if job.cancelled.load(Ordering::Acquire) {
            result.status = Status::Cancelled;
            result.action = Action::Review;
            result.error_code = None;
            result.reason_codes = vec!["CANCELLED".into()];
        } else if Instant::now() >= job.deadline {
            fail(&mut result, ErrorCode::DeadlineExceeded);
        }
        // Completion and cancellation linearize under the same lock.
        if let Ok(mut inner) = shared.inner.lock() {
            if job.cancelled.load(Ordering::Acquire) {
                result.status = Status::Cancelled;
                result.action = Action::Review;
                result.error_code = None;
                result.reason_codes = vec!["CANCELLED".into()];
            }
            let failed = matches!(
                result.error_code,
                Some(ErrorCode::ModelUnavailable | ErrorCode::InternalError)
            );
            let _ = job.reply.send(result);
            if let Ok(mut wake) = job.wake.lock()
                && let Some(waker) = wake.take()
            {
                waker.wake();
            }
            inner
                .active
                .remove(&(job.principal, job.request.request_id));
            inner.state = if failed {
                shared.backend.unload();
                State::Failed
            } else if shared.backend.loaded() {
                State::Ready
            } else {
                State::Unloaded
            };
            if inner.evict_requested && inner.active.is_empty() {
                inner.state = State::Evicting;
                shared.backend.unload();
                inner.evict_requested = false;
                inner.state = State::Unloaded;
            }
        } else {
            return;
        }
        last_completed = Instant::now();
    }
}
