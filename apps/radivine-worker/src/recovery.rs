//! Startup crash recovery (`C0a`–`C6`) and `boot_id` reconciliation (`JOB_SYSTEM_DESIGN.md` §4.3, `SECURITY_AND_FILE_SAFETY.md` §3.6).

use std::fs::{self, File};
use std::os::unix::fs::MetadataExt;
use std::path::Path;

use radivine_convert::{
    RealFsOps, Warning, cleanup_staging_dir_if_empty, cleanup_temp_safely,
    is_canonical_stage_dir_name, is_canonical_temp_part_name,
};
use radivine_jobs::ItemState;
use tracing::{info, warn};

use crate::store::{JobStore, RecoveryItemRow, StoreError};

/// Summary of actions taken during startup crash recovery.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RecoveryReport {
    pub dirty_previous_shutdown: bool,
    pub reboot_detected: bool,
    pub recovered_c0a: usize,
    pub recovered_c0b: usize,
    pub recovered_c1: usize,
    pub recovered_c2: usize,
    pub recovered_c3_succeeded: usize,
    pub recovered_c4_succeeded: usize,
    pub recovered_c5_stage_dirs: usize,
    pub reconciled_jobs: usize,
    pub orphaned_preserved: usize,
}

/// Reads `/proc/sys/kernel/random/boot_id` (falling back to `"unknown-boot-id"` if unavailable).
#[must_use]
pub fn read_current_boot_id() -> String {
    fs::read_to_string("/proc/sys/kernel/random/boot_id")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "unknown-boot-id".to_string())
}

/// Executes the complete `C0a`–`C6` startup recovery protocol before the worker binds `worker.sock`.
pub fn run_startup_recovery(
    store: &JobStore,
    current_boot_id: &str,
) -> Result<RecoveryReport, StoreError> {
    let (was_clean, prev_boot_id) = store.begin_worker_session(current_boot_id)?;
    let reboot_detected = prev_boot_id
        .as_deref()
        .is_some_and(|prev| prev != current_boot_id);

    let mut report = RecoveryReport {
        dirty_previous_shutdown: !was_clean,
        reboot_detected,
        ..RecoveryReport::default()
    };

    let rows = store.load_recovery_items()?;
    for row in rows {
        recover_single_item(store, &row, &mut report)?;
    }

    report.reconciled_jobs = store.reconcile_active_jobs_on_startup()?;
    if report.dirty_previous_shutdown
        || report.reboot_detected
        || report.recovered_c0a > 0
        || report.recovered_c0b > 0
        || report.recovered_c1 > 0
        || report.recovered_c2 > 0
        || report.recovered_c3_succeeded > 0
        || report.recovered_c4_succeeded > 0
        || report.recovered_c5_stage_dirs > 0
    {
        info!(?report, "completed worker startup crash recovery");
    }
    Ok(report)
}

fn recover_single_item(
    store: &JobStore,
    row: &RecoveryItemRow,
    report: &mut RecoveryReport,
) -> Result<(), StoreError> {
    let mut warnings: Vec<serde_json::Value> =
        serde_json::from_str(&row.warnings_json).unwrap_or_default();

    // Case C5 (or timed-out terminal item with leftover staging journal):
    if row.state.is_terminal() {
        clean_up_staged_artifacts_for_row(row, &mut warnings, report);
        let warnings_json = serde_json::to_string(&warnings).unwrap_or_else(|_| "[]".to_string());
        store.clear_terminal_item_staging_journal(row.item_id, &warnings_json)?;
        report.recovered_c5_stage_dirs += 1;
        return Ok(());
    }

    match row.state {
        ItemState::Inspecting | ItemState::Running => {
            recover_inspecting_or_running(store, row, &mut warnings, report)?;
        }
        ItemState::Committing => {
            recover_committing(store, row, &mut warnings, report)?;
        }
        _ => {}
    }

    Ok(())
}

