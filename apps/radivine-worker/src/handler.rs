//! Generic `JobHandler` trait and built-in `ConvertImageHandler` (`"convert.image"`, `ADR-027`).

use std::any::Any;
use std::env;
use std::fs::File;
use std::io::{BufReader, Read};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use radivine_convert::{
    BackendPrefs, CapabilityCatalog, ConversionEngine, ConversionError, ConversionOptions,
    ConvertSettings, FileIdentity, FormatId, PlannedItem, PresetOverrides, PresetRef, ProbeEnv,
    ResourceLimitsConfig, SameFormatPolicy, Stage, ValidatedOptions, derive_suffixed_filename_os,
    probe_all, random_hex16, resolve_output_destination_os, sniff_format,
};
use radivine_convert_image::{ImageBackend, descriptor as image_descriptor};
use radivine_jobs::{CONVERT_IMAGE_KIND, ItemState, JobProgressCounts, JobSummary};
use serde::{Deserialize, Serialize};

use crate::fault::WorkerFsOps;
use crate::fence::{InFlightItemSlot, StageCallback, WorkerItemControl};
use crate::governor::ResourceGovernor;
use crate::store::{ClaimedItem, JobStore};

/// Prepared job context produced during `JobState::Preparing` and cached for the job's execution.
pub struct PreparedJob {
    pub resolved_json: serde_json::Value,
    pub per_item_timeout: Duration,
    pub payload: Box<dyn Any + Send + Sync>,
}

/// Error returned during `JobHandler::prepare`.
#[derive(Debug, Clone)]
pub struct PrepareError {
    pub code: String,
    pub class: String,
    pub message: String,
}

/// Outcome of executing a single claimed item via [`JobHandler::execute_item`].
#[derive(Debug, Clone)]
pub enum ItemExecutionOutcome {
    Succeeded {
        output_path: PathBuf,
        source_bytes: u64,
        output_bytes: u64,
        duration_ms: u64,
        warnings_json: String,
    },
    Skipped {
        reason_code: String,
        reason_message: String,
        warnings_json: String,
    },
    Failed {
        error_code: String,
        error_class: String,
        error_message: String,
        retryable: bool,
        duration_ms: u64,
        warnings_json: String,
        timed_out: bool,
        is_dest_space_or_perm_error: bool,
    },
    Cancelled {
        duration_ms: u64,
    },
}

/// Generic handler trait implemented by each job kind (`"convert.image"`, etc.).
pub trait JobHandler: Send + Sync {
    /// Returns the canonical `jobs.kind` identifier handled by this implementation.
    fn kind(&self) -> &'static str;

    /// Validates job options against current settings during `JobState::Preparing` and produces
    /// the immutable `jobs.resolved` snapshot.
    fn prepare(&self, job: &JobSummary) -> Result<PreparedJob, PrepareError>;

    /// Executes a single claimed item.
    fn execute_item(
        &self,
        prepared: &PreparedJob,
        claimed: &ClaimedItem,
        slot: &Arc<InFlightItemSlot>,
        store: &JobStore,
        governor: &ResourceGovernor,
        on_stage: StageCallback<'_>,
    ) -> ItemExecutionOutcome;
}

/// Flexible JSON wire specification for `"convert.image"` job options.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct ConvertImageWireOptions {
    #[serde(default)]
    target: Option<FormatId>,
    #[serde(default)]
    preset_ref: Option<String>,
    #[serde(default)]
    overrides: Option<PresetOverrides>,
    #[serde(default)]
    options: Option<ConversionOptions>,
}

struct PreparedConvertImage {
    engine: Arc<ConversionEngine>,
    validated: ValidatedOptions,
    stage_dir_name: String,
}

/// Built-in `"convert.image"` [`JobHandler`] backed by `radivine-convert` and `radivine-convert-image`.
pub struct ConvertImageHandler {
    catalog: CapabilityCatalog,
}

impl Default for ConvertImageHandler {
    fn default() -> Self {
        Self::new()
    }
}

impl ConvertImageHandler {
    #[must_use]
    pub fn new() -> Self {
        let descriptors = vec![image_descriptor()];
        let catalog = probe_all(&descriptors, &BackendPrefs::default(), &ProbeEnv::default());
        Self { catalog }
    }

    #[must_use]
    pub fn resolve_convert_toml_path() -> PathBuf {
        if let Ok(xdg) = env::var("XDG_CONFIG_HOME")
            && !xdg.trim().is_empty()
        {
            return PathBuf::from(xdg).join("radivine").join("convert.toml");
        }
        if let Ok(home) = env::var("HOME")
            && !home.trim().is_empty()
        {
            return PathBuf::from(home)
                .join(".config")
                .join("radivine")
                .join("convert.toml");
        }
        PathBuf::from("/tmp/radivine/convert.toml")
    }

