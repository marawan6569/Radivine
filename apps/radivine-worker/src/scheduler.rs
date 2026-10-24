//! Round-robin multi-job scheduler, subscription event hub (`<= 10 Hz` coalescing),
//! and per-item watchdog thread (`JOB_SYSTEM_DESIGN.md` §5.4, §6, `AC-C2.11`, `AC-C2.12`, `AC-C2.13`).

use std::collections::HashMap;
use std::env;
use std::io::Write;
use std::os::unix::net::UnixStream;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use radivine_convert::Stage;
use radivine_jobs::{
    ItemState, JobId, JobProgressCounts, JobState, JobSummary, JobUpdate, PROTOCOL_VERSION,
};
use tracing::{info, warn};

use crate::fence::{InFlightItemSlot, TimeoutMarkOutcome};
use crate::governor::ResourceGovernor;
use crate::handler::{ItemExecutionOutcome, JobHandler, PreparedJob};
use crate::notify::notify_job_completed;
use crate::store::{FailedOutcome, JobStore, SucceededOutcome, now_unix_ms};

struct Subscriber {
    job_filter: Option<JobId>,
    stream: UnixStream,
}

/// Thread-safe event hub broadcasting [`JobUpdate`] frames to subscribed clients with
/// `<= 10 Hz` (`100 ms`) progress coalescing and non-blocking slow-client eviction (`AC-C2.13`).
pub struct EventHub {
    subscribers: Mutex<Vec<Subscriber>>,
    last_progress_emit: Mutex<HashMap<(JobId, u32, ItemState), Instant>>,
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new()
    }
}

impl EventHub {
    #[must_use]
    pub fn new() -> Self {
        Self {
            subscribers: Mutex::new(Vec::new()),
            last_progress_emit: Mutex::new(HashMap::new()),
        }
    }

    /// Registers a new subscribed client stream with a 250 ms write timeout.
    pub fn add_subscriber(&self, job_filter: Option<JobId>, stream: UnixStream) {
        let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
        self.subscribers
            .lock()
            .unwrap()
            .push(Subscriber { job_filter, stream });
    }

    #[must_use]
    pub fn subscriber_count(&self) -> usize {
        use std::io::Read;
        let mut subs = self.subscribers.lock().unwrap();
        subs.retain_mut(|sub| {
            let _ = sub.stream.set_nonblocking(true);
            let mut buf = [0u8; 1];
            let alive = match sub.stream.read(&mut buf) {
                Ok(0) => false, // EOF: client closed connection
                Ok(_) => true,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => true,
                Err(_) => false,
            };
            let _ = sub.stream.set_nonblocking(false);
            alive
        });
        subs.len()
    }

    /// Broadcasts a [`JobUpdate`] to all matching subscribers, evicting any slow or disconnected stream.
    pub fn publish(&self, event: &JobUpdate) {
        let target_job_id = match event {
            JobUpdate::JobCreated { job, .. } | JobUpdate::JobFinished { job, .. } => job.job_id,
            JobUpdate::JobStateChanged { job_id, .. }
            | JobUpdate::ItemProgress { job_id, .. }
            | JobUpdate::ItemCompleted { job_id, .. } => *job_id,
        };

        // Coalesce ItemProgress events to at most 10 Hz (100 ms per (job_id, item_index, state))
        if let JobUpdate::ItemProgress {
            job_id,
            item_index,
            state,
            ..
        } = event
        {
            let now = Instant::now();
            let mut map = self.last_progress_emit.lock().unwrap();
            let key = (*job_id, *item_index, *state);
            if let Some(&prev) = map.get(&key)
                && now.duration_since(prev) < Duration::from_millis(100)
            {
                return;
            }
            map.insert(key, now);
        }

        if let JobUpdate::JobFinished { job, .. } = event {
            self.last_progress_emit
                .lock()
                .unwrap()
                .retain(|(jid, _, _), _| *jid != job.job_id);
        }

        let Ok(mut line) = serde_json::to_string(event) else {
            return;
        };
        line.push('\n');
        let bytes = line.as_bytes();

        let mut subs = self.subscribers.lock().unwrap();
        subs.retain_mut(|sub| {
            if let Some(filter_id) = sub.job_filter
                && filter_id != target_job_id
            {
                return true;
            }
            sub.stream.write_all(bytes).is_ok() && sub.stream.flush().is_ok()
        });
    }
}

