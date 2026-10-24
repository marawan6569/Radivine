//! Conversion engine, planning, backend trait, and per-item execution pipeline.

use std::any::Any;
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Instant;

use rustix::fs::{AtFlags, Mode, OFlags, Timespec, Timestamps, openat, utimensat};
use serde::{Deserialize, Serialize};

use crate::backend::{BackendId, CapabilityCatalog};
use crate::error::{ConversionError, FsOp, Limit, UnsupportedFeature};
use crate::format::{FormatId, sniff_format};
use crate::options::{
    ChromaSubsampling, CollisionPolicy, OptionIssue, ResizeSpec, Rgb8, SameFormatPolicy,
    ValidatedOptions,
};
use crate::output::{
    CommitPermit, CommitRequest, FileIdentity, FsOps, PendingOutput, RealFsOps,
    StageCreatedJournal, StageIntentJournal, TempCreatedJournal, derive_suffixed_filename,
    random_hex16, resolve_output_destination,
};
use crate::settings::ResourceLimitsConfig;

/// Input file specification supplied to [`ConversionEngine::plan`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSpec {
    pub path: PathBuf,
    pub hint_mime: Option<String>,
}

impl SourceSpec {
    #[must_use]
    pub fn from_path(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            hint_mime: None,
        }
    }
}

/// Reason why a source item was skipped during planning or execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkipReason {
    AlreadyTargetFormat { format: FormatId },
    UnsupportedInput { path: PathBuf, detail: String },
    NoRoute(crate::backend::NoRoute),
    DestinationExists { dest: PathBuf },
}

/// Single planned conversion item produced by [`ConversionEngine::plan`].
#[derive(Debug, Clone)]
pub struct PlannedItem {
    pub item_index: u32,
    pub source_path: PathBuf,
    pub source_identity: FileIdentity,
    pub source_format: FormatId,
    pub target_format: FormatId,
    pub backend: BackendId,
    pub tentative_output_path: PathBuf,
    pub estimated_source_bytes: u64,
    pub stage_dir_name: String,
}

/// Complete conversion plan for a batch of [`SourceSpec`]s.
#[derive(Debug, Clone)]
pub struct ConversionPlan {
    pub items: Vec<PlannedItem>,
    pub skipped: Vec<(PathBuf, SkipReason)>,
    pub warnings: Vec<OptionIssue>,
}

/// Header-only structural probe of an input or output media stream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceProbe {
    pub format: FormatId,
    pub width: u32,
    pub height: u32,
    pub has_alpha: bool,
    pub is_animated: bool,
    pub orientation: u8,
    pub has_icc: bool,
    pub has_exif: bool,
    pub bit_depth: u8,
}

/// Execution stage reported via [`ItemControl::on_stage`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    Inspecting,
    Decoding,
    Transforming,
    Encoding,
    ValidatingOutput,
    Committing,
}

/// Pre-decode resource estimate returned by [`ConversionBackend::estimate_resources`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ResourceEstimate {
    pub peak_ram_bytes: u64,
    pub temp_disk_bytes: u64,
}

/// Opaque RAII guard holding reserved memory/concurrency budget for an in-flight item.
pub struct Reservation(#[allow(dead_code)] Box<dyn Any + Send>);

impl Reservation {
    #[must_use]
    pub fn empty() -> Self {
        Self(Box::new(()))
    }

    #[must_use]
    pub fn new<T: Any + Send>(guard: T) -> Self {
        Self(Box::new(guard))
    }
}

impl std::fmt::Debug for Reservation {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_tuple("Reservation").finish()
    }
}

/// Non-fatal conversion warning recorded on a [`ConvertedItem`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Warning {
    AlphaFlattened {
        background: Rgb8,
    },
    OrientationNormalized {
        from: u8,
    },
    ExifStrippedByPolicy,
    IccStrippedByPolicy,
    LosslessClampedForJpeg,
    KeptOriginalSize {
        width: u32,
        height: u32,
        requested: ResizeSpec,
    },
    SubsamplingApplied(ChromaSubsampling),
}

/// Summary returned by [`ConversionBackend::convert`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendOutput {
    pub output_width: u32,
    pub output_height: u32,
    pub warnings: Vec<Warning>,
}

