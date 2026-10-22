//! Background worker job dispatcher (`ACTION_SYSTEM.md` §7).

use std::collections::{HashMap, VecDeque};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use crate::action::{Action, ActionError, ActionId, JobHandle, JobId, Outcome};
use crate::context::Context;

/// Callback sink invoked when a background job completes (`ACTION_SYSTEM.md` §7).
pub type JobSink = Box<dyn Fn(JobEvent) + Send + Sync>;

/// Event emitted by [`Dispatcher`] when a worker thread finishes (`ACTION_SYSTEM.md` §7).
#[derive(Debug)]
pub enum JobEvent {
    /// A dispatched action finished (or failed / panicked).
    Finished {
        /// Unique job identifier.
        job: JobId,
        /// Identifier of the executed action.
        action: ActionId,
        /// Outcome or error produced by `Action::run`.
        result: Result<Outcome, ActionError>,
    },
}

struct PendingJob {
    id: JobId,
    action: Arc<dyn Action>,
    ctx: Arc<Context>,
    cancel_flag: Arc<AtomicBool>,
}

struct DispatcherState {
    next_id: u64,
    max_concurrent: usize,
    queue: VecDeque<PendingJob>,
    running: HashMap<JobId, Arc<AtomicBool>>,
}

/// Asynchronous action dispatcher with bounded concurrency (`<= max_concurrent`) and panic isolation
/// (`ACTION_SYSTEM.md` §7).
pub struct Dispatcher {
    sink: Arc<JobSink>,
    state: Arc<Mutex<DispatcherState>>,
}

impl Dispatcher {
    /// Creates a new [`Dispatcher`] delivering completion events to `sink` and running at most
    /// `max_concurrent` worker threads simultaneously (clamped to `1..=16`, default `4`).
    #[must_use]
    pub fn new(sink: JobSink, max_concurrent: usize) -> Self {
        let cap = max_concurrent.clamp(1, 16);
        Self {
            sink: Arc::new(sink),
            state: Arc::new(Mutex::new(DispatcherState {
                next_id: 1,
                max_concurrent: cap,
                queue: VecDeque::new(),
                running: HashMap::new(),
            })),
        }
    }

    /// Enqueues or immediately spawns `action` on `ctx` and returns its [`JobId`].
    pub fn dispatch(&mut self, action: Arc<dyn Action>, ctx: Arc<Context>) -> JobId {
        let (job_id, to_spawn) = {
            let mut guard = self
                .state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let id = JobId(guard.next_id);
            guard.next_id += 1;
            let cancel_flag = Arc::new(AtomicBool::new(false));
            let pending = PendingJob {
                id,
                action,
                ctx,
                cancel_flag: cancel_flag.clone(),
            };

            if guard.running.len() < guard.max_concurrent {
                guard.running.insert(id, cancel_flag);
                (id, Some(pending))
            } else {
                guard.queue.push_back(pending);
                (id, None)
            }
        };

        if let Some(job) = to_spawn {
            spawn_worker(job, self.sink.clone(), self.state.clone());
        }

        job_id
    }

    /// Requests cooperative cancellation of `job` (or removes it from the pending queue).
    pub fn cancel(&mut self, job: JobId) {
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(flag) = guard.running.get(&job) {
            flag.store(true, Ordering::Relaxed);
        }
        for pending in &guard.queue {
            if pending.id == job {
                pending.cancel_flag.store(true, Ordering::Relaxed);
            }
        }
        guard.queue.retain(|p| p.id != job);
    }

    /// Cancels all queued and currently running jobs (`ACTION_SYSTEM.md` §7).
    pub fn cancel_all(&mut self) {
        let mut guard = self
            .state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for pending in guard.queue.drain(..) {
            pending.cancel_flag.store(true, Ordering::Relaxed);
        }
        for flag in guard.running.values() {
            flag.store(true, Ordering::Relaxed);
        }
    }
}

fn spawn_worker(job: PendingJob, sink: Arc<JobSink>, state: Arc<Mutex<DispatcherState>>) {
    let job_id = job.id;
    let action_id = job.action.meta().id.clone();
    let thread_name = format!("radivine-job-{}", job_id.0);
    let sink_clone = sink.clone();
    let state_clone = state.clone();

    let spawn_res = std::thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            run_job_loop(job, sink_clone, state_clone);
        });

    if let Err(err) = spawn_res {
        // If thread creation fails, remove from running and report error through sink
        {
            let mut guard = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.running.remove(&job_id);
        }
        (sink)(JobEvent::Finished {
            job: job_id,
            action: action_id,
            result: Err(ActionError::Io(err)),
        });
    }
}

fn run_job_loop(first_job: PendingJob, sink: Arc<JobSink>, state: Arc<Mutex<DispatcherState>>) {
    let mut current = Some(first_job);

    while let Some(job) = current.take() {
        let action_id = job.action.meta().id.clone();
        let handle = JobHandle::new(job.id, job.cancel_flag.clone());

        let result = if handle.is_cancelled() {
            Err(ActionError::Cancelled)
        } else {
            match catch_unwind(AssertUnwindSafe(|| job.action.run(&job.ctx, &handle))) {
                Ok(res) => res,
                Err(_) => {
                    tracing::error!(
                        job_id = job.id.0,
                        action_id = %action_id,
                        "Action panicked during execution"
                    );
                    Err(ActionError::Other("internal error".to_string()))
                }
            }
        };

        // Transition: remove finished job and check if a queued job can run next on this worker
        {
            let mut guard = state
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            guard.running.remove(&job.id);
            while let Some(next) = guard.queue.pop_front() {
                if next.cancel_flag.load(Ordering::Relaxed) {
                    continue;
                }
                guard.running.insert(next.id, next.cancel_flag.clone());
                current = Some(next);
                break;
            }
        }

        (sink)(JobEvent::Finished {
            job: job.id,
            action: action_id,
            result,
        });
    }
}