fn recover_inspecting_or_running(
    store: &JobStore,
    row: &RecoveryItemRow,
    warnings: &mut Vec<serde_json::Value>,
    report: &mut RecoveryReport,
) -> Result<(), StoreError> {
    if let Some(ref stage_dir_path) = row.stage_dir_path {
        let stage_name_ok = stage_dir_path
            .file_name()
            .and_then(|n| n.to_str())
            .is_some_and(is_canonical_stage_dir_name);

        match (row.stage_dev, row.stage_ino) {
            // C0a or C0b: stage_dir_path committed, but (stage_dev, stage_ino) is NULL
            (None, _) | (_, None) => {
                if stage_name_ok {
                    match fs::symlink_metadata(stage_dir_path) {
                        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                            // C0a: crashed before mkdirat
                            report.recovered_c0a += 1;
                        }
                        Ok(meta) if meta.is_dir() => {
                            // C0b: crashed after mkdirat before stage_dev/stage_ino journal
                            report.recovered_c0b += 1;
                            if is_dir_empty(stage_dir_path) {
                                let _ = fs::remove_dir(stage_dir_path);
                                sync_parent_dir(stage_dir_path);
                            } else {
                                report.orphaned_preserved += 1;
                                push_warning(
                                    warnings,
                                    Warning::OrphanedTempPreserved {
                                        stage_dir: stage_dir_path.to_string_lossy().into_owned(),
                                        reason: "stage_dir created before C0b journal is non-empty; preserved without pattern deletion".to_string(),
                                    },
                                );
                            }
                        }
                        _ => {
                            report.recovered_c0b += 1;
                        }
                    }
                }
            }
            // C1 (unjournaled or journaled): (stage_dev, stage_ino) is recorded
            (Some(s_dev), Some(s_ino)) => {
                report.recovered_c1 += 1;
                if stage_name_ok {
                    if let (Some(temp_path), Some(t_dev), Some(t_ino)) =
                        (&row.temp_path, row.temp_dev, row.temp_ino)
                    {
                        unlink_verified_temp_if_present(
                            stage_dir_path,
                            s_dev,
                            s_ino,
                            temp_path,
                            t_dev,
                            t_ino,
                        );
                    }

                    match cleanup_staging_dir_if_empty(stage_dir_path, s_dev, s_ino) {
                        Ok(true) => {}
                        Ok(false) => {
                            // If stage_dir still exists with matching (s_dev, s_ino) and is non-empty
                            // because of an unjournaled temp file (C1_unjournaled), record OrphanedTempPreserved.
                            if let Ok(meta) = fs::symlink_metadata(stage_dir_path)
                                && meta.is_dir()
                                && meta.dev() == s_dev
                                && meta.ino() == s_ino
                                && !is_dir_empty(stage_dir_path)
                            {
                                report.orphaned_preserved += 1;
                                push_warning(
                                    warnings,
                                    Warning::OrphanedTempPreserved {
                                        stage_dir: stage_dir_path.to_string_lossy().into_owned(),
                                        reason: "unjournaled temp file in stage_dir preserved per INV-RECOVERY-OWNERSHIP".to_string(),
                                    },
                                );
                            }
                        }
                        Err(e) => {
                            warn!(
                                path = %stage_dir_path.display(),
                                error = %e,
                                "failed to clean up staging dir during C1 recovery"
                            );
                        }
                    }
                }
            }
        }
    } else {
        report.recovered_c1 += 1;
    }

    let warnings_json = serde_json::to_string(warnings).unwrap_or_else(|_| "[]".to_string());
    store.apply_recovery_item_update(
        row.item_id,
        ItemState::Interrupted,
        None,
        None,
        Some("interrupted"),
        Some("interrupted"),
        Some("Worker crashed or restarted while item was running"),
        true,
        &warnings_json,
    )
}

fn recover_committing(
    store: &JobStore,
    row: &RecoveryItemRow,
    warnings: &mut Vec<serde_json::Value>,
    report: &mut RecoveryReport,
) -> Result<(), StoreError> {
    let t_dev = row.temp_dev;
    let t_ino = row.temp_ino;

    let output_matches_temp_inode = match (&row.output_path, t_dev, t_ino) {
        (Some(out_path), Some(expected_dev), Some(expected_ino)) => {
            match fs::symlink_metadata(out_path) {
                Ok(meta)
                    if meta.is_file()
                        && meta.dev() == expected_dev
                        && meta.ino() == expected_ino
                        && meta.len() > 0 =>
                {
                    Some(meta.len())
                }
                _ => None,
            }
        }
        _ => None,
    };

    let temp_exists_with_inode = match (&row.temp_path, t_dev, t_ino) {
        (Some(temp_path), Some(expected_dev), Some(expected_ino)) => {
            fs::symlink_metadata(temp_path).is_ok_and(|meta| {
                meta.is_file() && meta.dev() == expected_dev && meta.ino() == expected_ino
            })
        }
        _ => false,
    };

    // Case C3 or C4: output_path is already published on disk with the exact (temp_dev, temp_ino) and non-zero size!
    if let (Some(out_path), Some(output_bytes)) = (&row.output_path, output_matches_temp_inode) {
        if temp_exists_with_inode {
            // Case C3: Rung B hardlink succeeded, crashed before unlinkat(temp)
            if let (
                Some(stage_dir_path),
                Some(s_dev),
                Some(s_ino),
                Some(temp_path),
                Some(td),
                Some(ti),
            ) = (
                &row.stage_dir_path,
                row.stage_dev,
                row.stage_ino,
                &row.temp_path,
                t_dev,
                t_ino,
            ) {
                unlink_verified_temp_if_present(stage_dir_path, s_dev, s_ino, temp_path, td, ti);
                let _ = cleanup_staging_dir_if_empty(stage_dir_path, s_dev, s_ino);
            }
            sync_parent_dir(out_path);
            report.recovered_c3_succeeded += 1;
            push_warning(
                warnings,
                Warning::RecoveredAfterInterruption {
                    recovery_stage: "C3".to_string(),
                },
            );
        } else {
            // Case C4: Rung A renameat2 or Rung B unlinkat(temp) succeeded, crashed before Succeeded DB commit
            if let (Some(stage_dir_path), Some(s_dev), Some(s_ino)) =
                (&row.stage_dir_path, row.stage_dev, row.stage_ino)
            {
                let _ = cleanup_staging_dir_if_empty(stage_dir_path, s_dev, s_ino);
            }
            sync_parent_dir(out_path);
            report.recovered_c4_succeeded += 1;
            push_warning(
                warnings,
                Warning::RecoveredAfterInterruption {
                    recovery_stage: "C4".to_string(),
                },
            );
        }

        let warnings_json = serde_json::to_string(warnings).unwrap_or_else(|_| "[]".to_string());
        return store.apply_recovery_item_update(
            row.item_id,
            ItemState::Succeeded,
            Some(out_path),
            Some(output_bytes),
            None,
            None,
            None,
            false,
            &warnings_json,
        );
    }

    // Case C2: output_path was not published with (temp_dev, temp_ino); clean up temp_path and mark Interrupted
    report.recovered_c2 += 1;
    if let (Some(stage_dir_path), Some(s_dev), Some(s_ino)) =
        (&row.stage_dir_path, row.stage_dev, row.stage_ino)
    {
        if let (Some(temp_path), Some(td), Some(ti)) = (&row.temp_path, t_dev, t_ino) {
            unlink_verified_temp_if_present(stage_dir_path, s_dev, s_ino, temp_path, td, ti);
        }
        let _ = cleanup_staging_dir_if_empty(stage_dir_path, s_dev, s_ino);
    }

    let warnings_json = serde_json::to_string(warnings).unwrap_or_else(|_| "[]".to_string());
    store.apply_recovery_item_update(
        row.item_id,
        ItemState::Interrupted,
        None,
        None,
        Some("interrupted"),
        Some("interrupted"),
        Some("Worker crashed before output publication completed (C2)"),
        true,
        &warnings_json,
    )
}

