//! Memory-budgeted `ResourceGovernor` with FIFO admission, exclusive oversized execution,
//! drain mode, and `statvfs` free-space prechecks (`JOB_SYSTEM_DESIGN.md` §6.2–§6.4, `AC-C2.12`).

use std::collections::VecDeque;
use std::env;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

use radivine_convert::{ConversionError, FsOp, Limit, Reservation, ResourceEstimate};

#[derive(Debug)]
struct GovernorState {
    memory_budget_bytes: u64,
    worker_memory_max_bytes: u64,
    max_concurrent_items: usize,
    in_use_bytes: u64,
    max_observed_in_use_bytes: u64,
    max_observed_concurrent_normal: usize,
    max_observed_concurrent_during_exclusive: usize,
    active_items: usize,
    exclusive_active: bool,
    draining: bool,
    next_ticket: u64,
    waiters: VecDeque<u64>,
}

/// Memory-budgeted concurrency governor shared across worker threads.
#[derive(Debug, Clone)]
pub struct ResourceGovernor {
    inner: Arc<(Mutex<GovernorState>, Condvar)>,
}

struct GovernorLease {
    bytes: u64,
    exclusive: bool,
    inner: Arc<(Mutex<GovernorState>, Condvar)>,
}

impl Drop for GovernorLease {
    fn drop(&mut self) {
        let (lock, cvar) = &*self.inner;
        if let Ok(mut st) = lock.lock() {
            st.in_use_bytes = st.in_use_bytes.saturating_sub(self.bytes);
            st.active_items = st.active_items.saturating_sub(1);
            if self.exclusive {
                st.exclusive_active = false;
            }
            cvar.notify_all();
        }
    }
}

impl ResourceGovernor {
    /// Creates a new [`ResourceGovernor`] with the specified budget and hard per-worker ceiling.
    #[must_use]
    pub fn new(memory_budget_bytes: u64, worker_memory_max_bytes: u64) -> Self {
        Self {
            inner: Arc::new((
                Mutex::new(GovernorState {
                    memory_budget_bytes,
                    worker_memory_max_bytes,
                    max_concurrent_items: usize::MAX,
                    in_use_bytes: 0,
                    max_observed_in_use_bytes: 0,
                    max_observed_concurrent_normal: 0,
                    max_observed_concurrent_during_exclusive: 0,
                    active_items: 0,
                    exclusive_active: false,
                    draining: false,
                    next_ticket: 0,
                    waiters: VecDeque::new(),
                }),
                Condvar::new(),
            )),
        }
    }

    #[must_use]
    pub fn memory_budget_bytes(&self) -> u64 {
        self.inner.0.lock().unwrap().memory_budget_bytes
    }

    #[must_use]
    pub fn worker_memory_max_bytes(&self) -> u64 {
        self.inner.0.lock().unwrap().worker_memory_max_bytes
    }

    /// Dynamically updates the governor's memory budget, hard worker ceiling, and concurrency cap.
    pub fn update_limits(
        &self,
        memory_budget_bytes: u64,
        worker_memory_max_bytes: u64,
        max_concurrent_items: usize,
    ) {
        let (lock, cvar) = &*self.inner;
        let mut st = lock.lock().unwrap();
        st.memory_budget_bytes = memory_budget_bytes;
        st.worker_memory_max_bytes = worker_memory_max_bytes.max(memory_budget_bytes);
        st.max_concurrent_items = max_concurrent_items.max(1);
        cvar.notify_all();
    }

    /// Transitions the governor into drain mode: wakes all waiting threads and causes all
    /// pending/subsequent `reserve` calls to return `Err(ConversionError::Cancelled)`.
    pub fn begin_drain(&self) {
        let (lock, cvar) = &*self.inner;
        let mut st = lock.lock().unwrap();
        st.draining = true;
        cvar.notify_all();
    }

    #[must_use]
    pub fn is_draining(&self) -> bool {
        self.inner.0.lock().unwrap().draining
    }

    #[must_use]
    pub fn active_items(&self) -> usize {
        self.inner.0.lock().unwrap().active_items
    }

