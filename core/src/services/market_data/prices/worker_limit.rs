use std::sync::Mutex;

pub(super) struct WorkerLimit {
    state: Mutex<State>,
}

struct State {
    limit: usize,
    active: usize,
}

pub(super) struct WorkerPermit<'a> {
    owner: &'a WorkerLimit,
}

impl WorkerLimit {
    pub(super) fn new(limit: usize) -> Self {
        Self {
            state: Mutex::new(State { limit, active: 0 }),
        }
    }

    pub(super) fn set_limit(&self, limit: usize) {
        self.state
            .lock()
            .expect("Worker admission lock poisoned")
            .limit = limit;
    }

    pub(super) fn try_acquire(&self) -> Option<WorkerPermit<'_>> {
        let mut state = self.state.lock().expect("Worker admission lock poisoned");
        if state.active >= state.limit {
            return None;
        }
        state.active += 1;
        Some(WorkerPermit { owner: self })
    }
}

impl Drop for WorkerPermit<'_> {
    fn drop(&mut self) {
        self.owner
            .state
            .lock()
            .expect("Worker admission lock poisoned")
            .active -= 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shrinking_drains_existing_jobs_before_admitting_replacements_and_growing_is_live() {
        let limit = WorkerLimit::new(2);
        let first = limit.try_acquire().unwrap();
        let second = limit.try_acquire().unwrap();
        assert!(limit.try_acquire().is_none());
        limit.set_limit(1);
        drop(first);
        assert!(limit.try_acquire().is_none());
        drop(second);
        let first = limit.try_acquire().unwrap();
        assert!(limit.try_acquire().is_none());
        limit.set_limit(3);
        let second = limit.try_acquire().unwrap();
        let third = limit.try_acquire().unwrap();
        assert!(limit.try_acquire().is_none());
        limit.set_limit(1);
        drop(second);
        drop(third);
        assert!(limit.try_acquire().is_none());
        drop(first);
        let _replacement = limit.try_acquire().unwrap();
        assert!(limit.try_acquire().is_none());
    }

    #[test]
    fn simultaneous_claims_obey_one_shared_process_limit() {
        use std::sync::{
            atomic::{AtomicUsize, Ordering},
            Barrier,
        };
        let limit = WorkerLimit::new(2);
        let start = Barrier::new(7);
        let attempted = Barrier::new(7);
        let admitted = AtomicUsize::new(0);
        std::thread::scope(|scope| {
            for _ in 0..6 {
                scope.spawn(|| {
                    start.wait();
                    let permit = limit.try_acquire();
                    if permit.is_some() {
                        admitted.fetch_add(1, Ordering::SeqCst);
                    }
                    attempted.wait();
                    drop(permit);
                });
            }
            start.wait();
            attempted.wait();
        });
        assert_eq!(admitted.load(Ordering::SeqCst), 2);
        let _replacement = limit.try_acquire().unwrap();
    }

    #[tokio::test]
    async fn cancelling_a_consumer_returns_capacity() {
        let limit = std::sync::Arc::new(WorkerLimit::new(1));
        let entered = std::sync::Arc::new(tokio::sync::Notify::new());
        let task_limit = limit.clone();
        let task_entered = entered.clone();
        let task = tokio::spawn(async move {
            let _permit = task_limit.try_acquire().unwrap();
            task_entered.notify_one();
            std::future::pending::<()>().await;
        });
        entered.notified().await;
        assert!(limit.try_acquire().is_none());
        task.abort();
        assert!(task.await.unwrap_err().is_cancelled());
        let _replacement = limit.try_acquire().unwrap();
    }
}