struct SchedulerState {
    rr_cursor: usize,
    shutting_down: bool,
    recycling_for_timeout: bool,
    in_flight_slots: HashMap<i64, Arc<InFlightItemSlot>>,
    job_cancel_flags: HashMap<JobId, Arc<AtomicBool>>,
    prepared_jobs: HashMap<JobId, Arc<PreparedJob>>,
    consecutive_dest_errors: HashMap<JobId, u32>,
}

/// Multi-threaded round-robin job scheduler and watchdog coordinator.
pub struct JobScheduler {
    store: Arc<JobStore>,
    governor: ResourceGovernor,
    handlers: HashMap<String, Arc<dyn JobHandler>>,
    events: Arc<EventHub>,
    inner: Arc<(Mutex<SchedulerState>, Condvar)>,
    last_activity_ms: Arc<AtomicU64>,
    recycle_spawn_args: Option<Vec<String>>,
    worker_threads: Mutex<Vec<JoinHandle<()>>>,
    watchdog_thread: Mutex<Option<JoinHandle<()>>>,
}

impl JobScheduler {
    /// Creates and starts the scheduler worker pool (`max_parallel` threads) and watchdog thread.
    pub fn start(
        store: Arc<JobStore>,
        governor: ResourceGovernor,
        handlers: Vec<Arc<dyn JobHandler>>,
        events: Arc<EventHub>,
        max_parallel: usize,
        recycle_spawn_args: Option<Vec<String>>,
    ) -> Arc<Self> {
        let mut handler_map = HashMap::new();
        for h in handlers {
            handler_map.insert(h.kind().to_string(), h);
        }

        let now = now_unix_ms() as u64;
        let sched = Arc::new(Self {
            store,
            governor,
            handlers: handler_map,
            events,
            inner: Arc::new((
                Mutex::new(SchedulerState {
                    rr_cursor: 0,
                    shutting_down: false,
                    recycling_for_timeout: false,
                    in_flight_slots: HashMap::new(),
                    job_cancel_flags: HashMap::new(),
                    prepared_jobs: HashMap::new(),
                    consecutive_dest_errors: HashMap::new(),
                }),
                Condvar::new(),
            )),
            last_activity_ms: Arc::new(AtomicU64::new(now)),
            recycle_spawn_args,
            worker_threads: Mutex::new(Vec::new()),
            watchdog_thread: Mutex::new(None),
        });

        let mut threads = Vec::with_capacity(max_parallel.max(1));
        for idx in 0..max_parallel.max(1) {
            let s = sched.clone();
            let handle = thread::Builder::new()
                .name(format!("radivine-worker-{idx}"))
                .spawn(move || s.worker_loop())
                .expect("failed to spawn worker thread");
            threads.push(handle);
        }
        *sched.worker_threads.lock().unwrap() = threads;

        let s_wd = sched.clone();
        let wd_handle = thread::Builder::new()
            .name("radivine-watchdog".to_string())
            .spawn(move || s_wd.watchdog_loop())
            .expect("failed to spawn watchdog thread");
        *sched.watchdog_thread.lock().unwrap() = Some(wd_handle);

        sched
    }

    #[must_use]
    pub fn governor(&self) -> &ResourceGovernor {
        &self.governor
    }

    #[must_use]
    pub fn events(&self) -> &Arc<EventHub> {
        &self.events
    }

    #[must_use]
    pub fn has_handler(&self, kind: &str) -> bool {
        self.handlers.contains_key(kind)
    }

    /// Records client or worker activity and wakes idle worker threads.
    pub fn notify_work_available(&self) {
        self.touch_activity();
        let (_lock, cvar) = &*self.inner;
        cvar.notify_all();
    }