    #[must_use]
    pub fn in_use_bytes(&self) -> u64 {
        self.inner.0.lock().unwrap().in_use_bytes
    }

    /// Returns `(max_observed_in_use_bytes, max_observed_concurrent_normal, max_observed_concurrent_during_exclusive)`.
    #[must_use]
    pub fn telemetry(&self) -> (u64, usize, usize) {
        let st = self.inner.0.lock().unwrap();
        (
            st.max_observed_in_use_bytes,
            st.max_observed_concurrent_normal,
            st.max_observed_concurrent_during_exclusive,
        )
    }

    /// Acquires a memory reservation in FIFO order, blocking until budget is available or
    /// cancellation/drain/timeout occurs (`AC-C2.12`).
    pub fn reserve(
        &self,
        estimate: ResourceEstimate,
        cancel_flag: &AtomicBool,
        check_timeout: &dyn Fn() -> Option<Duration>,
    ) -> Result<Reservation, ConversionError> {
        let needed = estimate.peak_ram_bytes;
        let (lock, cvar) = &*self.inner;
        let mut st = lock.lock().unwrap();

        if needed > st.worker_memory_max_bytes {
            return Err(ConversionError::LimitExceeded {
                limit: Limit::MemoryBytes {
                    needed,
                    max: st.worker_memory_max_bytes,
                },
            });
        }

        if st.draining || cancel_flag.load(Ordering::Relaxed) {
            return Err(ConversionError::Cancelled);
        }
        if let Some(elapsed) = check_timeout() {
            return Err(ConversionError::LimitExceeded {
                limit: Limit::Timeout { limit: elapsed },
            });
        }

        let ticket = st.next_ticket;
        st.next_ticket += 1;
        st.waiters.push_back(ticket);

        loop {
            if st.draining || cancel_flag.load(Ordering::Relaxed) {
                st.waiters.retain(|&t| t != ticket);
                cvar.notify_all();
                return Err(ConversionError::Cancelled);
            }
            if let Some(elapsed) = check_timeout() {
                st.waiters.retain(|&t| t != ticket);
                cvar.notify_all();
                return Err(ConversionError::LimitExceeded {
                    limit: Limit::Timeout { limit: elapsed },
                });
            }

            if needed > st.worker_memory_max_bytes {
                st.waiters.retain(|&t| t != ticket);
                cvar.notify_all();
                return Err(ConversionError::LimitExceeded {
                    limit: Limit::MemoryBytes {
                        needed,
                        max: st.worker_memory_max_bytes,
                    },
                });
            }

            let requires_exclusive = needed > st.memory_budget_bytes;
            let is_head = st.waiters.front() == Some(&ticket);
            if is_head {
                let can_admit = if requires_exclusive {
                    st.active_items == 0 && !st.exclusive_active
                } else {
                    !st.exclusive_active
                        && st.active_items < st.max_concurrent_items
                        && st.in_use_bytes.saturating_add(needed) <= st.memory_budget_bytes
                };

                if can_admit {
                    st.waiters.pop_front();
                    st.in_use_bytes = st.in_use_bytes.saturating_add(needed);
                    st.active_items += 1;
                    if requires_exclusive {
                        st.exclusive_active = true;
                        st.max_observed_concurrent_during_exclusive = st
                            .max_observed_concurrent_during_exclusive
                            .max(st.active_items);
                    } else {
                        st.max_observed_in_use_bytes =
                            st.max_observed_in_use_bytes.max(st.in_use_bytes);
                        st.max_observed_concurrent_normal =
                            st.max_observed_concurrent_normal.max(st.active_items);
                    }
                    if st.exclusive_active {
                        st.max_observed_concurrent_during_exclusive = st
                            .max_observed_concurrent_during_exclusive
                            .max(st.active_items);
                    }
                    cvar.notify_all();
                    return Ok(Reservation::new(GovernorLease {
                        bytes: needed,
                        exclusive: requires_exclusive,
                        inner: self.inner.clone(),
                    }));
                }
            }

            let (guard, _) = cvar.wait_timeout(st, Duration::from_millis(25)).unwrap();
            st = guard;
        }
    }
}