    fn parse_conversion_options(
        raw_options: &serde_json::Value,
        settings: &ConvertSettings,
    ) -> Result<ConversionOptions, PrepareError> {
        let normalized_options = crate::store::normalize_options_value(raw_options);
        let raw_options = &normalized_options;
        if raw_options.is_null()
            || raw_options
                .as_object()
                .is_some_and(serde_json::Map::is_empty)
        {
            return settings
                .resolve(&PresetRef::DefaultPreset, None)
                .map_err(|msg| PrepareError {
                    code: "invalid_options".to_string(),
                    class: "user_input".to_string(),
                    message: msg,
                });
        }

        // 1. Direct ConversionOptions JSON object (when all fields match ConversionOptions)
        if let Ok(direct) = serde_json::from_value::<ConversionOptions>(raw_options.clone()) {
            return Ok(direct);
        }

        // 2. Preset + overrides wrapper (`target` / `preset_ref` / `overrides` / `options`)
        if let Ok(wire) = serde_json::from_value::<ConvertImageWireOptions>(raw_options.clone()) {
            if let Some(opts) = wire.options {
                return Ok(opts);
            }
            if wire.target.is_some() || wire.preset_ref.is_some() || wire.overrides.is_some() {
                let pref = if let Some(pid) = wire.preset_ref {
                    PresetRef::PresetId(pid)
                } else if let Some(t) = wire.target {
                    PresetRef::Target(t)
                } else {
                    PresetRef::DefaultPreset
                };
                // Also support top-level PresetOverrides fields merged with `target`
                let top_ov = serde_json::from_value::<PresetOverrides>(raw_options.clone()).ok();
                let mut opts = settings
                    .resolve(&pref, wire.overrides.as_ref().or(top_ov.as_ref()))
                    .map_err(|msg| PrepareError {
                        code: "invalid_options".to_string(),
                        class: "user_input".to_string(),
                        message: msg,
                    })?;
                if let Some(t) = wire.target {
                    opts.target = t;
                }
                return Ok(opts);
            }
        }

        // 3. Partial PresetOverrides object (e.g. `{"quality": ...}` or `{"collision": "fail"}`)
        if let Ok(ov) = serde_json::from_value::<PresetOverrides>(raw_options.clone())
            && (ov.quality.is_some()
                || ov.resize.is_some()
                || ov.alpha_background.is_some()
                || ov.orientation.is_some()
                || ov.icc.is_some()
                || ov.exif.is_some()
                || ov.output_location.is_some()
                || ov.naming.is_some()
                || ov.collision.is_some()
                || ov.preserve_timestamps.is_some()
                || ov.same_format.is_some())
        {
            return settings
                .resolve(&PresetRef::DefaultPreset, Some(&ov))
                .map_err(|msg| PrepareError {
                    code: "invalid_options".to_string(),
                    class: "user_input".to_string(),
                    message: msg,
                });
        }

        Err(PrepareError {
            code: "invalid_options".to_string(),
            class: "user_input".to_string(),
            message: format!("invalid convert.image options JSON: {raw_options}"),
        })
    }
}