    /// Updates the last-activity timestamp (`now_unix_ms`).
    pub fn touch_activity(&self) {
        self.last_activity_ms
            .store(now_unix_ms() as u64, Ordering::Relaxed);
    }

    #[must_use]
    pub fn idle_duration(&self) -> Duration {
        let last = self.last_activity_ms.load(Ordering::Relaxed);
        let now = now_unix_ms() as u64;
        Duration::from_millis(now.saturating_sub(last))
    }

    #[must_use]
    pub fn in_flight_count(&self) -> usize {
        self.inner.0.lock().unwrap().in_flight_slots.len()
    }

    /// Cooperatively cancels a job (`< 100 ms` response, `AC-C2.9`): sets its in-memory cancel flag,
    /// marks all `Pending` items `Cancelled` in SQLite, wakes waiting threads, and broadcasts events.
    pub fn cancel_job(&self, job_id: JobId) -> Result<JobSummary, crate::store::StoreError> {
        self.touch_activity();
        {
            let mut st = self.inner.0.lock().unwrap();
            let flag = st
                .job_cancel_flags
                .entry(job_id)
                .or_insert_with(|| Arc::new(AtomicBool::new(true)));
            flag.store(true, Ordering::SeqCst);
            for slot in st.in_flight_slots.values() {
                if slot.job_id == job_id {
                    slot.cancel_flag.store(true, Ordering::SeqCst);
                }
            }
        }
        self.inner.1.notify_all();

        let summary = self.store.cancel_job(job_id)?;
        if summary.state.is_terminal() {
            self.cleanup_job_state(job_id);
            self.events.publish(&JobUpdate::JobFinished {
                v: PROTOCOL_VERSION,
                job: Box::new(summary.clone()),
            });
        } else {
            self.events.publish(&JobUpdate::JobStateChanged {
                v: PROTOCOL_VERSION,
                job_id,
                state: summary.state,
                summary: summary.counts,
            });
        }
        Ok(summary)
    }

    /// Initiates graceful shutdown (`SIGTERM` / idle exit), draining in-flight items for up to `drain_timeout`.
    pub fn shutdown_gracefully(&self, drain_timeout: Duration) {
        {
            let (lock, cvar) = &*self.inner;
            let mut st = lock.lock().unwrap();
            st.shutting_down = true;
            for flag in st.job_cancel_flags.values() {
                flag.store(true, Ordering::SeqCst);
            }
            for slot in st.in_flight_slots.values() {
                slot.cancel_flag.store(true, Ordering::SeqCst);
            }
            cvar.notify_all();
        }
        self.governor.begin_drain();

        let deadline = Instant::now() + drain_timeout;
        while Instant::now() < deadline {
            if self.in_flight_count() == 0 {
                break;
            }
            thread::sleep(Duration::from_millis(20));
        }

        if self.in_flight_count() > 0 {
            let _ = self
                .store
                .mark_inflight_items_interrupted_on_shutdown("Worker stopped via SIGTERM");
        } else {
            let mut threads = self.worker_threads.lock().unwrap();
            for h in threads.drain(..) {
                let _ = h.join();
            }
        }
        if let Some(wd) = self.watchdog_thread.lock().unwrap().take() {
            let _ = wd.join();
        }
    }

    fn cleanup_job_state(&self, job_id: JobId) {
        let mut st = self.inner.0.lock().unwrap();
        st.prepared_jobs.remove(&job_id);
        st.job_cancel_flags.remove(&job_id);
        st.consecutive_dest_errors.remove(&job_id);
    }