/// Verifies that the destination filesystem has at least `2 * estimate.temp_disk_bytes + 32 MiB`
/// of available space (`JOB_SYSTEM_DESIGN.md` §6.3).
pub fn check_destination_free_space(
    dest_dir: &Path,
    estimate: ResourceEstimate,
) -> Result<(), ConversionError> {
    let required = estimate
        .temp_disk_bytes
        .saturating_mul(2)
        .saturating_add(32 * 1024 * 1024);

    if let Ok(fake_str) = env::var("RADIVINE_WORKER_TEST_FAKE_AVAIL_BYTES")
        && let Ok(available) = fake_str.trim().parse::<u64>()
    {
        if available < required {
            return Err(ConversionError::Io {
                op: FsOp::CreateTemp,
                path: dest_dir.to_path_buf(),
                source: std::io::Error::from_raw_os_error(rustix::io::Errno::NOSPC.raw_os_error()),
            });
        }
        return Ok(());
    }

    let mut probe_path = dest_dir;
    while !probe_path.exists() {
        if let Some(parent) = probe_path.parent()
            && !parent.as_os_str().is_empty()
        {
            probe_path = parent;
        } else {
            break;
        }
    }

    if let Ok(vfs) = rustix::fs::statvfs(probe_path) {
        let block_size = if vfs.f_frsize > 0 {
            vfs.f_frsize
        } else {
            vfs.f_bsize
        };
        let available = vfs.f_bavail.saturating_mul(block_size);
        if available < required {
            return Err(ConversionError::Io {
                op: FsOp::CreateTemp,
                path: dest_dir.to_path_buf(),
                source: std::io::Error::from_raw_os_error(rustix::io::Errno::NOSPC.raw_os_error()),
            });
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread;

    #[test]
    fn governor_enforces_budget_and_exclusive_oversized_admission() {
        let budget = 256 * 1024 * 1024;
        let max_worker = 768 * 1024 * 1024;
        let gov = ResourceGovernor::new(budget, max_worker);
        let cancel = AtomicBool::new(false);
        let no_timeout = || None;

        // 1. Two 120 MiB items fit concurrently (240 MiB <= 256 MiB)
        let r1 = gov
            .reserve(
                ResourceEstimate {
                    peak_ram_bytes: 120 * 1024 * 1024,
                    temp_disk_bytes: 1024,
                },
                &cancel,
                &no_timeout,
            )
            .unwrap();
        let r2 = gov
            .reserve(
                ResourceEstimate {
                    peak_ram_bytes: 120 * 1024 * 1024,
                    temp_disk_bytes: 1024,
                },
                &cancel,
                &no_timeout,
            )
            .unwrap();
        assert_eq!(gov.active_items(), 2);
        assert_eq!(gov.in_use_bytes(), 240 * 1024 * 1024);

        // 2. An oversized 350 MiB item (> 256 MiB budget, <= 768 MiB ceiling) waits until r1 & r2 drop, then runs exclusively
        let gov_clone = gov.clone();
        let handle = thread::spawn(move || {
            let cancel_inner = AtomicBool::new(false);
            let _r_oversized = gov_clone
                .reserve(
                    ResourceEstimate {
                        peak_ram_bytes: 350 * 1024 * 1024,
                        temp_disk_bytes: 1024,
                    },
                    &cancel_inner,
                    &|| None,
                )
                .unwrap();
            assert_eq!(gov_clone.active_items(), 1);
            thread::sleep(Duration::from_millis(40));
        });

        thread::sleep(Duration::from_millis(30));
        drop(r1);
        drop(r2);
        handle.join().unwrap();

        let (max_normal_bytes, max_normal_conc, max_excl_conc) = gov.telemetry();
        assert!(max_normal_bytes <= budget);
        assert_eq!(max_normal_conc, 2);
        assert_eq!(max_excl_conc, 1);
    }
}