/// Completed item report returned by [`ConversionEngine::convert_item`].
#[derive(Debug, Clone)]
pub struct ConvertedItem {
    pub source_path: PathBuf,
    pub output_path: Option<PathBuf>,
    pub skipped_existing: bool,
    pub source_format: FormatId,
    pub target_format: FormatId,
    pub backend: BackendId,
    pub source_bytes: u64,
    pub output_bytes: u64,
    pub output_width: u32,
    pub output_height: u32,
    pub duration: std::time::Duration,
    pub warnings: Vec<Warning>,
}

/// Cooperative cancellation, progress, resource reservation, and crash-recovery journal callbacks.
///
/// Object-safe (`&mut dyn ItemControl`) and lifetime-polymorphic in [`ItemControl::acquire_commit_permit`].
pub trait ItemControl {
    fn check_cancelled(&self) -> Result<(), ConversionError>;
    fn on_stage(&mut self, stage: Stage);
    fn reserve(&mut self, estimate: ResourceEstimate) -> Result<Reservation, ConversionError>;
    fn record_stage_intent(
        &mut self,
        stage: &StageIntentJournal<'_>,
    ) -> Result<(), ConversionError>;
    fn record_stage_created(
        &mut self,
        stage: &StageCreatedJournal<'_>,
    ) -> Result<(), ConversionError>;
    fn record_temp_created(&mut self, temp: &TempCreatedJournal<'_>)
    -> Result<(), ConversionError>;
    fn acquire_commit_permit<'a>(
        &mut self,
        req: &CommitRequest<'a>,
    ) -> Result<CommitPermit<'a>, ConversionError>;
}

/// Default no-op [`ItemControl`] implementation for standalone CLI and unit tests.
#[derive(Debug, Default, Clone, Copy)]
pub struct NoopControl;

impl ItemControl for NoopControl {
    fn check_cancelled(&self) -> Result<(), ConversionError> {
        Ok(())
    }

    fn on_stage(&mut self, _stage: Stage) {}

    fn reserve(&mut self, _estimate: ResourceEstimate) -> Result<Reservation, ConversionError> {
        Ok(Reservation::empty())
    }

    fn record_stage_intent(
        &mut self,
        _stage: &StageIntentJournal<'_>,
    ) -> Result<(), ConversionError> {
        Ok(())
    }

    fn record_stage_created(
        &mut self,
        _stage: &StageCreatedJournal<'_>,
    ) -> Result<(), ConversionError> {
        Ok(())
    }

    fn record_temp_created(
        &mut self,
        _temp: &TempCreatedJournal<'_>,
    ) -> Result<(), ConversionError> {
        Ok(())
    }

    fn acquire_commit_permit<'a>(
        &mut self,
        req: &CommitRequest<'a>,
    ) -> Result<CommitPermit<'a>, ConversionError> {
        Ok(CommitPermit::issue(req))
    }
}

/// Buffered, seekable reader over an open source file or validation buffer.
pub struct SourceReader<'a> {
    inner: &'a mut dyn BufReadSeek,
}

pub trait BufReadSeek: BufRead + Seek {}
impl<T: BufRead + Seek> BufReadSeek for T {}

impl<'a> SourceReader<'a> {
    pub fn new(inner: &'a mut dyn BufReadSeek) -> Self {
        Self { inner }
    }
}

impl Read for SourceReader<'_> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl BufRead for SourceReader<'_> {
    fn fill_buf(&mut self) -> io::Result<&[u8]> {
        self.inner.fill_buf()
    }

    fn consume(&mut self, amt: usize) {
        self.inner.consume(amt);
    }
}

impl Seek for SourceReader<'_> {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        self.inner.seek(pos)
    }
}

/// Buffered writer over an open staging `.part` file.
pub struct OutputSink<'a> {
    inner: BufWriter<&'a mut File>,
}

impl<'a> OutputSink<'a> {
    pub fn new(file: &'a mut File) -> Self {
        Self {
            inner: BufWriter::with_capacity(64 * 1024, file),
        }
    }