    fn worker_loop(&self) {
        loop {
            if self.is_stopping() {
                return;
            }

            let Some((job, prepared, claimed, slot)) = self.claim_next_round_robin() else {
                let (lock, cvar) = &*self.inner;
                let st = lock.lock().unwrap();
                if st.shutting_down || st.recycling_for_timeout {
                    return;
                }
                let _ = cvar.wait_timeout(st, Duration::from_millis(50)).unwrap();
                continue;
            };

            self.touch_activity();

            let Some(handler) = self.handlers.get(&job.kind).cloned() else {
                self.finish_missing_handler(&job, &claimed, &slot);
                continue;
            };

            let events_ref = &self.events;
            let job_id = job.job_id;
            let item_index = claimed.item_index;
            let mut stage_cb = |item_state: ItemState, stage: Stage, summary: JobProgressCounts| {
                let stage_name = match stage {
                    Stage::Inspecting => "inspecting",
                    Stage::Decoding => "decoding",
                    Stage::Transforming => "transforming",
                    Stage::Encoding => "encoding",
                    Stage::ValidatingOutput => "validating_output",
                    Stage::Committing => "committing",
                };
                events_ref.publish(&JobUpdate::ItemProgress {
                    v: PROTOCOL_VERSION,
                    job_id,
                    item_index,
                    state: item_state,
                    stage: Some(stage_name.to_string()),
                    summary,
                });
            };

            let outcome = handler.execute_item(
                &prepared,
                &claimed,
                &slot,
                &self.store,
                &self.governor,
                &mut stage_cb,
            );

            self.finalize_item_outcome(job_id, &claimed, &slot, outcome);
            self.touch_activity();
        }
    }

    fn is_stopping(&self) -> bool {
        let st = self.inner.0.lock().unwrap();
        st.shutting_down || st.recycling_for_timeout
    }

    /// Selects the next active job in round-robin order (`AC-C2.11`), preparing `Queued` jobs
    /// and claiming the next `Pending` item.
    fn claim_next_round_robin(
        &self,
    ) -> Option<(
        JobSummary,
        Arc<PreparedJob>,
        crate::store::ClaimedItem,
        Arc<InFlightItemSlot>,
    )> {
        let active_jobs = self.store.list_active_jobs().ok()?;
        if active_jobs.is_empty() {
            return None;
        }

        let len = active_jobs.len();
        let start_cursor = {
            let st = self.inner.0.lock().unwrap();
            if st.shutting_down || st.recycling_for_timeout {
                return None;
            }
            st.rr_cursor % len
        };

        for offset in 0..len {
            let idx = (start_cursor + offset) % len;
            let mut job = active_jobs[idx].clone();

            if job.state == JobState::Queued || job.state == JobState::Preparing {
                match self.prepare_job_if_needed(&job) {
                    Ok(Some(updated_job)) => {
                        job = updated_job;
                    }
                    Ok(None) => continue,
                    Err(()) => continue,
                }
            }

            if job.state != JobState::Running {
                continue;
            }

            let prepared = match self.ensure_prepared_cached(&job) {
                Ok(p) => p,
                Err(()) => continue,
            };

            if let Ok(Some(claimed)) = self.store.claim_next_pending_item_for_job(job.job_id) {
                let mut st = self.inner.0.lock().unwrap();
                st.rr_cursor = (idx + 1) % len.max(1);
                let cancel_flag = st
                    .job_cancel_flags
                    .entry(job.job_id)
                    .or_insert_with(|| Arc::new(AtomicBool::new(false)))
                    .clone();
                let slot = InFlightItemSlot::new(
                    claimed.item_id,
                    job.job_id,
                    claimed.item_index,
                    prepared.per_item_timeout,
                    cancel_flag,
                );
                st.in_flight_slots.insert(claimed.item_id, slot.clone());
                return Some((job, prepared, claimed, slot));
            }
        }

        None
    }

