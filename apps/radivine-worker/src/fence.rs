//! Lock-free-I/O `Mutex<ItemPhase>` watchdog commit fence (`SECURITY_AND_FILE_SAFETY.md` §3.6, `JOB_SYSTEM_DESIGN.md` §6.4).
//!
//! Enforces the 3-step `acquire_commit_permit` protocol:
//! - **Step A**: `O(1)` in-memory transition to `JournalingCommit { attempt }` under `Mutex<ItemPhase>`.
//! - **Step B**: Unlocked `PRAGMA synchronous = FULL` SQLite `Committing` journal write.
//! - **Step C**: `O(1)` in-memory verification under `Mutex<ItemPhase>` that the watchdog did not
//!   mark the item `TimedOut` during Step B before transitioning to `InCommit` and issuing [`CommitPermit`].

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use radivine_convert::{
    CommitPermit, CommitRequest, ConversionError, FsOp, ItemControl, Limit, Reservation,
    ResourceEstimate, Stage, StageCreatedJournal, StageIntentJournal, TempCreatedJournal,
};
use radivine_jobs::{ItemState, JobId, JobProgressCounts};

use crate::governor::{ResourceGovernor, check_destination_free_space};
use crate::store::JobStore;

/// Per-item execution phase synchronized between the worker thread and the watchdog thread.
///
/// The mutex guarding [`ItemPhase`] is **never** held across SQLite or filesystem I/O.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemPhase {
    Running,
    JournalingCommit {
        attempt: u32,
    },
    InCommit {
        attempt: u32,
        timeout_observed: bool,
    },
    TimedOut,
    Finished,
}

/// Result of the watchdog polling an overdue [`InFlightItemSlot`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TimeoutMarkOutcome {
    /// Item was in `Running` or `JournalingCommit` (or `InCommit` after returning) and is now `TimedOut`.
    /// The caller should record `Failed(Timeout)` in SQLite and trigger worker recycling (`exit(75)`).
    MarkedTimedOut,
    /// Item is currently inside `commit_no_replace` (`InCommit`); `timeout_observed` was set to `true`.
    /// If the current probe succeeds, the item finishes as `Succeeded`; if it fails with `EEXIST`,
    /// the next probe will abort in Step A with `TimedOut`.
    DeferredInCommit,
    /// Item already finished or was already processed as `TimedOut`.
    AlreadyTerminal,
}

/// Shared state for a single in-flight item monitored by the watchdog thread.
pub struct InFlightItemSlot {
    pub item_id: i64,
    pub job_id: JobId,
    pub item_index: u32,
    pub started_at: Instant,
    pub timeout: Duration,
    pub cancel_flag: Arc<AtomicBool>,
    phase: Mutex<ItemPhase>,
    timeout_recorded_in_db: AtomicBool,
}

impl InFlightItemSlot {
    #[must_use]
    pub fn new(
        item_id: i64,
        job_id: JobId,
        item_index: u32,
        timeout: Duration,
        cancel_flag: Arc<AtomicBool>,
    ) -> Arc<Self> {
        Arc::new(Self {
            item_id,
            job_id,
            item_index,
            started_at: Instant::now(),
            timeout,
            cancel_flag,
            phase: Mutex::new(ItemPhase::Running),
            timeout_recorded_in_db: AtomicBool::new(false),
        })
    }

    #[must_use]
    pub fn phase(&self) -> ItemPhase {
        *self.phase.lock().unwrap()
    }

    /// Transitions the slot to `Finished` if it was not already `TimedOut`.
    ///
    /// Returns `true` if the worker thread won the race and should commit the normal item outcome,
    /// or `false` if the item was marked `TimedOut` by the watchdog.
    #[must_use]
    pub fn try_mark_finished(&self) -> bool {
        let mut guard = self.phase.lock().unwrap();
        match *guard {
            ItemPhase::TimedOut => false,
            _ => {
                *guard = ItemPhase::Finished;
                true
            }
        }
    }

