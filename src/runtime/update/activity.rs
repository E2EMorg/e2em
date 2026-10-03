//! Admission and maintenance share a lock, closing the idle-check/admission race.
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub struct Activity {
    inner: Mutex<Inner>,
}
struct Inner {
    active: usize,
    last_completed: Instant,
    maintenance: bool,
}
impl Default for Activity {
    fn default() -> Self {
        Self {
            inner: Mutex::new(Inner {
                active: 0,
                last_completed: Instant::now(),
                maintenance: false,
            }),
        }
    }
}
impl Activity {
    pub fn enter(self: &Arc<Self>) -> Option<Work> {
        let mut inner = self.inner.lock().ok()?;
        if inner.maintenance {
            return None;
        }
        inner.active += 1;
        Some(Work(self.clone()))
    }
    pub fn idle_for(&self, duration: Duration) -> bool {
        self.inner.lock().is_ok_and(|i| {
            !i.maintenance && i.active == 0 && i.last_completed.elapsed() >= duration
        })
    }
    /// Once acquired, new work cannot enter. Used only for the final restart.
    pub fn begin_maintenance(&self, duration: Duration) -> bool {
        let Ok(mut inner) = self.inner.lock() else {
            return false;
        };
        if inner.maintenance || inner.active != 0 || inner.last_completed.elapsed() < duration {
            return false;
        }
        inner.maintenance = true;
        true
    }
}
pub struct Work(Arc<Activity>);
impl Drop for Work {
    fn drop(&mut self) {
        if let Ok(mut inner) = self.0.inner.lock() {
            inner.active -= 1;
            inner.last_completed = Instant::now();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn work_and_reply_must_finish_before_restart() {
        let activity = Arc::new(Activity::default());
        let job = activity.enter().unwrap();
        let reply = activity.enter().unwrap();
        assert!(!activity.begin_maintenance(Duration::ZERO));
        drop(job);
        assert!(!activity.begin_maintenance(Duration::ZERO));
        drop(reply);
        assert!(!activity.begin_maintenance(Duration::from_secs(60)));
        assert!(activity.begin_maintenance(Duration::ZERO));
        assert!(activity.enter().is_none());
    }
    #[test]
    fn admission_and_maintenance_are_exclusive() {
        for _ in 0..100 {
            let activity = Arc::new(Activity::default());
            let other = activity.clone();
            let barrier = Arc::new(std::sync::Barrier::new(2));
            let other_barrier = barrier.clone();
            let worker = std::thread::spawn(move || {
                other_barrier.wait();
                other.enter()
            });
            barrier.wait();
            let maintenance = activity.begin_maintenance(Duration::ZERO);
            let work = worker.join().unwrap();
            assert_ne!(maintenance, work.is_some());
        }
    }
}