    fn prepare_job_if_needed(&self, job: &JobSummary) -> Result<Option<JobSummary>, ()> {
        let Some(handler) = self.handlers.get(&job.kind).cloned() else {
            let _ = self.store.fail_job_in_preparing(
                job.job_id,
                "unknown_job_kind",
                "invalid_options",
                &format!("unknown job kind '{}'", job.kind),
            );
            if let Ok(Some(failed_job)) = self.store.get_job_summary(job.job_id) {
                self.events.publish(&JobUpdate::JobFinished {
                    v: PROTOCOL_VERSION,
                    job: Box::new(failed_job.clone()),
                });
                notify_job_completed(&failed_job);
            }
            return Err(());
        };

        if job.state == JobState::Queued
            && let Ok(Some(prep_summary)) = self.store.mark_job_preparing(job.job_id)
        {
            self.events.publish(&JobUpdate::JobStateChanged {
                v: PROTOCOL_VERSION,
                job_id: job.job_id,
                state: JobState::Preparing,
                summary: prep_summary.counts,
            });
        }

        match handler.prepare(job) {
            Ok(prepared) => {
                let prepared_arc = Arc::new(prepared);
                if let Ok(Some(running_job)) = self
                    .store
                    .mark_job_running(job.job_id, &prepared_arc.resolved_json)
                {
                    self.inner
                        .0
                        .lock()
                        .unwrap()
                        .prepared_jobs
                        .insert(job.job_id, prepared_arc);
                    self.events.publish(&JobUpdate::JobStateChanged {
                        v: PROTOCOL_VERSION,
                        job_id: job.job_id,
                        state: JobState::Running,
                        summary: running_job.counts,
                    });
                    return Ok(Some(running_job));
                }
                self.store.get_job_summary(job.job_id).map_err(|_| ())
            }
            Err(prep_err) => {
                if let Ok(Some(failed_job)) = self.store.fail_job_in_preparing(
                    job.job_id,
                    &prep_err.code,
                    &prep_err.class,
                    &prep_err.message,
                ) {
                    self.cleanup_job_state(job.job_id);
                    self.events.publish(&JobUpdate::JobFinished {
                        v: PROTOCOL_VERSION,
                        job: Box::new(failed_job.clone()),
                    });
                    notify_job_completed(&failed_job);
                }
                Err(())
            }
        }
    }

    fn ensure_prepared_cached(&self, job: &JobSummary) -> Result<Arc<PreparedJob>, ()> {
        {
            let st = self.inner.0.lock().unwrap();
            if let Some(p) = st.prepared_jobs.get(&job.job_id) {
                return Ok(p.clone());
            }
        }
        let Some(handler) = self.handlers.get(&job.kind).cloned() else {
            return Err(());
        };
        let prepared = Arc::new(handler.prepare(job).map_err(|_| ())?);
        self.inner
            .0
            .lock()
            .unwrap()
            .prepared_jobs
            .insert(job.job_id, prepared.clone());
        Ok(prepared)
    }

    fn finish_missing_handler(
        &self,
        job: &JobSummary,
        claimed: &crate::store::ClaimedItem,
        slot: &Arc<InFlightItemSlot>,
    ) {
        let _ = slot.try_mark_finished();
        self.inner
            .0
            .lock()
            .unwrap()
            .in_flight_slots
            .remove(&claimed.item_id);
        let _ = self.store.complete_item_failed(
            job.job_id,
            claimed.item_id,
            &FailedOutcome {
                error_code: "unknown_job_kind",
                error_class: "invalid_options",
                error_message: "unknown job kind",
                retryable: false,
                duration_ms: 0,
                warnings_json: "[]",
            },
            false,
        );
    }