    /// Atomically claims the right to write the `Failed(Timeout)` record to SQLite for this item.
    #[must_use]
    pub fn claim_timeout_db_record(&self) -> bool {
        !self.timeout_recorded_in_db.swap(true, Ordering::SeqCst)
    }

    /// Invoked by the watchdog thread when `started_at.elapsed() > timeout`.
    ///
    /// Acquires `Mutex<ItemPhase>` in `O(1)` time and returns `(TimeoutMarkOutcome, lock_wait_duration)`.
    pub fn try_mark_timeout(&self) -> (TimeoutMarkOutcome, Duration) {
        let t0 = Instant::now();
        let mut guard = self.phase.lock().unwrap();
        let lock_wait = t0.elapsed();

        let outcome = match *guard {
            ItemPhase::Running | ItemPhase::JournalingCommit { .. } => {
                *guard = ItemPhase::TimedOut;
                TimeoutMarkOutcome::MarkedTimedOut
            }
            ItemPhase::InCommit {
                attempt,
                timeout_observed: _,
            } => {
                *guard = ItemPhase::InCommit {
                    attempt,
                    timeout_observed: true,
                };
                TimeoutMarkOutcome::DeferredInCommit
            }
            ItemPhase::TimedOut => TimeoutMarkOutcome::MarkedTimedOut,
            ItemPhase::Finished => TimeoutMarkOutcome::AlreadyTerminal,
        };
        (outcome, lock_wait)
    }
}

/// Callback invoked when an item's stage changes (`Inspecting`, `Decoding`, etc.).
pub type StageCallback<'a> = &'a mut dyn FnMut(ItemState, Stage, JobProgressCounts);

/// Optional test hook invoked during Step B (unlocked SQLite `Committing` write).
pub type StepBHook<'a> = Option<&'a dyn Fn()>;

/// Concrete [`ItemControl`] implementation used by `radivine-worker` during item execution.
pub struct WorkerItemControl<'a> {
    pub store: &'a JobStore,
    pub governor: &'a ResourceGovernor,
    pub slot: &'a Arc<InFlightItemSlot>,
    pub dest_dir_for_precheck: &'a Path,
    pub on_stage_cb: StageCallback<'a>,
    pub step_b_hook: StepBHook<'a>,
}

impl WorkerItemControl<'_> {
    fn check_timeout_or_cancel(&self) -> Result<(), ConversionError> {
        if self.slot.cancel_flag.load(Ordering::Relaxed) || self.governor.is_draining() {
            return Err(ConversionError::Cancelled);
        }
        let phase = self.slot.phase();
        if phase == ItemPhase::TimedOut {
            return Err(ConversionError::LimitExceeded {
                limit: Limit::Timeout {
                    limit: self.slot.timeout,
                },
            });
        }
        let elapsed = self.slot.started_at.elapsed();
        if elapsed > self.slot.timeout {
            let (outcome, _) = self.slot.try_mark_timeout();
            if outcome == TimeoutMarkOutcome::MarkedTimedOut {
                return Err(ConversionError::LimitExceeded {
                    limit: Limit::Timeout {
                        limit: self.slot.timeout,
                    },
                });
            }
        }
        Ok(())
    }
}