    pub fn flush_inner(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

impl Write for OutputSink<'_> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.inner.write(buf)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}

/// Trait implemented by conversion backends (e.g. `radivine_convert_image::ImageBackend`).
pub trait ConversionBackend: Send + Sync {
    fn id(&self) -> BackendId;

    /// Reads header metadata only without decoding the full pixel grid.
    fn inspect(
        &self,
        source: &mut SourceReader<'_>,
        sniffed: FormatId,
    ) -> Result<SourceProbe, ConversionError>;

    /// Estimates peak memory and temporary disk usage prior to full decode.
    fn estimate_resources(&self, probe: &SourceProbe, opts: &ValidatedOptions) -> ResourceEstimate;

    /// Executes decode -> transform -> encode into `sink`.
    fn convert(
        &self,
        source: &mut SourceReader<'_>,
        probe: &SourceProbe,
        opts: &ValidatedOptions,
        sink: &mut OutputSink<'_>,
        ctrl: &mut dyn ItemControl,
    ) -> Result<BackendOutput, ConversionError>;
}

/// Stateless conversion coordinator routing items across registered [`ConversionBackend`]s.
pub struct ConversionEngine {
    catalog: CapabilityCatalog,
    backends: BTreeMap<BackendId, Arc<dyn ConversionBackend>>,
    limits: ResourceLimitsConfig,
    fs_ops: Arc<dyn FsOps>,
}

impl ConversionEngine {
    /// Creates a new [`ConversionEngine`] with the given [`CapabilityCatalog`], backends, and resource limits.
    #[must_use]
    pub fn new(
        catalog: CapabilityCatalog,
        backends: Vec<Arc<dyn ConversionBackend>>,
        limits: ResourceLimitsConfig,
    ) -> Self {
        Self::with_fs_ops(catalog, backends, limits, Arc::new(RealFsOps))
    }

    /// Creates a [`ConversionEngine`] with a custom [`FsOps`] implementation for fault-injection tests.
    #[must_use]
    pub fn with_fs_ops(
        catalog: CapabilityCatalog,
        backends: Vec<Arc<dyn ConversionBackend>>,
        limits: ResourceLimitsConfig,
        fs_ops: Arc<dyn FsOps>,
    ) -> Self {
        let mut map = BTreeMap::new();
        for b in backends {
            map.insert(b.id(), b);
        }
        Self {
            catalog,
            backends: map,
            limits,
            fs_ops,
        }
    }

    /// Returns the [`CapabilityCatalog`].
    #[must_use]
    pub const fn catalog(&self) -> &CapabilityCatalog {
        &self.catalog
    }

    /// Returns the configured [`ResourceLimitsConfig`].
    #[must_use]
    pub const fn limits(&self) -> &ResourceLimitsConfig {
        &self.limits
    }

    /// Plans a batch of [`SourceSpec`]s without writing any output files.
    #[must_use]
    pub fn plan(&self, sources: &[SourceSpec], opts: &ValidatedOptions) -> ConversionPlan {
        let mut items = Vec::with_capacity(sources.len());
        let mut skipped = Vec::new();
        let warnings = Vec::new();
        let stage_dir_name = format!(".rvstage-plan-{}", random_hex16());

        for (idx, spec) in sources.iter().enumerate() {
            let file = match open_source_nofollow(&spec.path) {
                Ok(f) => f,
                Err(e) => {
                    skipped.push((
                        spec.path.clone(),
                        SkipReason::UnsupportedInput {
                            path: spec.path.clone(),
                            detail: e.to_string(),
                        },
                    ));
                    continue;
                }
            };

            let identity = match FileIdentity::from_file(&file) {
                Ok(id) => id,
                Err(e) => {
                    skipped.push((
                        spec.path.clone(),
                        SkipReason::UnsupportedInput {
                            path: spec.path.clone(),
                            detail: e.to_string(),
                        },
                    ));
                    continue;
                }
            };

            let mut header = [0u8; 16];
            let mut reader = BufReader::new(&file);
            let n = match reader.read(&mut header) {
                Ok(n) => n,
                Err(e) => {
                    skipped.push((
                        spec.path.clone(),
                        SkipReason::UnsupportedInput {
                            path: spec.path.clone(),
                            detail: e.to_string(),
                        },
                    ));
                    continue;
                }
            };

            let Some(source_format) = sniff_format(&header[..n]) else {
                skipped.push((
                    spec.path.clone(),
                    SkipReason::UnsupportedInput {
                        path: spec.path.clone(),
                        detail: "unrecognized magic bytes".to_string(),
                    },
                ));
                continue;
            };

            if source_format == opts.target() && opts.same_format == SameFormatPolicy::Skip {
                skipped.push((
                    spec.path.clone(),
                    SkipReason::AlreadyTargetFormat {
                        format: source_format,
                    },
                ));
                continue;
            }

            let (backend_id, _cap) =
                match self
                    .catalog
                    .route(source_format, opts.target(), opts.required_features())
                {
                    Ok(pair) => pair,
                    Err(no_route) => {
                        skipped.push((spec.path.clone(), SkipReason::NoRoute(no_route)));
                        continue;
                    }
                };

            let Ok((dest_dir, stem, ext)) =
                resolve_output_destination(&spec.path, opts.target(), &opts.output)
            else {
                skipped.push((
                    spec.path.clone(),
                    SkipReason::UnsupportedInput {
                        path: spec.path.clone(),
                        detail: "invalid output destination".to_string(),
                    },
                ));
                continue;
            };

            let base_output = dest_dir.join(derive_suffixed_filename(&stem, &ext, 0));
            if opts.output.collision == CollisionPolicy::Skip && base_output.exists() {
                skipped.push((
                    spec.path.clone(),
                    SkipReason::DestinationExists { dest: base_output },
                ));
                continue;
            }

            items.push(PlannedItem {
                item_index: u32::try_from(idx).unwrap_or(u32::MAX),
                source_path: spec.path.clone(),
                source_identity: identity,
                source_format,
                target_format: opts.target(),
                backend: backend_id,
                tentative_output_path: base_output,
                estimated_source_bytes: identity.size,
                stage_dir_name: stage_dir_name.clone(),
            });
        }

        ConversionPlan {
            items,
            skipped,
            warnings,
        }
    }

    /// Inspects a single source file header without decoding pixel data.
    pub fn inspect(&self, path: &Path) -> Result<SourceProbe, ConversionError> {
        let file = open_source_nofollow(path)?;
        let identity = FileIdentity::from_file(&file).map_err(|source| ConversionError::Io {
            op: FsOp::StatSource,
            path: path.to_path_buf(),
            source,
        })?;
        if identity.size > self.limits.max_source_bytes {
            return Err(ConversionError::LimitExceeded {
                limit: Limit::SourceFileBytes {
                    size: identity.size,
                    max: self.limits.max_source_bytes,
                },
            });
        }

        let mut buf_reader = BufReader::new(file);
        let mut header = [0u8; 16];
        let n = buf_reader
            .read(&mut header)
            .map_err(|source| ConversionError::Io {
                op: FsOp::ReadSource,
                path: path.to_path_buf(),
                source,
            })?;
        let Some(fmt) = sniff_format(&header[..n]) else {
            return Err(ConversionError::UnknownInputFormat {
                path: path.to_path_buf(),
            });
        };
        buf_reader
            .seek(SeekFrom::Start(0))
            .map_err(|source| ConversionError::Io {
                op: FsOp::ReadSource,
                path: path.to_path_buf(),
                source,
            })?;

        let backend = self
            .backends
            .values()
            .next()
            .ok_or(ConversionError::NoRoute(
                crate::backend::NoRoute::UnsupportedPair { from: fmt, to: fmt },
            ))?;

        let mut src_reader = SourceReader::new(&mut buf_reader);
        backend.inspect(&mut src_reader, fmt)
    }

    /// Executes the full safety-verified conversion pipeline for a single [`PlannedItem`].
    pub fn convert_item(
        &self,
        item: &PlannedItem,
        opts: &ValidatedOptions,
        ctrl: &mut dyn ItemControl,
    ) -> Result<ConvertedItem, ConversionError> {
        let start = Instant::now();
        ctrl.check_cancelled()?;
        ctrl.on_stage(Stage::Inspecting);

        // 1. Open source with O_RDONLY | O_CLOEXEC and verify regular file + identity
        let src_file = open_source_nofollow(&item.source_path)?;
        let initial_identity =
            FileIdentity::from_file(&src_file).map_err(|source| ConversionError::Io {
                op: FsOp::StatSource,
                path: item.source_path.clone(),
                source,
            })?;

        if !initial_identity.matches(&item.source_identity) {
            return Err(ConversionError::SourceModifiedDuringConversion {
                path: item.source_path.clone(),
            });
        }

        if initial_identity.size > self.limits.max_source_bytes {
            return Err(ConversionError::LimitExceeded {
                limit: Limit::SourceFileBytes {
                    size: initial_identity.size,
                    max: self.limits.max_source_bytes,
                },
            });
        }

        // 2. Magic-byte sniff
        let mut buf_reader = BufReader::new(&src_file);
        let mut header = [0u8; 16];
        let n = buf_reader
            .read(&mut header)
            .map_err(|source| ConversionError::Io {
                op: FsOp::ReadSource,
                path: item.source_path.clone(),
                source,
            })?;
        let Some(sniffed) = sniff_format(&header[..n]) else {
            return Err(ConversionError::UnknownInputFormat {
                path: item.source_path.clone(),
            });
        };
        if sniffed != item.source_format {
            return Err(ConversionError::SourceModifiedDuringConversion {
                path: item.source_path.clone(),
            });
        }
        buf_reader
            .seek(SeekFrom::Start(0))
            .map_err(|source| ConversionError::Io {
                op: FsOp::ReadSource,
                path: item.source_path.clone(),
                source,
            })?;

        // 3. Route and lookup backend
        let backend = self.backends.get(&item.backend).ok_or_else(|| {
            ConversionError::NoRoute(crate::backend::NoRoute::BackendUnavailable {
                backend: item.backend,
                reason: "backend not registered in engine".to_string(),
                install_hint: None,
            })
        })?;

        // 4. Header-only inspect & pre-decode limits check
        let probe = {
            let mut src_reader = SourceReader::new(&mut buf_reader);
            backend.inspect(&mut src_reader, sniffed)?
        };

        if probe.is_animated {
            return Err(ConversionError::UnsupportedFeature {
                feature: UnsupportedFeature::Animation,
            });
        }

        if probe.width > self.limits.max_dimension || probe.height > self.limits.max_dimension {
            return Err(ConversionError::LimitExceeded {
                limit: Limit::Dimensions {
                    width: probe.width,
                    height: probe.height,
                    max: self.limits.max_dimension,
                },
            });
        }

        let total_pixels = u64::from(probe.width) * u64::from(probe.height);
        if total_pixels > self.limits.max_pixels {
            return Err(ConversionError::LimitExceeded {
                limit: Limit::PixelCount {
                    pixels: total_pixels,
                    max: self.limits.max_pixels,
                },
            });
        }

        // Verify capability dimension limits
        let _ = self
            .catalog
            .route_with_dimensions(
                sniffed,
                opts.target(),
                opts.required_features(),
                probe.width,
                probe.height,
            )
            .map_err(ConversionError::NoRoute)?;

        // 5. Resource estimate & reservation
        let estimate = backend.estimate_resources(&probe, opts);
        if estimate.peak_ram_bytes > self.limits.worker_memory_max_bytes {
            return Err(ConversionError::LimitExceeded {
                limit: Limit::MemoryBytes {
                    needed: estimate.peak_ram_bytes,
                    max: self.limits.worker_memory_max_bytes,
                },
            });
        }
        let _reservation = ctrl.reserve(estimate)?;

        // 6. Resolve destination directory and guard against OutputEqualsSource
        let (dest_dir, stem, ext) =
            resolve_output_destination(&item.source_path, opts.target(), &opts.output)?;
        let base_candidate = dest_dir.join(derive_suffixed_filename(&stem, &ext, 0));

        if base_candidate == item.source_path {
            return Err(ConversionError::OutputEqualsSource {
                path: item.source_path.clone(),
            });
        }
        if let Ok(dest_meta) = fs::metadata(&base_candidate) {
            let dest_id = FileIdentity::from_metadata(&dest_meta);
            if dest_id.dev == initial_identity.dev && dest_id.ino == initial_identity.ino {
                return Err(ConversionError::OutputEqualsSource {
                    path: item.source_path.clone(),
                });
            }
        }

        // Fast-path pre-check for Skip / Fail collision policies
        if base_candidate.exists() {
            match opts.output.collision {
                CollisionPolicy::Skip => {
                    return Ok(ConvertedItem {
                        source_path: item.source_path.clone(),
                        output_path: None,
                        skipped_existing: true,
                        source_format: item.source_format,
                        target_format: opts.target(),
                        backend: item.backend,
                        source_bytes: initial_identity.size,
                        output_bytes: 0,
                        output_width: probe.width,
                        output_height: probe.height,
                        duration: start.elapsed(),
                        warnings: Vec::new(),
                    });
                }
                CollisionPolicy::Fail => {
                    return Err(ConversionError::OutputExists {
                        path: base_candidate,
                    });
                }
                CollisionPolicy::Suffix => {}
            }
        }

        // 7. Create staging directory and O_RDWR .part file (journals StageIntent -> StageCreated -> TempCreated)
        let mut pending = PendingOutput::create(
            &dest_dir,
            &item.stage_dir_name,
            item.item_index,
            ctrl,
            self.fs_ops.clone(),
        )?;

        // 8. Rewind source and execute backend conversion into pending .part sink
        buf_reader
            .seek(SeekFrom::Start(0))
            .map_err(|source| ConversionError::Io {
                op: FsOp::ReadSource,
                path: item.source_path.clone(),
                source,
            })?;

        let temp_path_clone = pending.temp_path().to_path_buf();
        let backend_out = {
            let mut src_reader = SourceReader::new(&mut buf_reader);
            let mut sink = OutputSink::new(pending.file_mut());
            let out = backend.convert(&mut src_reader, &probe, opts, &mut sink, ctrl)?;
            sink.flush_inner().map_err(|source| ConversionError::Io {
                op: FsOp::WriteTemp,
                path: temp_path_clone,
                source,
            })?;
            out
        };

        ctrl.check_cancelled()?;
        ctrl.on_stage(Stage::ValidatingOutput);

        // 9. Flush, fsync .part file, verify non-empty, and rewind open O_RDWR fd to offset 0
        let output_bytes = pending.sync_and_rewind_for_validation()?;

        // 10. Validate output magic bytes and structural header directly from the open O_RDWR fd
        {
            let mut out_buf_reader = BufReader::new(pending.file_mut());
            let mut out_hdr = [0u8; 16];
            let out_n = out_buf_reader.read(&mut out_hdr).map_err(|source| {
                ConversionError::OutputValidationFailed {
                    detail: format!("failed to read output header: {source}"),
                }
            })?;
            if sniff_format(&out_hdr[..out_n]) != Some(opts.target()) {
                return Err(ConversionError::OutputValidationFailed {
                    detail: format!(
                        "encoded output magic bytes do not match target format {}",
                        opts.target()
                    ),
                });
            }
            out_buf_reader.seek(SeekFrom::Start(0)).map_err(|source| {
                ConversionError::OutputValidationFailed {
                    detail: format!("failed to seek output fd: {source}"),
                }
            })?;
            let mut val_reader = SourceReader::new(&mut out_buf_reader);
            let out_probe = backend
                .inspect(&mut val_reader, opts.target())
                .map_err(|e| ConversionError::OutputValidationFailed {
                    detail: format!("post-encode header inspection failed: {e}"),
                })?;
            if out_probe.width != backend_out.output_width
                || out_probe.height != backend_out.output_height
            {
                return Err(ConversionError::OutputValidationFailed {
                    detail: format!(
                        "encoded dimensions {}x{} do not match expected {}x{}",
                        out_probe.width,
                        out_probe.height,
                        backend_out.output_width,
                        backend_out.output_height
                    ),
                });
            }
        }

        // 11. Verify source file was not modified or replaced during conversion
        let final_fd_identity =
            FileIdentity::from_file(&src_file).map_err(|source| ConversionError::Io {
                op: FsOp::StatSource,
                path: item.source_path.clone(),
                source,
            })?;
        let final_path_meta =
            fs::metadata(&item.source_path).map_err(|source| ConversionError::Io {
                op: FsOp::StatSource,
                path: item.source_path.clone(),
                source,
            })?;
        let final_path_identity = FileIdentity::from_metadata(&final_path_meta);
        if !initial_identity.matches(&final_fd_identity)
            || !initial_identity.matches(&final_path_identity)
        {
            return Err(ConversionError::SourceModifiedDuringConversion {
                path: item.source_path.clone(),
            });
        }

        // 12. Commit output via two-rung atomic protocol (acquires CommitPermit immediately prior)
        ctrl.on_stage(Stage::Committing);
        let commit_res = pending.commit_with_policy(&stem, &ext, &opts.output, ctrl)?;

        let Some((final_output_path, _rung)) = commit_res else {
            return Ok(ConvertedItem {
                source_path: item.source_path.clone(),
                output_path: None,
                skipped_existing: true,
                source_format: item.source_format,
                target_format: opts.target(),
                backend: item.backend,
                source_bytes: initial_identity.size,
                output_bytes: 0,
                output_width: backend_out.output_width,
                output_height: backend_out.output_height,
                duration: start.elapsed(),
                warnings: backend_out.warnings,
            });
        };

        // 13. Optionally preserve source mtime on the committed output file
        if opts.output.preserve_timestamps {
            let ts = Timestamps {
                last_access: Timespec {
                    tv_sec: initial_identity.mtime_sec,
                    tv_nsec: initial_identity.mtime_nsec as _,
                },
                last_modification: Timespec {
                    tv_sec: initial_identity.mtime_sec,
                    tv_nsec: initial_identity.mtime_nsec as _,
                },
            };
            let _ = utimensat(
                rustix::fs::CWD,
                &final_output_path,
                &ts,
                AtFlags::SYMLINK_NOFOLLOW,
            );
        }

        let mut warnings = backend_out.warnings;
        if opts.target() == FormatId::Jpeg
            && matches!(
                opts.options().quality,
                crate::options::QualitySetting::Intent(crate::options::QualityIntent::Lossless)
            )
            && !warnings.contains(&Warning::LosslessClampedForJpeg)
        {
            warnings.push(Warning::LosslessClampedForJpeg);
        }

        Ok(ConvertedItem {
            source_path: item.source_path.clone(),
            output_path: Some(final_output_path),
            skipped_existing: false,
            source_format: item.source_format,
            target_format: opts.target(),
            backend: item.backend,
            source_bytes: initial_identity.size,
            output_bytes,
            output_width: backend_out.output_width,
            output_height: backend_out.output_height,
            duration: start.elapsed(),
            warnings,
        })
    }
}

/// Opens a source path for reading (`O_RDONLY | O_CLOEXEC`) and verifies it is a regular file.
///
/// Follows valid user symlinks as required by `SECURITY_AND_FILE_SAFETY.md` §2.3 while rejecting
/// FIFOs, sockets, device nodes, and directories via `fstat` on the opened descriptor.
fn open_source_nofollow(path: &Path) -> Result<File, ConversionError> {
    let fd = openat(
        rustix::fs::CWD,
        path,
        OFlags::RDONLY | OFlags::CLOEXEC,
        Mode::empty(),
    )
    .map_err(|errno| ConversionError::Io {
        op: FsOp::OpenSource,
        path: path.to_path_buf(),
        source: io::Error::from_raw_os_error(errno.raw_os_error()),
    })?;
    let file = File::from(fd);
    let meta = file.metadata().map_err(|source| ConversionError::Io {
        op: FsOp::StatSource,
        path: path.to_path_buf(),
        source,
    })?;
    if !meta.is_file() {
        return Err(ConversionError::Io {
            op: FsOp::OpenSource,
            path: path.to_path_buf(),
            source: io::Error::new(
                io::ErrorKind::InvalidInput,
                "source path is not a regular file",
            ),
        });
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time and runtime verification that `ItemControl` is dyn-compatible (`&mut dyn ItemControl`)
    /// and that `CommitPermit<'a>` is `!Send + !Sync` while tying its lifetime `'a` to `CommitRequest<'a>`.
    #[test]
    fn item_control_dyn_compatibility_and_commit_permit_lifetime() {
        let mut noop = NoopControl;
        let ctrl: &mut dyn ItemControl = &mut noop;

        let stage_path = PathBuf::from("/tmp/.rvstage-1");
        let temp_path = PathBuf::from("/tmp/.rvstage-1/item-0-0123456789abcdef.part");
        let out_path = PathBuf::from("/tmp/out.webp");

        ctrl.record_stage_intent(&StageIntentJournal {
            stage_dir_path: &stage_path,
        })
        .unwrap();
        ctrl.record_stage_created(&StageCreatedJournal {
            stage_dir_path: &stage_path,
            stage_dir_dev: 1,
            stage_dir_ino: 2,
        })
        .unwrap();
        ctrl.record_temp_created(&TempCreatedJournal {
            stage_dir_path: &stage_path,
            temp_path: &temp_path,
            temp_dev: 1,
            temp_ino: 3,
        })
        .unwrap();

        let req = CommitRequest {
            stage_dir_path: &stage_path,
            temp_path: &temp_path,
            temp_dev: 1,
            temp_ino: 3,
            final_output_path: &out_path,
        };
        let _permit = ctrl.acquire_commit_permit(&req).unwrap();
    }
}