impl JobHandler for ConvertImageHandler {
    fn kind(&self) -> &'static str {
        CONVERT_IMAGE_KIND
    }

    fn prepare(&self, job: &JobSummary) -> Result<PreparedJob, PrepareError> {
        let (settings, _) = ConvertSettings::load_or_default(&Self::resolve_convert_toml_path());
        let source_json = job.resolved.as_ref().unwrap_or(&job.options);
        let options = Self::parse_conversion_options(source_json, &settings)?;

        let (validated, _warnings) = options.validate(&self.catalog).map_err(|issues| {
            let err = ConversionError::InvalidOptions { issues };
            PrepareError {
                code: err.code().to_string(),
                class: err.class().as_str().to_string(),
                message: err.user_summary(),
            }
        })?;

        let resolved_json =
            serde_json::to_value(validated.options()).map_err(|e| PrepareError {
                code: "invalid_options".to_string(),
                class: "invalid_options".to_string(),
                message: e.to_string(),
            })?;

        let limits: ResourceLimitsConfig = settings.limits;
        let per_item_timeout = if let Ok(ms_str) = env::var("RADIVINE_WORKER_ITEM_TIMEOUT_MS")
            && let Ok(ms) = ms_str.trim().parse::<u64>()
        {
            Duration::from_millis(ms.max(10))
        } else {
            Duration::from_secs(limits.per_item_timeout_secs.max(1))
        };

        let backend = Arc::new(ImageBackend::with_limits(limits.clone()));
        let engine = Arc::new(ConversionEngine::with_fs_ops(
            self.catalog.clone(),
            vec![backend],
            limits,
            Arc::new(WorkerFsOps::new()),
        ));

        let stage_dir_name = format!(".rvstage-{}-{}", job.job_id.0, random_hex16());

        Ok(PreparedJob {
            resolved_json,
            per_item_timeout,
            payload: Box::new(PreparedConvertImage {
                engine,
                validated,
                stage_dir_name,
            }),
        })
    }

    fn execute_item(
        &self,
        prepared: &PreparedJob,
        claimed: &ClaimedItem,
        slot: &Arc<InFlightItemSlot>,
        store: &JobStore,
        governor: &ResourceGovernor,
        on_stage: StageCallback<'_>,
    ) -> ItemExecutionOutcome {
        let start = Instant::now();
        let Some(ctx) = prepared.payload.downcast_ref::<PreparedConvertImage>() else {
            return ItemExecutionOutcome::Failed {
                error_code: "internal_error".to_string(),
                error_class: "backend_bug".to_string(),
                error_message: "invalid prepared job payload".to_string(),
                retryable: false,
                duration_ms: 0,
                warnings_json: "[]".to_string(),
                timed_out: false,
                is_dest_space_or_perm_error: false,
            };
        };

        // Optional test delay hook (e.g., for cancellation / fairness / watchdog tests)
        if let Ok(sleep_ms_str) = env::var("RADIVINE_WORKER_TEST_ITEM_SLEEP_MS")
            && let Ok(total_ms) = sleep_ms_str.trim().parse::<u64>()
            && total_ms > 0
        {
            let deadline = Instant::now() + Duration::from_millis(total_ms);
            while Instant::now() < deadline {
                if slot.cancel_flag.load(std::sync::atomic::Ordering::Relaxed)
                    || governor.is_draining()
                {
                    return ItemExecutionOutcome::Cancelled {
                        duration_ms: start.elapsed().as_millis() as u64,
                    };
                }
                if slot.phase() == crate::fence::ItemPhase::TimedOut {
                    return ItemExecutionOutcome::Failed {
                        error_code: "timed_out".to_string(),
                        error_class: "limit_exceeded".to_string(),
                        error_message: "Conversion exceeded the per-file time limit".to_string(),
                        retryable: false,
                        duration_ms: start.elapsed().as_millis() as u64,
                        warnings_json: "[]".to_string(),
                        timed_out: true,
                        is_dest_space_or_perm_error: false,
                    };
                }
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        // Pre-plan single item (stat + magic sniff + route)
        let planned_res = plan_single_item(
            &ctx.engine,
            &ctx.validated,
            claimed,
            &ctx.stage_dir_name,
            store,
        );

        let (planned, dest_dir) = match planned_res {
            Ok(PlanSingleOutcome::Ready(p, d)) => (p, d),
            Ok(PlanSingleOutcome::Skipped {
                reason_code,
                reason_message,
            }) => {
                return ItemExecutionOutcome::Skipped {
                    reason_code,
                    reason_message,
                    warnings_json: "[]".to_string(),
                };
            }
            Err(err) => {
                return map_conversion_error(err, start.elapsed());
            }
        };

        let mut ctrl = WorkerItemControl {
            store,
            governor,
            slot,
            dest_dir_for_precheck: &dest_dir,
            on_stage_cb: on_stage,
            step_b_hook: None,
        };

        match ctx.engine.convert_item(&planned, &ctx.validated, &mut ctrl) {
            Ok(converted) => {
                let warnings_json =
                    serde_json::to_string(&converted.warnings).unwrap_or_else(|_| "[]".to_string());
                if converted.skipped_existing {
                    ItemExecutionOutcome::Skipped {
                        reason_code: "destination_exists".to_string(),
                        reason_message: "Skipped because output file already exists".to_string(),
                        warnings_json,
                    }
                } else if let Some(out_path) = converted.output_path {
                    ItemExecutionOutcome::Succeeded {
                        output_path: out_path,
                        source_bytes: converted.source_bytes,
                        output_bytes: converted.output_bytes,
                        duration_ms: converted.duration.as_millis() as u64,
                        warnings_json,
                    }
                } else {
                    ItemExecutionOutcome::Skipped {
                        reason_code: "skipped".to_string(),
                        reason_message: "Skipped".to_string(),
                        warnings_json,
                    }
                }
            }
            Err(err) => map_conversion_error(err, start.elapsed()),
        }
    }
}

enum PlanSingleOutcome {
    Ready(PlannedItem, PathBuf),
    Skipped {
        reason_code: String,
        reason_message: String,
    },
}

fn plan_single_item(
    engine: &ConversionEngine,
    validated: &ValidatedOptions,
    claimed: &ClaimedItem,
    stage_dir_name: &str,
    store: &JobStore,
) -> Result<PlanSingleOutcome, ConversionError> {
    let file = File::open(&claimed.source_path).map_err(|source| ConversionError::Io {
        op: radivine_convert::FsOp::OpenSource,
        path: claimed.source_path.clone(),
        source,
    })?;
    let meta = file.metadata().map_err(|source| ConversionError::Io {
        op: radivine_convert::FsOp::StatSource,
        path: claimed.source_path.clone(),
        source,
    })?;
    if !meta.is_file() {
        return Err(ConversionError::Io {
            op: radivine_convert::FsOp::OpenSource,
            path: claimed.source_path.clone(),
            source: std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "source path is not a regular file",
            ),
        });
    }
    let identity = FileIdentity::from_metadata(&meta);
    let mtime_ns = i128::from(identity.mtime_sec) * 1_000_000_000 + i128::from(identity.mtime_nsec);
    let _ = store.record_source_identity(
        claimed.item_id,
        identity.dev,
        identity.ino,
        identity.size,
        mtime_ns,
    );

    let mut header = [0u8; 16];
    let mut reader = BufReader::new(&file);
    let n = reader
        .read(&mut header)
        .map_err(|source| ConversionError::Io {
            op: radivine_convert::FsOp::ReadSource,
            path: claimed.source_path.clone(),
            source,
        })?;
    let Some(source_format): Option<FormatId> = sniff_format(&header[..n]) else {
        return Err(ConversionError::UnknownInputFormat {
            path: claimed.source_path.clone(),
        });
    };

    if source_format == validated.target() && validated.same_format == SameFormatPolicy::Skip {
        return Ok(PlanSingleOutcome::Skipped {
            reason_code: "already_target_format".to_string(),
            reason_message: format!("Source is already in {} format", source_format),
        });
    }

    let (backend_id, _) = engine
        .catalog()
        .route(
            source_format,
            validated.target(),
            validated.required_features(),
        )
        .map_err(ConversionError::NoRoute)?;

    let (dest_dir, stem, ext) =
        resolve_output_destination_os(&claimed.source_path, validated.target(), &validated.output)?;
    let tentative_output_path = dest_dir.join(derive_suffixed_filename_os(&stem, &ext, 0));
    let _ = stage_dir_name;
    let item_stage_dir_name = format!(".rvstage-{}-{}", claimed.job_id.0, random_hex16());

    Ok(PlanSingleOutcome::Ready(
        PlannedItem {
            item_index: claimed.item_index,
            source_path: claimed.source_path.clone(),
            source_identity: identity,
            source_format,
            target_format: validated.target(),
            backend: backend_id,
            tentative_output_path,
            estimated_source_bytes: identity.size,
            stage_dir_name: item_stage_dir_name,
        },
        dest_dir,
    ))
}