impl ItemControl for WorkerItemControl<'_> {
    fn check_cancelled(&self) -> Result<(), ConversionError> {
        self.check_timeout_or_cancel()
    }

    fn on_stage(&mut self, stage: Stage) {
        let item_state = match stage {
            Stage::Inspecting => ItemState::Inspecting,
            Stage::Decoding | Stage::Transforming | Stage::Encoding | Stage::ValidatingOutput => {
                ItemState::Running
            }
            Stage::Committing => ItemState::Committing,
        };
        let stage_str = match stage {
            Stage::Inspecting => "inspecting",
            Stage::Decoding => "decoding",
            Stage::Transforming => "transforming",
            Stage::Encoding => "encoding",
            Stage::ValidatingOutput => "validating_output",
            Stage::Committing => "committing",
        };
        // Note: Stage::Committing DB update is performed with PRAGMA synchronous = FULL inside
        // acquire_commit_permit Step B so each candidate output path is journaled atomically.
        if stage != Stage::Committing
            && let Ok(counts) = self.store.update_item_stage(
                self.slot.job_id,
                self.slot.item_id,
                item_state,
                stage_str,
            )
        {
            (self.on_stage_cb)(item_state, stage, counts);
        }
    }

    fn reserve(&mut self, estimate: ResourceEstimate) -> Result<Reservation, ConversionError> {
        check_destination_free_space(self.dest_dir_for_precheck, estimate)?;
        let slot = self.slot;
        self.governor.reserve(estimate, &slot.cancel_flag, &|| {
            if slot.phase() == ItemPhase::TimedOut {
                return Some(slot.started_at.elapsed());
            }
            let el = slot.started_at.elapsed();
            if el > slot.timeout {
                let _ = slot.try_mark_timeout();
                Some(el)
            } else {
                None
            }
        })
    }

    fn record_stage_intent(
        &mut self,
        stage: &StageIntentJournal<'_>,
    ) -> Result<(), ConversionError> {
        self.check_timeout_or_cancel()?;
        self.store
            .record_stage_intent(self.slot.item_id, stage.stage_dir_path)
            .map_err(|e| ConversionError::Io {
                op: FsOp::CreateStageDir,
                path: stage.stage_dir_path.to_path_buf(),
                source: std::io::Error::other(e.to_string()),
            })
    }

    fn record_stage_created(
        &mut self,
        stage: &StageCreatedJournal<'_>,
    ) -> Result<(), ConversionError> {
        self.store
            .record_stage_created(
                self.slot.item_id,
                stage.stage_dir_path,
                stage.stage_dir_dev,
                stage.stage_dir_ino,
            )
            .map_err(|e| ConversionError::Io {
                op: FsOp::CreateStageDir,
                path: stage.stage_dir_path.to_path_buf(),
                source: std::io::Error::other(e.to_string()),
            })?;
        self.check_timeout_or_cancel()
    }

    fn record_temp_created(
        &mut self,
        temp: &TempCreatedJournal<'_>,
    ) -> Result<(), ConversionError> {
        self.store
            .record_temp_created(
                self.slot.item_id,
                temp.stage_dir_path,
                temp.temp_path,
                temp.temp_dev,
                temp.temp_ino,
            )
            .map_err(|e| ConversionError::Io {
                op: FsOp::CreateTemp,
                path: temp.temp_path.to_path_buf(),
                source: std::io::Error::other(e.to_string()),
            })?;
        self.check_timeout_or_cancel()
    }

    fn acquire_commit_permit<'a>(
        &mut self,
        req: &CommitRequest<'a>,
    ) -> Result<CommitPermit<'a>, ConversionError> {
        // Step A (O(1), no I/O): lock ItemPhase, check cancellation/timeout, set JournalingCommit { attempt }
        {
            let mut phase = self.slot.phase.lock().unwrap();
            if self.slot.cancel_flag.load(Ordering::Relaxed) || self.governor.is_draining() {
                *phase = ItemPhase::Finished;
                return Err(ConversionError::Cancelled);
            }
            match *phase {
                ItemPhase::Running
                | ItemPhase::InCommit {
                    timeout_observed: false,
                    ..
                } => {
                    *phase = ItemPhase::JournalingCommit {
                        attempt: req.attempt(),
                    };
                }
                ItemPhase::TimedOut
                | ItemPhase::InCommit {
                    timeout_observed: true,
                    ..
                } => {
                    *phase = ItemPhase::TimedOut;
                    return Err(ConversionError::LimitExceeded {
                        limit: Limit::Timeout {
                            limit: self.slot.timeout,
                        },
                    });
                }
                ItemPhase::JournalingCommit { .. } | ItemPhase::Finished => {
                    return Err(ConversionError::Cancelled);
                }
            }
        } // Mutex<ItemPhase> is dropped here BEFORE any SQLite I/O!

        // Step B (unlocked I/O): commit state = 'committing' + candidate output_path under PRAGMA synchronous = FULL
        if let Some(hook) = self.step_b_hook {
            hook();
        }
        self.store
            .record_committing_candidate(
                self.slot.item_id,
                req.final_output_path,
                req.temp_dev(),
                req.temp_ino(),
            )
            .map_err(|e| ConversionError::Io {
                op: FsOp::CommitOutput,
                path: req.final_output_path.to_path_buf(),
                source: std::io::Error::other(e.to_string()),
            })?;

        // Step C (O(1), no I/O): re-lock ItemPhase, verify watchdog did not mark TimedOut during Step B
        {
            let mut phase = self.slot.phase.lock().unwrap();
            if self.slot.cancel_flag.load(Ordering::Relaxed) || self.governor.is_draining() {
                *phase = ItemPhase::Finished;
                return Err(ConversionError::Cancelled);
            }
            match *phase {
                ItemPhase::JournalingCommit { attempt } if attempt == req.attempt() => {
                    *phase = ItemPhase::InCommit {
                        attempt,
                        timeout_observed: false,
                    };
                    Ok(req.authorize())
                }
                ItemPhase::TimedOut => Err(ConversionError::LimitExceeded {
                    limit: Limit::Timeout {
                        limit: self.slot.timeout,
                    },
                }),
                _ => Err(ConversionError::Cancelled),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use radivine_jobs::{Origin, RequestId};
    use std::thread;

    /// `T-WATCHDOG-FENCE`, `T-WATCHDOG-LOCK-NONBLOCKING`, and `T-WATCHDOG-COLLISION-FENCE`:
    /// 1. Verify watchdog `try_mark_timeout` acquires `Mutex<ItemPhase>` in `< 5 ms` even when
    ///    Step B (`PRAGMA synchronous = FULL` SQLite write) is artificially stalled for `500 ms`.
    /// 2. Verify Step C aborts with `Limit::Timeout` and refuses to issue `CommitPermit` when the
    ///    watchdog marked `TimedOut` during Step B (`T-WATCHDOG-FENCE`).
    /// 3. Verify that if a timeout occurs during attempt 0 (`InCommit { attempt: 0, timeout_observed: true }`)
    ///    and attempt 0 fails with `EEXIST`, Step A of attempt 1 immediately aborts with `Limit::Timeout`
    ///    and never publishes attempt 1 (`T-WATCHDOG-COLLISION-FENCE`).
    #[test]
    fn t_watchdog_fence_nonblocking_and_collision_fence() {
        let tmp = tempfile::tempdir().unwrap();
        let store = Arc::new(JobStore::open(&tmp.path().join("jobs.sqlite3")).unwrap());
        let (job_id, _, _) = store
            .submit_or_deduplicate(
                &RequestId::generate(),
                Origin::Test,
                "convert.image",
                &[tmp.path().join("a.png")],
                &serde_json::json!({"target": "webp"}),
            )
            .unwrap();
        store.mark_job_preparing(job_id).unwrap();
        store
            .mark_job_running(job_id, &serde_json::json!({"target": "webp"}))
            .unwrap();
        let claimed = store
            .claim_next_pending_item_for_job(job_id)
            .unwrap()
            .unwrap();

        let gov = ResourceGovernor::new(256 * 1024 * 1024, 768 * 1024 * 1024);
        let cancel = Arc::new(AtomicBool::new(false));
        let slot = InFlightItemSlot::new(
            claimed.item_id,
            job_id,
            0,
            Duration::from_millis(50),
            cancel,
        );

        // Part 1 & 2: Stall Step B for 500 ms while watchdog calls try_mark_timeout()
        let slot_for_wd = slot.clone();
        let wd_handle = thread::spawn(move || {
            thread::sleep(Duration::from_millis(80));
            let (outcome, lock_wait) = slot_for_wd.try_mark_timeout();
            assert_eq!(outcome, TimeoutMarkOutcome::MarkedTimedOut);
            assert!(
                lock_wait < Duration::from_millis(5),
                "watchdog lock acquisition took {lock_wait:?}, must be < 5ms"
            );
        });

        let mut noop_stage = |_: ItemState, _: Stage, _: JobProgressCounts| {};
        let slow_step_b = || {
            thread::sleep(Duration::from_millis(500));
        };
        let mut ctrl = WorkerItemControl {
            store: &store,
            governor: &gov,
            slot: &slot,
            dest_dir_for_precheck: tmp.path(),
            on_stage_cb: &mut noop_stage,
            step_b_hook: Some(&slow_step_b),
        };

        let stage_path = tmp.path().join(".rvstage-1-0123456789abcdef");
        let temp_path = stage_path.join("item-0-0123456789abcdef.part");
        let cand0 = tmp.path().join("a.webp");
        let req0 = CommitRequest {
            stage_dir_path: &stage_path,
            temp_path: &temp_path,
            temp_dev: 1,
            temp_ino: 2,
            final_output_path: &cand0,
            attempt: 0,
        };

        let res0 = ctrl.acquire_commit_permit(&req0);
        wd_handle.join().unwrap();
        assert!(matches!(
            res0,
            Err(ConversionError::LimitExceeded {
                limit: Limit::Timeout { .. }
            })
        ));
        assert_eq!(slot.phase(), ItemPhase::TimedOut);

        // Part 3: T-WATCHDOG-COLLISION-FENCE:
        // Suppose attempt 0 acquired permit before timeout, then watchdog observed timeout during InCommit(0),
        // and attempt 0 got EEXIST -> attempt 1 calls acquire_commit_permit:
        let cancel2 = Arc::new(AtomicBool::new(false));
        let slot2 = InFlightItemSlot::new(
            claimed.item_id,
            job_id,
            0,
            Duration::from_millis(50),
            cancel2,
        );
        let mut ctrl2 = WorkerItemControl {
            store: &store,
            governor: &gov,
            slot: &slot2,
            dest_dir_for_precheck: tmp.path(),
            on_stage_cb: &mut noop_stage,
            step_b_hook: None,
        };
        let _permit0 = ctrl2.acquire_commit_permit(&req0).unwrap();
        assert_eq!(
            slot2.phase(),
            ItemPhase::InCommit {
                attempt: 0,
                timeout_observed: false
            }
        );

        // Watchdog fires while attempt 0 is in commit_no_replace:
        let (outcome2, wait2) = slot2.try_mark_timeout();
        assert_eq!(outcome2, TimeoutMarkOutcome::DeferredInCommit);
        assert!(wait2 < Duration::from_millis(5));
        assert_eq!(
            slot2.phase(),
            ItemPhase::InCommit {
                attempt: 0,
                timeout_observed: true
            }
        );

        // Attempt 0 returns EEXIST, so PendingOutput loops to attempt 1 ("a (1).webp"):
        let cand1 = tmp.path().join("a (1).webp");
        let req1 = CommitRequest {
            stage_dir_path: &stage_path,
            temp_path: &temp_path,
            temp_dev: 1,
            temp_ino: 2,
            final_output_path: &cand1,
            attempt: 1,
        };
        let res1 = ctrl2.acquire_commit_permit(&req1);
        assert!(matches!(
            res1,
            Err(ConversionError::LimitExceeded {
                limit: Limit::Timeout { .. }
            })
        ));
        assert_eq!(slot2.phase(), ItemPhase::TimedOut);
    }
}