    fn finalize_item_outcome(
        &self,
        job_id: JobId,
        claimed: &crate::store::ClaimedItem,
        slot: &Arc<InFlightItemSlot>,
        outcome: ItemExecutionOutcome,
    ) {
        let won_finish = slot.try_mark_finished();
        {
            let mut st = self.inner.0.lock().unwrap();
            st.in_flight_slots.remove(&claimed.item_id);
        }

        match outcome {
            ItemExecutionOutcome::Succeeded {
                output_path,
                source_bytes,
                output_bytes,
                duration_ms,
                warnings_json,
            } => {
                // Note: even if a timeout occurred AFTER InCommit started (DeferredInCommit),
                // if commit_no_replace succeeded on the active probe, the file is published on
                // disk and MUST be recorded as Succeeded!
                self.reset_consecutive_dest_errors(job_id);
                if let Ok((item_rep, job_sum, finished)) = self.store.complete_item_succeeded(
                    job_id,
                    claimed.item_id,
                    &SucceededOutcome {
                        output_path: &output_path,
                        source_bytes,
                        output_bytes,
                        duration_ms,
                        warnings_json: &warnings_json,
                    },
                ) {
                    self.publish_item_and_job_events(item_rep, job_sum, finished);
                }
            }
            ItemExecutionOutcome::Skipped {
                reason_code,
                reason_message,
                warnings_json,
            } => {
                if !won_finish {
                    self.record_timeout_failure_if_needed(slot);
                    return;
                }
                self.reset_consecutive_dest_errors(job_id);
                if let Ok((item_rep, job_sum, finished)) = self.store.complete_item_skipped(
                    job_id,
                    claimed.item_id,
                    &reason_code,
                    &reason_message,
                    &warnings_json,
                ) {
                    self.publish_item_and_job_events(item_rep, job_sum, finished);
                }
            }
            ItemExecutionOutcome::Cancelled { duration_ms } => {
                if !won_finish {
                    self.record_timeout_failure_if_needed(slot);
                    return;
                }
                let is_shutting_down = self.inner.0.lock().unwrap().shutting_down;
                let res = if is_shutting_down {
                    self.store.complete_item_interrupted(
                        job_id,
                        claimed.item_id,
                        duration_ms,
                        "Worker stopped via SIGTERM",
                    )
                } else {
                    self.store
                        .complete_item_cancelled(job_id, claimed.item_id, duration_ms)
                };
                if let Ok((item_rep, job_sum, finished)) = res {
                    self.publish_item_and_job_events(item_rep, job_sum, finished);
                }
            }
            ItemExecutionOutcome::Failed {
                error_code,
                error_class,
                error_message,
                retryable,
                duration_ms,
                warnings_json,
                timed_out,
                is_dest_space_or_perm_error,
            } => {
                if timed_out || !won_finish {
                    self.record_timeout_failure_if_needed(slot);
                    return;
                }
                if let Ok((item_rep, mut job_sum, mut finished)) = self.store.complete_item_failed(
                    job_id,
                    claimed.item_id,
                    &FailedOutcome {
                        error_code: &error_code,
                        error_class: &error_class,
                        error_message: &error_message,
                        retryable,
                        duration_ms,
                        warnings_json: &warnings_json,
                    },
                    false,
                ) {
                    if is_dest_space_or_perm_error && !finished {
                        let count = self.increment_consecutive_dest_errors(job_id);
                        if count >= 3
                            && let Ok(Some(aborted_sum)) = self.store.fail_remaining_pending_items(
                                job_id,
                                &error_code,
                                &error_class,
                                &error_message,
                            )
                        {
                            job_sum = aborted_sum;
                            finished = job_sum.state.is_terminal();
                        }
                    } else if !is_dest_space_or_perm_error {
                        self.reset_consecutive_dest_errors(job_id);
                    }
                    self.publish_item_and_job_events(item_rep, job_sum, finished);
                }
            }
        }
    }

    fn reset_consecutive_dest_errors(&self, job_id: JobId) {
        self.inner
            .0
            .lock()
            .unwrap()
            .consecutive_dest_errors
            .insert(job_id, 0);
    }

    fn increment_consecutive_dest_errors(&self, job_id: JobId) -> u32 {
        let mut st = self.inner.0.lock().unwrap();
        let entry = st.consecutive_dest_errors.entry(job_id).or_insert(0);
        *entry += 1;
        *entry
    }

    fn publish_item_and_job_events(
        &self,
        item_rep: radivine_jobs::JobItemReport,
        job_sum: JobSummary,
        finished: bool,
    ) {
        let job_id = job_sum.job_id;
        self.events.publish(&JobUpdate::ItemCompleted {
            v: PROTOCOL_VERSION,
            job_id,
            item: Box::new(item_rep),
            summary: job_sum.counts,
        });
        if finished {
            self.cleanup_job_state(job_id);
            self.events.publish(&JobUpdate::JobFinished {
                v: PROTOCOL_VERSION,
                job: Box::new(job_sum.clone()),
            });
            notify_job_completed(&job_sum);
        }
    }