fn map_conversion_error(err: ConversionError, elapsed: Duration) -> ItemExecutionOutcome {
    let duration_ms = elapsed.as_millis() as u64;
    if matches!(err, ConversionError::Cancelled) {
        return ItemExecutionOutcome::Cancelled { duration_ms };
    }
    let timed_out = matches!(
        err,
        ConversionError::LimitExceeded {
            limit: radivine_convert::Limit::Timeout { .. }
        }
    );
    let is_dest_space_or_perm_error = matches!(
        &err,
        ConversionError::Io { op, source, .. }
            if matches!(
                op,
                radivine_convert::FsOp::CreateStageDir
                    | radivine_convert::FsOp::CreateTemp
                    | radivine_convert::FsOp::WriteTemp
                    | radivine_convert::FsOp::CommitOutput
            ) && matches!(
                source.raw_os_error(),
                Some(libc_enospc)
                    if libc_enospc == rustix::io::Errno::NOSPC.raw_os_error()
                        || libc_enospc == rustix::io::Errno::ACCESS.raw_os_error()
                        || libc_enospc == rustix::io::Errno::PERM.raw_os_error()
                        || libc_enospc == rustix::io::Errno::ROFS.raw_os_error()
            )
    );

    ItemExecutionOutcome::Failed {
        error_code: err.code().to_string(),
        error_class: err.class().as_str().to_string(),
        error_message: err.user_summary(),
        retryable: err.is_retryable(),
        duration_ms,
        warnings_json: "[]".to_string(),
        timed_out,
        is_dest_space_or_perm_error,
    }
}

/// Helper signature to satisfy unused-warning checks when building stage callbacks.
pub fn _ignore_stage(_s: ItemState, _st: Stage, _c: JobProgressCounts) {}