fn clean_up_staged_artifacts_for_row(
    row: &RecoveryItemRow,
    warnings: &mut Vec<serde_json::Value>,
    report: &mut RecoveryReport,
) {
    let Some(ref stage_dir_path) = row.stage_dir_path else {
        return;
    };
    let stage_name_ok = stage_dir_path
        .file_name()
        .and_then(|n| n.to_str())
        .is_some_and(is_canonical_stage_dir_name);
    if !stage_name_ok {
        return;
    }

    if let (Some(s_dev), Some(s_ino)) = (row.stage_dev, row.stage_ino) {
        if let (Some(temp_path), Some(t_dev), Some(t_ino)) =
            (&row.temp_path, row.temp_dev, row.temp_ino)
        {
            unlink_verified_temp_if_present(stage_dir_path, s_dev, s_ino, temp_path, t_dev, t_ino);
        }
        let _ = cleanup_staging_dir_if_empty(stage_dir_path, s_dev, s_ino);
    } else if let Ok(meta) = fs::symlink_metadata(stage_dir_path)
        && meta.is_dir()
    {
        if is_dir_empty(stage_dir_path) {
            let _ = fs::remove_dir(stage_dir_path);
            sync_parent_dir(stage_dir_path);
        } else {
            report.orphaned_preserved += 1;
            push_warning(
                warnings,
                Warning::OrphanedTempPreserved {
                    stage_dir: stage_dir_path.to_string_lossy().into_owned(),
                    reason: "unverified terminal item stage_dir preserved".to_string(),
                },
            );
        }
    }
}

fn unlink_verified_temp_if_present(
    stage_dir_path: &Path,
    expected_stage_dev: u64,
    expected_stage_ino: u64,
    temp_path: &Path,
    expected_temp_dev: u64,
    expected_temp_ino: u64,
) {
    let Some(temp_name) = temp_path.file_name() else {
        return;
    };
    if !temp_name.to_str().is_some_and(is_canonical_temp_part_name) {
        return;
    };
    let Ok(stage_meta) = fs::symlink_metadata(stage_dir_path) else {
        return;
    };
    if !stage_meta.is_dir()
        || stage_meta.dev() != expected_stage_dev
        || stage_meta.ino() != expected_stage_ino
    {
        return;
    }
    let Ok(stage_fd) = File::open(stage_dir_path) else {
        return;
    };
    let _ = cleanup_temp_safely(
        &stage_fd,
        temp_name,
        expected_temp_dev,
        expected_temp_ino,
        &RealFsOps,
    );
}

fn is_dir_empty(dir: &Path) -> bool {
    let Ok(mut iter) = fs::read_dir(dir) else {
        return false;
    };
    iter.next().is_none()
}

fn sync_parent_dir(path: &Path) {
    if let Some(parent) = path.parent()
        && let Ok(fd) = File::open(parent)
    {
        let _ = fd.sync_all();
    }
}

fn push_warning(warnings: &mut Vec<serde_json::Value>, warning: Warning) {
    if let Ok(val) = serde_json::to_value(warning) {
        warnings.push(val);
    }
}