    fn record_timeout_failure_if_needed(&self, slot: &Arc<InFlightItemSlot>) {
        if !slot.claim_timeout_db_record() {
            return;
        }
        let duration_ms = slot.started_at.elapsed().as_millis() as u64;
        if let Ok((item_rep, job_sum, finished)) = self.store.complete_item_failed(
            slot.job_id,
            slot.item_id,
            &FailedOutcome {
                error_code: "timed_out",
                error_class: "limit_exceeded",
                error_message: "Conversion exceeded the per-file time limit",
                retryable: false,
                duration_ms,
                warnings_json: "[]",
            },
            true,
        ) {
            self.publish_item_and_job_events(item_rep, job_sum, finished);
        }
    }

    fn watchdog_loop(&self) {
        let poll_interval = Duration::from_millis(50);
        loop {
            thread::sleep(poll_interval);
            let slots: Vec<Arc<InFlightItemSlot>> = {
                let st = self.inner.0.lock().unwrap();
                if st.shutting_down {
                    return;
                }
                st.in_flight_slots.values().cloned().collect()
            };

            let mut triggered_slot: Option<Arc<InFlightItemSlot>> = None;
            for slot in slots {
                if slot.started_at.elapsed() > slot.timeout {
                    let (outcome, _wait) = slot.try_mark_timeout();
                    if outcome == TimeoutMarkOutcome::MarkedTimedOut {
                        self.record_timeout_failure_if_needed(&slot);
                        triggered_slot = Some(slot);
                        break;
                    }
                }
            }

            if let Some(timed_out_slot) = triggered_slot {
                self.recycle_worker_after_timeout(timed_out_slot.item_id);
                return;
            }
        }
    }

    fn recycle_worker_after_timeout(&self, timed_out_item_id: i64) {
        warn!(
            item_id = timed_out_item_id,
            "per-item watchdog timeout triggered; draining healthy items and recycling worker (exit 75)"
        );
        {
            let (lock, cvar) = &*self.inner;
            let mut st = lock.lock().unwrap();
            st.recycling_for_timeout = true;
            cvar.notify_all();
        }
        self.governor.begin_drain();

        let drain_ms = env::var("RADIVINE_WORKER_RECYCLE_DRAIN_MS")
            .ok()
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(5_000);
        let deadline = Instant::now() + Duration::from_millis(drain_ms);

        // Wait up to drain_ms for any OTHER healthy in-flight items to finish and commit
        while Instant::now() < deadline {
            let other_inflight = {
                let st = self.inner.0.lock().unwrap();
                st.in_flight_slots
                    .keys()
                    .filter(|&&id| id != timed_out_item_id)
                    .count()
            };
            if other_inflight == 0 {
                break;
            }
            thread::sleep(Duration::from_millis(25));
        }

        let _ = self.store.wal_checkpoint_truncate();

        // If active jobs with pending items remain and replacement spawning is enabled,
        // spawn a replacement worker process that will wait for our worker.lock to release on exit(75).
        if env::var("RADIVINE_WORKER_DISABLE_RECYCLE_SPAWN")
            .ok()
            .as_deref()
            != Some("1")
            && let Some(ref args) = self.recycle_spawn_args
            && let Ok((active_jobs, _)) = self.store.active_counts()
            && active_jobs > 0
            && let Ok(exe) = env::current_exe()
        {
            use std::os::unix::process::CommandExt;
            use std::process::{Command, Stdio};
            let mut cmd = Command::new(exe);
            cmd.args(args)
                .env_remove("RADIVINE_WORKER_TEST_CRASH_AT")
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .process_group(0);
            let _ = cmd.spawn();
            info!("spawned replacement worker prior to exit(75)");
        }

        std::process::exit(75);
    }
}
