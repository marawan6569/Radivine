//! `radivine-convert` — Pure-Rust, codec-free, desktop-agnostic media conversion core.
//!
//! Enforces `#![forbid(unsafe_code)]` (`ADR-028`) and contains:
//! - Format registry and magic-byte sniffing ([`format`])
//! - Capability descriptors, probing, and deterministic routing ([`backend`])
//! - Conversion options, quality intents, and validation ([`options`])
//! - Persistent `convert.toml` settings and 3-layer preset resolution ([`settings`])
//! - Two-rung atomic no-replace output commit and crash-recovery journaling contracts ([`output`])
//! - Stateless [`engine::ConversionEngine`] and [`engine::ConversionBackend`] trait ([`engine`])
//! - Structured error taxonomy ([`error`])

#![forbid(unsafe_code)]

pub mod backend;
pub mod engine;
pub mod error;
pub mod format;
pub mod header_probe;
pub mod options;
pub mod output;
pub mod settings;

pub use backend::{
    Availability, BackendDescriptor, BackendId, BackendKind, BackendPrefs, Capability,
    CapabilityCatalog, FeatureSet, NoRoute, ProbeEnv, ProbeSpec, TargetStatus, probe_all,
};
pub use engine::{
    BackendOutput, ConversionBackend, ConversionEngine, ConversionPlan, ConvertedItem, ItemControl,
    NoopControl, OutputSink, PlannedItem, Reservation, ResourceEstimate, SkipReason, SourceProbe,
    SourceReader, SourceSpec, Stage, Warning,
};
pub use error::{ConversionError, ErrorClass, FsOp, Limit, UnsupportedFeature};
pub use format::{
    FormatDescriptor, FormatId, FormatTraits, MediaKind, format, format_from_extension,
    format_from_mime, formats, sniff_format,
};
pub use header_probe::{
    header_inspection_count, inspect_file_header, inspect_reader_header, parse_exif_orientation,
    pixel_decode_count, reset_header_inspection_count,
};
pub use options::{
    AlphaPolicy, ChromaSubsampling, CollisionPolicy, ConversionOptions, ExifPolicy, FormatQuality,
    IccPolicy, JpegExt, MetadataPolicy, NamingPolicy, OptionIssue, OrientationPolicy,
    OutputLocation, OutputPolicy, PngCompression, QualityIntent, QualitySetting, ResizeSpec, Rgb8,
    SameFormatPolicy, Severity, ValidatedOptions, WebPMode,
};
pub use output::{
    CommitAttemptError, CommitPermit, CommitRequest, CommitRung, DirSyncRole, FileIdentity, FsOps,
    PendingOutput, RealFsOps, StageCreatedJournal, StageIntentJournal, TempCreatedJournal,
    cleanup_staging_dir_if_empty, cleanup_temp_safely, commit_no_replace, derive_suffixed_filename,
    derive_suffixed_filename_os, is_canonical_stage_dir_name, is_canonical_temp_part_name,
    random_hex16, resolve_output_destination, resolve_output_destination_os,
};
pub use settings::{
    CURRENT_CONVERT_SCHEMA_VERSION, ConvertSettings, Preset, PresetOverrides, PresetRef,
    ResourceLimitsConfig, SettingsWarning, builtin_presets,
};
