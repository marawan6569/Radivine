//! Standalone `radivine-control convert` window (`CONVERSION_UX.md` §5, `AC-C4.5`).
//!
//! - Invoked via `radivine-control convert [--stdin] [PATHS…]`.
//! - Bypasses `ControlInstanceLock::acquire_or_notify_existing()` (`ADR-C15`) so multiple
//!   Advanced Convert windows can coexist with each other and with the main Control Center.
//! - Performs asynchronous header-only inspection on a background thread (`inspect_file_header`,
//!   reading `<= 64 KiB` per file for up to `1,000` files and summarizing any remaining paths by
//!   extension). **Never decodes pixels or generates thumbnails** (`AC-C4.5`).
//! - Performs live `ConversionOptions` validation and debounced asynchronous destination checks.
//! - Submits a durable job with inline `ConversionOptions` and `Origin::Advanced`.

use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::BufRead;
use std::num::NonZeroU32;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::str::FromStr;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui::{
    self, Align, Color32, CornerRadius, FontFamily, FontId, Key, Layout, Margin, RichText,
    ScrollArea, Stroke, Vec2,
};
use radivine::config::AppConfig;
use radivine_convert::{
    AlphaPolicy, ChromaSubsampling, CollisionPolicy, ConversionOptions, ConvertSettings,
    ExifPolicy, FormatId, FormatQuality, IccPolicy, MetadataPolicy, NamingPolicy,
    OrientationPolicy, OutputLocation, OutputPolicy, PngCompression, Preset, PresetOverrides,
    QualityIntent, QualitySetting, ResizeSpec, ResourceLimitsConfig, Rgb8, SameFormatPolicy,
    WebPMode, builtin_presets, format_from_extension, inspect_file_header,
};
use radivine_jobs::{CONVERT_IMAGE_KIND, JobId, JobsClient, Origin, RequestId, WirePath};

use crate::persistence::{
    DiskFileSnapshot, apply_convert_settings_doc, atomic_write_with_backup, convert_path_for,
};
use crate::service::ServicePaths;
use crate::ui::theme::{
    BrandPalette, EMBEDDED_APP_ICON_PNG, apply_palette_visuals, decode_png_to_icon_data,
    install_inter_font, section_card,
};

/// Maximum number of paths read from `--stdin` (`SECURITY_AND_FILE_SAFETY.md` §2.5).
pub const MAX_STDIN_LINES: usize = 10_000;
/// Maximum byte length of a single `--stdin` line (`16 KiB`).
pub const MAX_STDIN_LINE_BYTES: usize = 16 * 1024;
/// Maximum cumulative bytes read from `--stdin` (`16 MiB`).
pub const MAX_STDIN_TOTAL_BYTES: usize = 16 * 1024 * 1024;
/// Maximum number of files inspected via header magic bytes before falling back to extension summary.
pub const MAX_EAGER_HEADER_PROBES: usize = 1_000;

static PIXEL_DECODE_CALL_COUNT: AtomicU64 = AtomicU64::new(0);

/// Returns the number of full pixel decode calls performed inside `radivine-control` (always `0`, `AC-C4.5`).
#[must_use]
pub fn pixel_decode_count() -> u64 {
    PIXEL_DECODE_CALL_COUNT.load(Ordering::Relaxed)
}

/// Parsed CLI arguments for `radivine-control convert [--stdin] [PATHS…]`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AdvancedConvertCliArgs {
    /// Read newline-separated `WirePath` entries from `stdin`.
    pub read_stdin: bool,
    /// Optional explicit `--config <PATH>` override.
    pub config_path: Option<PathBuf>,
    /// Positional path arguments.
    pub paths: Vec<WirePath>,
}

/// Parses arguments following `radivine-control convert`.
pub fn parse_convert_cli_args<I, S>(iter: I) -> Result<AdvancedConvertCliArgs, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut out = AdvancedConvertCliArgs::default();
    let args: Vec<_> = iter
        .into_iter()
        .map(|s| s.as_ref().to_os_string())
        .collect();
    let mut idx = 0;
    let mut positional_only = false;

    while idx < args.len() {
        let arg = &args[idx];
        if !positional_only {
            if arg == "--" {
                positional_only = true;
                idx += 1;
                continue;
            }
            if arg == "--stdin" {
                out.read_stdin = true;
                idx += 1;
                continue;
            }
            if arg == "--config" {
                idx += 1;
                let Some(val) = args.get(idx) else {
                    return Err("Missing path value after --config".to_string());
                };
                out.config_path = Some(PathBuf::from(val));
                idx += 1;
                continue;
            }
        }
        let wp = parse_arg_to_wire_path(arg)?;
        out.paths.push(wp);
        idx += 1;
    }

    Ok(out)
}

fn parse_arg_to_wire_path(arg: &OsStr) -> Result<WirePath, String> {
    let raw = arg.as_bytes();
    if let Ok(s) = std::str::from_utf8(raw) {
        let trimmed = s.trim();
        if trimmed.starts_with("b64:") {
            return WirePath::parse_encoded(trimmed).map_err(|e| e.to_string());
        }
        if let Some(rest) = trimmed.strip_prefix("file://") {
            let decoded = percent_decode_bytes(rest.as_bytes());
            return WirePath::from_raw_bytes(&decoded).map_err(|e| e.to_string());
        }
    }
    let path = Path::new(arg);
    if path.is_absolute() {
        WirePath::from_raw_bytes(raw).map_err(|e| e.to_string())
    } else {
        let abs = std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path);
        WirePath::from_path(&abs).map_err(|e| e.to_string())
    }
}

fn percent_decode_bytes(input: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%'
            && i + 2 < input.len()
            && let (Some(h), Some(l)) = (hex_val(input[i + 1]), hex_val(input[i + 2]))
        {
            out.push((h << 4) | l);
            i += 3;
            continue;
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

fn hex_val(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

/// Reads newline-separated [`WirePath`] lines from `reader` with strict security bounds:
/// `<= 10,000` lines, `<= 16 KiB` per line, `<= 16 MiB` total (`SECURITY_AND_FILE_SAFETY.md` §2.5).
pub fn read_bounded_stdin_paths<R: BufRead>(mut reader: R) -> Result<Vec<WirePath>, String> {
    let mut paths = Vec::new();
    let mut total_bytes = 0usize;
    let mut buf = Vec::new();

    loop {
        buf.clear();
        let bytes_read = reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| format!("Failed reading stdin: {e}"))?;
        if bytes_read == 0 {
            break;
        }
        total_bytes = total_bytes.saturating_add(bytes_read);
        if total_bytes > MAX_STDIN_TOTAL_BYTES {
            return Err(format!(
                "Stdin input exceeded maximum total size ({MAX_STDIN_TOTAL_BYTES} bytes)"
            ));
        }
        if bytes_read > MAX_STDIN_LINE_BYTES {
            return Err(format!(
                "Stdin path line exceeded maximum length ({MAX_STDIN_LINE_BYTES} bytes)"
            ));
        }
        if buf.ends_with(b"\n") {
            buf.pop();
            if buf.ends_with(b"\r") {
                buf.pop();
            }
        }
        if buf.is_empty() {
            continue;
        }
        if paths.len() >= MAX_STDIN_LINES {
            return Err(format!(
                "Selection exceeded maximum allowed items ({MAX_STDIN_LINES})"
            ));
        }

        let wp = if let Ok(s) = std::str::from_utf8(&buf) {
            if s.starts_with("b64:") {
                WirePath::parse_encoded(s).map_err(|e| e.to_string())?
            } else if let Some(rest) = s.strip_prefix("file://") {
                let decoded = percent_decode_bytes(rest.as_bytes());
                WirePath::from_raw_bytes(&decoded).map_err(|e| e.to_string())?
            } else {
                WirePath::from_raw_bytes(&buf).map_err(|e| e.to_string())?
            }
        } else {
            WirePath::from_raw_bytes(&buf).map_err(|e| e.to_string())?
        };
        paths.push(wp);
    }

    Ok(paths)
}

/// Summary of header-only inspection for the selected files (`CONVERSION_UX.md` §5.3).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SelectionInspectionSummary {
    /// Total paths passed to the window.
    pub total_selected: usize,
    /// Number of valid supported image files.
    pub supported_count: usize,
    /// Breakdown of supported formats (`FormatId -> count`).
    pub format_counts: BTreeMap<FormatId, usize>,
    /// Number of inspected files that contain an alpha channel (`has_alpha == true`).
    pub alpha_count: usize,
    /// Minimum `(width, height)` observed across header-probed files.
    pub min_dimensions: Option<(u32, u32)>,
    /// Maximum `(width, height)` observed across header-probed files.
    pub max_dimensions: Option<(u32, u32)>,
    /// Unsupported or unreadable items `(display_name, reason)`.
    pub unsupported_items: Vec<(String, String)>,
    /// Common parent directory if all items share one, or the first item's parent directory.
    pub primary_parent_dir: Option<PathBuf>,
    /// Whether `total_selected > MAX_EAGER_HEADER_PROBES` and remaining items were summarized by extension.
    pub truncated_header_probe: bool,
}

impl SelectionInspectionSummary {
    /// Formats a human-readable header summary line (e.g. `"24 images · 18 PNG, 6 WEBP · 512×512 – 3840×2160"`).
    #[must_use]
    pub fn headline_text(&self) -> String {
        if self.total_selected == 0 {
            return "No files selected".to_string();
        }
        let img_word = if self.supported_count == 1 {
            "image"
        } else {
            "images"
        };
        let mut parts = vec![format!("{} {img_word}", self.supported_count)];

        if !self.format_counts.is_empty() {
            let fmt_list: Vec<String> = self
                .format_counts
                .iter()
                .map(|(fmt, cnt)| format!("{cnt} {}", fmt.as_str().to_ascii_uppercase()))
                .collect();
            parts.push(fmt_list.join(", "));
        }

        if let (Some((min_w, min_h)), Some((max_w, max_h))) =
            (self.min_dimensions, self.max_dimensions)
        {
            if (min_w, min_h) == (max_w, max_h) {
                parts.push(format!("{min_w}×{min_h}"));
            } else {
                parts.push(format!("{min_w}×{min_h} – {max_w}×{max_h}"));
            }
        }

        if self.alpha_count > 0 {
            parts.push(format!("{} with transparency", self.alpha_count));
        }

        parts.join(" · ")
    }
}

/// Inspects up to [`MAX_EAGER_HEADER_PROBES`] (`1,000`) files using header-only probing
/// (`inspect_file_header`, `<= 64 KiB` read per file) and summarizes any remaining items by
/// file extension (`CONVERSION_UX.md` §5.3, `AC-C4.5`).
#[must_use]
pub fn inspect_selection_headers(
    paths: &[WirePath],
    limits: &ResourceLimitsConfig,
) -> SelectionInspectionSummary {
    let mut summary = SelectionInspectionSummary {
        total_selected: paths.len(),
        truncated_header_probe: paths.len() > MAX_EAGER_HEADER_PROBES,
        ..SelectionInspectionSummary::default()
    };

    for (idx, wp) in paths.iter().enumerate() {
        let Ok(path_buf) = wp.to_path_buf() else {
            summary
                .unsupported_items
                .push((wp.lossy_display(), "Invalid path encoding".to_string()));
            continue;
        };

        if summary.primary_parent_dir.is_none() {
            summary.primary_parent_dir = path_buf.parent().map(Path::to_path_buf);
        }

        let display_name = path_buf
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| wp.lossy_display());

        if idx < MAX_EAGER_HEADER_PROBES {
            match inspect_file_header(&path_buf, limits) {
                Ok(info) => {
                    summary.supported_count += 1;
                    *summary.format_counts.entry(info.format).or_insert(0) += 1;
                    if info.has_alpha {
                        summary.alpha_count += 1;
                    }
                    let dims = (info.width, info.height);
                    summary.min_dimensions = Some(match summary.min_dimensions {
                        Some((w, h)) => (w.min(dims.0), h.min(dims.1)),
                        None => dims,
                    });
                    summary.max_dimensions = Some(match summary.max_dimensions {
                        Some((w, h)) => (w.max(dims.0), h.max(dims.1)),
                        None => dims,
                    });
                }
                Err(err) => {
                    summary
                        .unsupported_items
                        .push((display_name, err.to_string()));
                }
            }
        } else {
            let ext = path_buf
                .extension()
                .and_then(|e| e.to_str())
                .unwrap_or_default();
            if let Some(fmt) = format_from_extension(ext) {
                summary.supported_count += 1;
                *summary.format_counts.entry(fmt).or_insert(0) += 1;
            } else {
                summary.unsupported_items.push((
                    display_name,
                    "Unsupported file extension (not header-probed beyond 1,000 items)".to_string(),
                ));
            }
        }
    }

    summary
}

/// Resize mode selector in the Advanced Convert window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiResizeMode {
    /// Keep original pixel dimensions.
    Original,
    /// Constrain width (`px`), scaling height proportionally.
    Width,
    /// Constrain height (`px`), scaling width proportionally.
    Height,
    /// Fit within `width × height` bounding box.
    FitWithin,
}

/// Destination mode selector in the Advanced Convert window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiDestMode {
    /// Save next to source files.
    SameFolder,
    /// Save in a relative subfolder under each source's parent folder.
    Subfolder,
    /// Save in an explicit absolute folder path.
    CustomFolder,
}

enum BgTaskMessage {
    Inspection(SelectionInspectionSummary),
    DestProbe {
        probe_seq: u64,
        result: Result<(), String>,
    },
    Submitted(Result<(JobId, bool), String>),
}

/// Interactive state for the standalone `radivine-control convert` window (`AC-C4.5`).
pub struct AdvancedConvertApp {
    /// Selected input paths (`WirePath` preserves non-UTF-8 Linux filenames losslessly).
    pub input_paths: Vec<WirePath>,
    /// Loaded `convert.toml` settings.
    pub settings: ConvertSettings,
    convert_path: PathBuf,
    service_paths: ServicePaths,
    is_dark_theme: bool,
    /// Header inspection summary (`None` while background inspection is still running).
    pub inspection: Option<SelectionInspectionSummary>,
    /// Currently selected target format (`PNG`, `JPEG`, `WebP`).
    pub target_format: FormatId,
    /// Currently selected preset ID (`"custom"` when manually modified).
    pub selected_preset_id: String,
    /// Lossy quality slider (`1..=100`) for JPEG and lossy WebP.
    pub quality_value: u8,
    /// WebP encoding mode (`Lossy` vs `Lossless`).
    pub webp_lossless: bool,
    /// PNG compression level (`Fast`, `Default`, `Best`).
    pub png_compression: PngCompression,
    /// JPEG chroma subsampling (`Auto`, `4:4:4`, `4:2:0`).
    pub jpeg_subsampling: ChromaSubsampling,
    /// Resize mode.
    pub resize_mode: UiResizeMode,
    /// Target width text field.
    pub resize_width_text: String,
    /// Target height text field.
    pub resize_height_text: String,
    /// Allow upscaling smaller images (`false` by default per `CONVERSION_UX.md` §5.4).
    pub allow_upscale: bool,
    /// Background color (`#RRGGBB`) when flattening alpha onto JPEG.
    pub background_hex: String,
    /// Auto-orient from EXIF orientation tag.
    pub auto_orient: bool,
    /// Preserve ICC color profile.
    pub keep_icc: bool,
    /// Strip EXIF metadata (`true` = `ExifPolicy::Strip`, `false` = `ExifPolicy::PreserveAll`).
    pub strip_exif: bool,
    /// Output destination mode.
    pub dest_mode: UiDestMode,
    /// Subfolder name when `dest_mode == UiDestMode::Subfolder`.
    pub dest_subfolder_text: String,
    /// Custom folder path when `dest_mode == UiDestMode::CustomFolder`.
    pub dest_folder_text: String,
    /// Append suffix to output stem.
    pub append_suffix: bool,
    /// Filename suffix text (e.g. `"-converted"`).
    pub suffix_text: String,
    /// Collision handling policy.
    pub collision_policy: CollisionPolicy,
    /// Preserve source modification timestamp.
    pub preserve_timestamps: bool,
    /// Inline `"Save as preset…"` modal open state.
    pub save_preset_open: bool,
    /// New preset name input.
    pub save_preset_name: String,
    /// Inline error for `"Save as preset…"`.
    pub save_preset_error: Option<String>,
    /// Inline status message after saving a preset.
    pub save_preset_ok: Option<String>,
    /// Live validation errors from `ConversionOptions` and form inputs.
    pub validation_errors: Vec<String>,
    /// Asynchronous destination writability error (`None` if valid).
    pub dest_probe_error: Option<String>,
    dest_probe_seq: u64,
    last_dest_edit: Option<Instant>,
    dest_probe_pending: bool,
    /// Whether a job submission is currently in flight.
    pub submitting: bool,
    /// Submission error banner (`AC-C4.5`: stays open on failure with error banner).
    pub submit_error: Option<String>,
    /// Submitted job ID (`Some(job_id)` once accepted by the worker).
    pub submitted_job_id: Option<JobId>,
    /// Whether the user requested closing the window (`Cancel` / `Esc`).
    pub should_close: bool,
    bg_tx: Sender<BgTaskMessage>,
    bg_rx: Receiver<BgTaskMessage>,
    egui_ctx: Option<egui::Context>,
    fonts_installed: bool,
}

impl AdvancedConvertApp {
    /// Creates a new [`AdvancedConvertApp`] and starts non-blocking background header inspection.
    #[must_use]
    pub fn new(
        input_paths: Vec<WirePath>,
        service_paths: ServicePaths,
        egui_ctx: Option<egui::Context>,
    ) -> Self {
        let config_path = service_paths
            .config_path
            .clone()
            .unwrap_or_else(AppConfig::default_path);
        let app_cfg = AppConfig::load_or_default(Some(&config_path));
        let is_dark_theme = !matches!(
            app_cfg
                .appearance
                .theme
                .trim()
                .to_ascii_lowercase()
                .as_str(),
            "light"
        );

        let convert_path = convert_path_for(&config_path);
        let (settings, _) = ConvertSettings::load_or_default(&convert_path);

        let (bg_tx, bg_rx) = mpsc::channel();

        // Spawn asynchronous header inspection thread (`<= 1,000` files, zero pixel decode)
        let paths_for_probe = input_paths.clone();
        let limits_for_probe = settings.limits.clone();
        let probe_tx = bg_tx.clone();
        let probe_ctx = egui_ctx.clone();
        let _ = thread::Builder::new()
            .name("radivine-convert-header-probe".to_string())
            .spawn(move || {
                let summary = inspect_selection_headers(&paths_for_probe, &limits_for_probe);
                let _ = probe_tx.send(BgTaskMessage::Inspection(summary));
                if let Some(ctx) = probe_ctx {
                    ctx.request_repaint();
                }
            });

        let (dest_mode, dest_subfolder_text, dest_folder_text) =
            match &settings.defaults.output.location {
                OutputLocation::SameDirectory => (
                    UiDestMode::SameFolder,
                    "Converted".to_string(),
                    String::new(),
                ),
                OutputLocation::Subfolder(sub) => {
                    (UiDestMode::Subfolder, sub.clone(), String::new())
                }
                OutputLocation::Directory(dir) => (
                    UiDestMode::CustomFolder,
                    "Converted".to_string(),
                    dir.to_string_lossy().into_owned(),
                ),
            };

        let (append_suffix, suffix_text) = match &settings.defaults.output.naming {
            NamingPolicy::PreserveStem { .. } => (false, "-converted".to_string()),
            NamingPolicy::AppendSuffix { suffix, .. } => (true, suffix.clone()),
        };

        let mut app = Self {
            input_paths,
            target_format: settings.quick_convert_target,
            selected_preset_id: settings.default_preset.clone(),
            quality_value: 82,
            webp_lossless: false,
            png_compression: PngCompression::Default,
            jpeg_subsampling: ChromaSubsampling::Auto,
            resize_mode: UiResizeMode::Original,
            resize_width_text: "1920".to_string(),
            resize_height_text: "1080".to_string(),
            allow_upscale: false,
            background_hex: settings.defaults.alpha.background.to_hex(),
            auto_orient: settings.defaults.metadata.orientation == OrientationPolicy::AutoOrient,
            keep_icc: settings.defaults.metadata.icc == IccPolicy::Preserve,
            strip_exif: settings.defaults.metadata.exif == ExifPolicy::Strip,
            dest_mode,
            dest_subfolder_text,
            dest_folder_text,
            append_suffix,
            suffix_text,
            collision_policy: settings.defaults.output.collision,
            preserve_timestamps: settings.defaults.output.preserve_timestamps,
            save_preset_open: false,
            save_preset_name: String::new(),
            save_preset_error: None,
            save_preset_ok: None,
            validation_errors: Vec::new(),
            dest_probe_error: None,
            dest_probe_seq: 0,
            last_dest_edit: None,
            dest_probe_pending: false,
            submitting: false,
            submit_error: None,
            submitted_job_id: None,
            should_close: false,
            settings,
            convert_path,
            service_paths,
            is_dark_theme,
            inspection: None,
            bg_tx,
            bg_rx,
            egui_ctx,
            fonts_installed: false,
        };

        let initial_preset = app.selected_preset_id.clone();
        app.apply_preset_by_id(&initial_preset);
        app.recompute_validation();
        app
    }

    /// Returns all available presets (built-in + user-defined).
    #[must_use]
    pub fn all_presets(&self) -> Vec<Preset> {
        let mut list = builtin_presets();
        list.extend(self.settings.user_presets.iter().cloned());
        list
    }

    /// Applies a preset by ID (`CONVERSION_UX.md` §5.4).
    pub fn apply_preset_by_id(&mut self, preset_id: &str) {
        let Some(preset) = self.all_presets().into_iter().find(|p| p.id == preset_id) else {
            return;
        };
        self.selected_preset_id = preset.id.clone();
        self.target_format = preset.target;
        self.apply_overrides(&preset.overrides);
        self.recompute_validation();
    }

    /// Selects a target format (`PNG`, `JPEG`, `WebP`) and applies its Quick Convert default preset.
    pub fn select_target_format(&mut self, fmt: FormatId) {
        self.target_format = fmt;
        let default_id = self.settings.quick_convert_preset_for(fmt);
        self.apply_preset_by_id(&default_id);
        self.target_format = fmt;
        self.recompute_validation();
    }

    fn apply_overrides(&mut self, ov: &PresetOverrides) {
        if let Some(ref q) = ov.quality {
            match q {
                QualitySetting::Intent(QualityIntent::Lossless) => {
                    self.webp_lossless = true;
                }
                QualitySetting::Intent(QualityIntent::Maximum) => {
                    self.webp_lossless = false;
                    self.quality_value = 92;
                }
                QualitySetting::Intent(QualityIntent::High) => {
                    self.webp_lossless = false;
                    self.quality_value = 86;
                }
                QualitySetting::Intent(QualityIntent::Balanced) => {
                    self.webp_lossless = false;
                    self.quality_value = 82;
                }
                QualitySetting::Intent(QualityIntent::Compact) => {
                    self.webp_lossless = false;
                    self.quality_value = 72;
                }
                QualitySetting::Intent(QualityIntent::Custom(val)) => {
                    self.webp_lossless = false;
                    self.quality_value = (*val).clamp(1, 100);
                }
                QualitySetting::PerFormat(FormatQuality::Png { compression }) => {
                    self.png_compression = *compression;
                }
                QualitySetting::PerFormat(FormatQuality::Jpeg {
                    quality,
                    subsampling,
                    ..
                }) => {
                    self.quality_value = (*quality).clamp(1, 100);
                    self.jpeg_subsampling = *subsampling;
                }
                QualitySetting::PerFormat(FormatQuality::WebP { mode, quality, .. }) => {
                    self.webp_lossless = *mode == WebPMode::Lossless;
                    self.quality_value = (*quality).clamp(1, 100);
                }
            }
        }
        if let Some(ref r) = ov.resize {
            match r {
                ResizeSpec::Original => {
                    self.resize_mode = UiResizeMode::Original;
                }
                ResizeSpec::Width { px, allow_upscale } => {
                    self.resize_mode = UiResizeMode::Width;
                    self.resize_width_text = px.get().to_string();
                    self.allow_upscale = *allow_upscale;
                }
                ResizeSpec::Height { px, allow_upscale } => {
                    self.resize_mode = UiResizeMode::Height;
                    self.resize_height_text = px.get().to_string();
                    self.allow_upscale = *allow_upscale;
                }
                ResizeSpec::Fit {
                    width,
                    height,
                    allow_upscale,
                } => {
                    self.resize_mode = UiResizeMode::FitWithin;
                    self.resize_width_text = width.get().to_string();
                    self.resize_height_text = height.get().to_string();
                    self.allow_upscale = *allow_upscale;
                }
            }
        }
        if let Some(bg) = ov.alpha_background {
            self.background_hex = bg.to_hex();
        }
        if let Some(o) = ov.orientation {
            self.auto_orient = o == OrientationPolicy::AutoOrient;
        }
        if let Some(icc) = ov.icc {
            self.keep_icc = icc == IccPolicy::Preserve;
        }
        if let Some(exif) = ov.exif {
            self.strip_exif = exif == ExifPolicy::Strip;
        }
    }

    /// Builds the [`ConversionOptions`] corresponding to the current UI controls, or returns
    /// human-readable validation error strings.
    pub fn build_conversion_options(&self) -> Result<ConversionOptions, Vec<String>> {
        let mut errors = Vec::new();

        let quality = match self.target_format {
            FormatId::Png => QualitySetting::PerFormat(FormatQuality::Png {
                compression: self.png_compression,
            }),
            FormatId::Jpeg => QualitySetting::PerFormat(FormatQuality::Jpeg {
                quality: self.quality_value.clamp(1, 100),
                progressive: true,
                optimize_coding: true,
                subsampling: self.jpeg_subsampling,
            }),
            FormatId::WebP => QualitySetting::PerFormat(FormatQuality::WebP {
                mode: if self.webp_lossless {
                    WebPMode::Lossless
                } else {
                    WebPMode::Lossy
                },
                quality: self.quality_value.clamp(1, 100),
                alpha_quality: 100,
                method: 4,
            }),
            _ => QualitySetting::Intent(QualityIntent::Balanced),
        };

        let max_dim = self.settings.limits.max_dimension.max(1);
        let resize = match self.resize_mode {
            UiResizeMode::Original => ResizeSpec::Original,
            UiResizeMode::Width => match self.resize_width_text.trim().parse::<u32>() {
                Ok(w) if (1..=max_dim).contains(&w) => ResizeSpec::Width {
                    px: NonZeroU32::new(w).unwrap(),
                    allow_upscale: self.allow_upscale,
                },
                _ => {
                    errors.push(format!("Resize width must be between 1 and {max_dim} px."));
                    ResizeSpec::Original
                }
            },
            UiResizeMode::Height => match self.resize_height_text.trim().parse::<u32>() {
                Ok(h) if (1..=max_dim).contains(&h) => ResizeSpec::Height {
                    px: NonZeroU32::new(h).unwrap(),
                    allow_upscale: self.allow_upscale,
                },
                _ => {
                    errors.push(format!("Resize height must be between 1 and {max_dim} px."));
                    ResizeSpec::Original
                }
            },
            UiResizeMode::FitWithin => {
                let w_res = self.resize_width_text.trim().parse::<u32>();
                let h_res = self.resize_height_text.trim().parse::<u32>();
                match (w_res, h_res) {
                    (Ok(w), Ok(h)) if (1..=max_dim).contains(&w) && (1..=max_dim).contains(&h) => {
                        ResizeSpec::Fit {
                            width: NonZeroU32::new(w).unwrap(),
                            height: NonZeroU32::new(h).unwrap(),
                            allow_upscale: self.allow_upscale,
                        }
                    }
                    _ => {
                        errors.push(format!(
                            "Fit Within width and height must both be between 1 and {max_dim} px."
                        ));
                        ResizeSpec::Original
                    }
                }
            }
        };

        let bg_rgb = match Rgb8::from_str(&self.background_hex) {
            Ok(rgb) => rgb,
            Err(e) => {
                errors.push(e);
                Rgb8::WHITE
            }
        };
        let alpha = AlphaPolicy { background: bg_rgb };

        let location = match self.dest_mode {
            UiDestMode::SameFolder => OutputLocation::SameDirectory,
            UiDestMode::Subfolder => {
                let sub = self.dest_subfolder_text.trim();
                if sub.is_empty()
                    || sub == "."
                    || sub == ".."
                    || sub.contains('/')
                    || sub.contains('\\')
                    || sub.contains('\0')
                {
                    errors.push(
                        "Subfolder must be a single relative folder name (no '/' or '..')."
                            .to_string(),
                    );
                }
                OutputLocation::Subfolder(sub.to_string())
            }
            UiDestMode::CustomFolder => {
                let raw = self.dest_folder_text.trim();
                let p = PathBuf::from(raw);
                if raw.is_empty() || !p.is_absolute() {
                    errors.push("Custom output folder must be an absolute path.".to_string());
                }
                OutputLocation::Directory(p)
            }
        };

        let jpeg_ext = self.settings.defaults.output.naming.jpeg_ext();

        let naming = if self.append_suffix {
            let suf = self.suffix_text.trim();
            if suf.is_empty() || suf.contains('/') || suf.contains('\\') || suf.contains('\0') {
                errors.push(
                    "Filename suffix must be non-empty and cannot contain path separators."
                        .to_string(),
                );
            }
            NamingPolicy::AppendSuffix {
                suffix: suf.to_string(),
                jpeg_ext,
            }
        } else {
            NamingPolicy::PreserveStem { jpeg_ext }
        };

        if !errors.is_empty() {
            return Err(errors);
        }

        // Advanced mode re-encodes same-format sources when requested (`CONVERSION_UX.md` §5.4).
        Ok(ConversionOptions {
            target: self.target_format,
            quality,
            resize,
            alpha,
            metadata: MetadataPolicy {
                orientation: if self.auto_orient {
                    OrientationPolicy::AutoOrient
                } else {
                    OrientationPolicy::PreserveRaw
                },
                icc: if self.keep_icc {
                    IccPolicy::Preserve
                } else {
                    IccPolicy::Strip
                },
                exif: if self.strip_exif {
                    ExifPolicy::Strip
                } else {
                    ExifPolicy::PreserveAll
                },
            },
            output: OutputPolicy {
                location,
                naming,
                collision: self.collision_policy,
                max_collision_probes: 999,
                preserve_timestamps: self.preserve_timestamps,
            },
            same_format: SameFormatPolicy::Reencode,
        })
    }

    /// Recomputes synchronous validation errors.
    pub fn recompute_validation(&mut self) {
        self.validation_errors.clear();
        if self.input_paths.is_empty() {
            self.validation_errors
                .push("No input files selected.".to_string());
        }
        if let Some(ref insp) = self.inspection
            && insp.supported_count == 0
        {
            self.validation_errors
                .push("None of the selected files are supported images.".to_string());
        }
        if let Err(errs) = self.build_conversion_options() {
            self.validation_errors.extend(errs);
        }
    }

    /// Returns `true` when the `Convert` button should be enabled (`AC-C4.5`).
    #[must_use]
    pub fn can_submit(&self) -> bool {
        !self.submitting
            && !self.input_paths.is_empty()
            && self.validation_errors.is_empty()
            && self.dest_probe_error.is_none()
            && self
                .inspection
                .as_ref()
                .is_none_or(|insp| insp.supported_count > 0)
    }

    /// Drains background messages (header inspection, destination check, job submission).
    pub fn drain_background_messages(&mut self) {
        while let Ok(msg) = self.bg_rx.try_recv() {
            match msg {
                BgTaskMessage::Inspection(summary) => {
                    if self.dest_folder_text.is_empty()
                        && let Some(ref parent) = summary.primary_parent_dir
                    {
                        self.dest_folder_text = parent.to_string_lossy().into_owned();
                    }
                    self.inspection = Some(summary);
                    self.recompute_validation();
                }
                BgTaskMessage::DestProbe { probe_seq, result } => {
                    if probe_seq == self.dest_probe_seq {
                        self.dest_probe_pending = false;
                        self.dest_probe_error = result.err();
                    }
                }
                BgTaskMessage::Submitted(res) => {
                    self.submitting = false;
                    match res {
                        Ok((job_id, _dedup)) => {
                            self.submitted_job_id = Some(job_id);
                            self.submit_error = None;
                        }
                        Err(err) => {
                            self.submit_error = Some(err);
                        }
                    }
                }
            }
        }

        // Trigger debounced async destination check (150 ms)
        if self.dest_probe_pending
            && self
                .last_dest_edit
                .is_some_and(|t| t.elapsed() >= Duration::from_millis(150))
        {
            self.last_dest_edit = None;
            self.spawn_destination_probe();
        }
    }

    fn schedule_dest_probe(&mut self) {
        self.dest_probe_seq = self.dest_probe_seq.wrapping_add(1);
        if self.dest_mode != UiDestMode::CustomFolder {
            self.dest_probe_error = None;
            self.dest_probe_pending = false;
            return;
        }
        self.last_dest_edit = Some(Instant::now());
        self.dest_probe_pending = true;
    }

    fn spawn_destination_probe(&mut self) {
        let seq = self.dest_probe_seq;
        let path = PathBuf::from(self.dest_folder_text.trim());
        let tx = self.bg_tx.clone();
        let ctx = self.egui_ctx.clone();
        let _ = thread::Builder::new()
            .name("radivine-convert-dest-probe".to_string())
            .spawn(move || {
                let result = probe_custom_destination(&path);
                let _ = tx.send(BgTaskMessage::DestProbe {
                    probe_seq: seq,
                    result,
                });
                if let Some(c) = ctx {
                    c.request_repaint();
                }
            });
    }

    /// Saves the current Advanced Convert settings as a user preset in `convert.toml`.
    pub fn save_current_as_user_preset(&mut self) -> Result<String, String> {
        let name = self.save_preset_name.trim().to_string();
        if name.is_empty() {
            return Err("Preset name cannot be empty.".to_string());
        }
        let slug: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() {
                    c.to_ascii_lowercase()
                } else {
                    '-'
                }
            })
            .collect::<String>()
            .trim_matches('-')
            .to_string();
        let id = format!("user.{slug}");
        if id == "user." {
            return Err("Preset name must contain at least one letter or number.".to_string());
        }

        let opts = self
            .build_conversion_options()
            .map_err(|errs| errs.join(" · "))?;

        let preset = Preset {
            id: id.clone(),
            name: name.clone(),
            description: format!(
                "Custom {} preset",
                self.target_format.as_str().to_uppercase()
            ),
            builtin: false,
            target: opts.target,
            overrides: PresetOverrides {
                quality: Some(opts.quality),
                resize: Some(opts.resize),
                alpha_background: Some(opts.alpha.background),
                orientation: Some(opts.metadata.orientation),
                icc: Some(opts.metadata.icc),
                exif: Some(opts.metadata.exif),
                output_location: Some(opts.output.location),
                naming: Some(opts.output.naming),
                collision: Some(opts.output.collision),
                preserve_timestamps: Some(opts.output.preserve_timestamps),
                same_format: Some(opts.same_format),
            },
        };

        self.settings.user_presets.retain(|p| p.id != id);
        self.settings.user_presets.push(preset);

        let snap = DiskFileSnapshot::load(&self.convert_path);
        let updated = apply_convert_settings_doc(&snap.raw_text, &self.settings)
            .map_err(|errs| errs.join(" · "))?;
        atomic_write_with_backup(&self.convert_path, &updated, false)
            .map_err(|e| format!("Failed writing {}: {e}", self.convert_path.display()))?;

        self.selected_preset_id = id.clone();
        self.save_preset_open = false;
        self.save_preset_error = None;
        self.save_preset_ok = Some(format!("Saved preset '{name}' ({id})."));
        Ok(id)
    }

    /// Submits the conversion job to `radivine-worker` on a background thread (`Origin::Advanced`, `AC-C4.5`).
    pub fn trigger_submit(&mut self) {
        self.recompute_validation();
        if !self.can_submit() {
            return;
        }
        let Ok(options) = self.build_conversion_options() else {
            return;
        };

        self.submitting = true;
        self.submit_error = None;

        let socket_path = self.service_paths.worker_socket_path();
        let state_dir = self.service_paths.state_dir.clone();
        let items = self.input_paths.clone();
        let tx = self.bg_tx.clone();
        let ctx = self.egui_ctx.clone();

        let _ = thread::Builder::new()
            .name("radivine-convert-submit".to_string())
            .spawn(move || {
                let res = submit_advanced_job(&socket_path, &state_dir, &items, &options);
                let _ = tx.send(BgTaskMessage::Submitted(res));
                if let Some(c) = ctx {
                    c.request_repaint();
                }
            });
    }

    /// Synchronous helper for tests to submit the current job directly against `socket_path`.
    pub fn submit_blocking(&mut self, socket_path: &Path) -> Result<(JobId, bool), String> {
        self.recompute_validation();
        let options = self.build_conversion_options().map_err(|e| e.join(" · "))?;
        let res = submit_advanced_job(
            socket_path,
            &self.service_paths.state_dir,
            &self.input_paths,
            &options,
        );
        match res {
            Ok((job_id, dedup)) => {
                self.submitted_job_id = Some(job_id);
                self.submit_error = None;
                Ok((job_id, dedup))
            }
            Err(err) => {
                self.submit_error = Some(err.clone());
                Err(err)
            }
        }
    }

    /// Renders the complete Advanced Convert window frame into `ui`.
    pub fn render_frame(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.fonts_installed {
            install_inter_font(&ctx);
            self.fonts_installed = true;
        }
        self.drain_background_messages();

        // Keyboard shortcuts (`Esc` cancels/closes, `Enter` submits when valid and not editing a text field)
        let esc_pressed = ctx.input(|i| i.key_pressed(Key::Escape));
        if esc_pressed {
            if self.save_preset_open {
                self.save_preset_open = false;
            } else {
                self.should_close = true;
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        }

        let enter_pressed = ctx.input(|i| i.key_pressed(Key::Enter));
        let any_text_focused = ctx.memory(|m| m.focused().is_some());
        if enter_pressed
            && !any_text_focused
            && self.submitted_job_id.is_none()
            && self.can_submit()
        {
            self.trigger_submit();
        }

        let pal = if self.is_dark_theme {
            BrandPalette::dark()
        } else {
            BrandPalette::light()
        };
        apply_palette_visuals(&ctx, self.is_dark_theme, &pal);

        egui::CentralPanel::default()
            .frame(
                egui::Frame::NONE
                    .fill(pal.bg_canvas)
                    .inner_margin(Margin::same(18)),
            )
            .show(ui, |ui| {
                // Header
                ui.horizontal(|ui| {
                    ui.label(
                        RichText::new("Convert Images")
                            .font(FontId::new(19.0, FontFamily::Proportional))
                            .strong()
                            .color(pal.fg_primary),
                    );
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let summary_str = match &self.inspection {
                            Some(insp) => insp.headline_text(),
                            None => format!("Inspecting {} selected items…", self.input_paths.len()),
                        };
                        ui.label(
                            RichText::new(summary_str)
                                .size(12.0)
                                .color(pal.fg_secondary),
                        );
                    });
                });
                ui.add_space(10.0);

                // Post-submit confirmation toast/banner (`CONVERSION_UX.md` §5.5)
                if let Some(job_id) = self.submitted_job_id {
                    egui::Frame::NONE
                        .fill(pal.ok_bg)
                        .stroke(Stroke::new(1.0, pal.ok_fg))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.horizontal_wrapped(|ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "Queued job #{job_id} ({} items). Conversion continues in the background.",
                                        self.input_paths.len()
                                    ))
                                    .size(12.5)
                                    .strong()
                                    .color(pal.ok_fg),
                                );
                                if ui
                                    .button(RichText::new("Show progress / Open Jobs").size(12.0))
                                    .clicked()
                                {
                                    open_control_center_jobs_page(&self.service_paths);
                                    self.should_close = true;
                                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                }
                                if ui.button(RichText::new("Close").size(12.0)).clicked() {
                                    self.should_close = true;
                                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                                }
                            });
                        });
                    ui.add_space(10.0);
                }

                // Submission error banner (`AC-C4.5`)
                if let Some(ref err) = self.submit_error {
                    egui::Frame::NONE
                        .fill(pal.err_bg)
                        .stroke(Stroke::new(1.0, pal.err_fg))
                        .corner_radius(CornerRadius::same(8))
                        .inner_margin(Margin::symmetric(14, 10))
                        .show(ui, |ui| {
                            ui.label(
                                RichText::new(format!("Submission failed: {err}"))
                                    .size(12.5)
                                    .strong()
                                    .color(pal.err_fg),
                            );
                        });
                    ui.add_space(10.0);
                }

                ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .max_height(ui.available_height() - 56.0)
                    .show(ui, |ui| {
                        self.render_selection_card(ui, &pal);
                        ui.add_space(12.0);
                        self.render_target_and_preset_card(ui, &pal);
                        ui.add_space(12.0);
                        self.render_quality_and_resize_card(ui, &pal);
                        ui.add_space(12.0);
                        self.render_alpha_and_metadata_card(ui, &pal);
                        ui.add_space(12.0);
                        self.render_destination_card(ui, &pal);
                        ui.add_space(8.0);
                    });

                ui.separator();
                ui.add_space(6.0);

                // Sticky action footer
                ui.horizontal(|ui| {
                    if let Some(first_err) = self
                        .validation_errors
                        .first()
                        .or(self.dest_probe_error.as_ref())
                    {
                        ui.label(RichText::new(first_err).size(11.5).color(pal.err_fg));
                    } else if let Some(ref ok_msg) = self.save_preset_ok {
                        ui.label(RichText::new(ok_msg).size(11.5).color(pal.ok_fg));
                    }

                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let label = if self.submitting {
                            "Submitting…".to_string()
                        } else {
                            format!(
                                "Convert {} → {}",
                                self.input_paths.len(),
                                self.target_format.as_str().to_ascii_uppercase()
                            )
                        };
                        let btn = egui::Button::new(
                            RichText::new(label)
                                .size(13.0)
                                .strong()
                                .color(Color32::WHITE),
                        )
                        .fill(pal.leaf_green);
                        if ui.add_enabled(self.can_submit(), btn).clicked() {
                            self.trigger_submit();
                        }

                        if ui.button(RichText::new("Cancel").size(12.5)).clicked() {
                            self.should_close = true;
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                });
            });
    }

    fn render_selection_card(&self, ui: &mut egui::Ui, pal: &BrandPalette) {
        section_card(ui, pal, "1. Selection Summary", |ui| {
            ui.label(
                RichText::new(
                    "Asynchronous header-only inspection (no full image decode or thumbnail generation)",
                )
                .size(11.5)
                .color(pal.fg_muted),
            );
            ui.add_space(4.0);
            if let Some(ref insp) = self.inspection {
                ui.label(
                    RichText::new(insp.headline_text())
                        .size(13.0)
                        .strong()
                        .color(pal.fg_primary),
                );
                if insp.truncated_header_probe {
                    ui.label(
                        RichText::new(format!(
                            "Header-probed first {MAX_EAGER_HEADER_PROBES} files; remaining {} files summarized by extension.",
                            insp.total_selected.saturating_sub(MAX_EAGER_HEADER_PROBES)
                        ))
                        .size(11.5)
                        .color(pal.fg_secondary),
                    );
                }
                if !insp.unsupported_items.is_empty() {
                    ui.add_space(4.0);
                    egui::CollapsingHeader::new(
                        RichText::new(format!(
                            "{} unsupported or unreadable item(s) will be skipped/failed",
                            insp.unsupported_items.len()
                        ))
                        .size(12.0)
                        .color(pal.warn_fg),
                    )
                    .default_open(insp.unsupported_items.len() <= 3)
                    .show(ui, |ui| {
                        for (name, reason) in insp.unsupported_items.iter().take(25) {
                            ui.label(
                                RichText::new(format!("• {name}: {reason}"))
                                    .size(11.5)
                                    .color(pal.fg_secondary),
                            );
                        }
                    });
                }
            } else {
                ui.label(
                    RichText::new("Inspecting image headers…")
                        .size(12.5)
                        .color(pal.fg_secondary),
                );
            }
        });
    }

    fn render_target_and_preset_card(&mut self, ui: &mut egui::Ui, pal: &BrandPalette) {
        section_card(ui, pal, "2. Target Format & Preset", |ui| {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new("Format:")
                        .size(12.5)
                        .strong()
                        .color(pal.fg_primary),
                );
                for (fmt, label) in [
                    (FormatId::Png, "PNG"),
                    (FormatId::Jpeg, "JPEG"),
                    (FormatId::WebP, "WebP"),
                ] {
                    if ui
                        .selectable_label(self.target_format == fmt, label)
                        .clicked()
                    {
                        self.select_target_format(fmt);
                    }
                }

                ui.add_space(16.0);
                ui.label(
                    RichText::new("Preset:")
                        .size(12.5)
                        .strong()
                        .color(pal.fg_primary),
                );
                let presets = self.all_presets();
                let current_label = presets
                    .iter()
                    .find(|p| p.id == self.selected_preset_id)
                    .map(|p| p.name.clone())
                    .unwrap_or_else(|| "Custom".to_string());

                let mut chosen_id: Option<String> = None;
                egui::ComboBox::from_id_salt("adv_preset_combo")
                    .selected_text(current_label)
                    .width(210.0)
                    .show_ui(ui, |ui| {
                        for p in &presets {
                            if ui
                                .selectable_label(
                                    self.selected_preset_id == p.id,
                                    format!("{} ({})", p.name, p.target.as_str().to_uppercase()),
                                )
                                .clicked()
                            {
                                chosen_id = Some(p.id.clone());
                            }
                        }
                    });
                if let Some(id) = chosen_id {
                    self.apply_preset_by_id(&id);
                }

                if ui
                    .button(RichText::new("Save as preset…").size(12.0))
                    .clicked()
                {
                    self.save_preset_open = !self.save_preset_open;
                    self.save_preset_error = None;
                }
            });

            if self.save_preset_open {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Preset name:").size(12.0));
                    ui.add(
                        egui::TextEdit::singleline(&mut self.save_preset_name).desired_width(180.0),
                    );
                    if ui.button(RichText::new("Save").size(12.0)).clicked()
                        && let Err(err) = self.save_current_as_user_preset()
                    {
                        self.save_preset_error = Some(err);
                    }
                    if ui.button(RichText::new("Cancel").size(12.0)).clicked() {
                        self.save_preset_open = false;
                    }
                });
                if let Some(ref err) = self.save_preset_error {
                    ui.label(RichText::new(err).size(11.5).color(pal.err_fg));
                }
            }
        });
    }

    fn render_quality_and_resize_card(&mut self, ui: &mut egui::Ui, pal: &BrandPalette) {
        section_card(ui, pal, "3. Quality & Resize", |ui| {
            let mut changed = false;
            match self.target_format {
                FormatId::Png => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("PNG compression:").size(12.5));
                        for (lvl, label) in [
                            (PngCompression::Fast, "Fast"),
                            (PngCompression::Default, "Default (Lossless)"),
                            (PngCompression::Best, "Best"),
                        ] {
                            if ui
                                .selectable_label(self.png_compression == lvl, label)
                                .clicked()
                            {
                                self.png_compression = lvl;
                                self.selected_preset_id = "custom".to_string();
                                changed = true;
                            }
                        }
                    });
                }
                FormatId::Jpeg => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("JPEG quality:").size(12.5));
                        if ui
                            .add(egui::Slider::new(&mut self.quality_value, 1..=100).suffix("%"))
                            .changed()
                        {
                            self.selected_preset_id = "custom".to_string();
                            changed = true;
                        }
                        ui.add_space(12.0);
                        ui.label(RichText::new("Chroma subsampling:").size(12.0));
                        for (sub, label) in [
                            (ChromaSubsampling::Auto, "Auto"),
                            (ChromaSubsampling::Cs420, "4:2:0"),
                            (ChromaSubsampling::Cs444, "4:4:4"),
                        ] {
                            if ui
                                .selectable_label(self.jpeg_subsampling == sub, label)
                                .clicked()
                            {
                                self.jpeg_subsampling = sub;
                                self.selected_preset_id = "custom".to_string();
                                changed = true;
                            }
                        }
                    });
                }
                FormatId::WebP => {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("WebP mode:").size(12.5));
                        if ui.selectable_label(!self.webp_lossless, "Lossy").clicked() {
                            self.webp_lossless = false;
                            self.selected_preset_id = "custom".to_string();
                            changed = true;
                        }
                        if ui
                            .selectable_label(self.webp_lossless, "Lossless")
                            .clicked()
                        {
                            self.webp_lossless = true;
                            self.selected_preset_id = "custom".to_string();
                            changed = true;
                        }
                        if !self.webp_lossless {
                            ui.add_space(12.0);
                            ui.label(RichText::new("Quality:").size(12.0));
                            if ui
                                .add(
                                    egui::Slider::new(&mut self.quality_value, 1..=100).suffix("%"),
                                )
                                .changed()
                            {
                                self.selected_preset_id = "custom".to_string();
                                changed = true;
                            }
                        }
                    });
                }
                _ => {}
            }

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label(RichText::new("Resize:").size(12.5));
                for (mode, label) in [
                    (UiResizeMode::Original, "Original size"),
                    (UiResizeMode::Width, "Width (px)"),
                    (UiResizeMode::Height, "Height (px)"),
                    (UiResizeMode::FitWithin, "Fit within (W × H)"),
                ] {
                    if ui
                        .selectable_label(self.resize_mode == mode, label)
                        .clicked()
                    {
                        self.resize_mode = mode;
                        self.selected_preset_id = "custom".to_string();
                        changed = true;
                    }
                }
            });

            if self.resize_mode != UiResizeMode::Original {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if matches!(
                        self.resize_mode,
                        UiResizeMode::Width | UiResizeMode::FitWithin
                    ) {
                        ui.label(RichText::new("Width (px):").size(12.0));
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut self.resize_width_text)
                                    .desired_width(80.0),
                            )
                            .changed()
                        {
                            self.selected_preset_id = "custom".to_string();
                            changed = true;
                        }
                    }
                    if matches!(
                        self.resize_mode,
                        UiResizeMode::Height | UiResizeMode::FitWithin
                    ) {
                        ui.label(RichText::new("Height (px):").size(12.0));
                        if ui
                            .add(
                                egui::TextEdit::singleline(&mut self.resize_height_text)
                                    .desired_width(80.0),
                            )
                            .changed()
                        {
                            self.selected_preset_id = "custom".to_string();
                            changed = true;
                        }
                    }
                    let mut do_not_upscale = !self.allow_upscale;
                    if ui
                        .checkbox(&mut do_not_upscale, "Do not upscale smaller images")
                        .changed()
                    {
                        self.allow_upscale = !do_not_upscale;
                        self.selected_preset_id = "custom".to_string();
                        changed = true;
                    }
                });
            }

            if changed {
                self.recompute_validation();
            }
        });
    }

    fn render_alpha_and_metadata_card(&mut self, ui: &mut egui::Ui, pal: &BrandPalette) {
        section_card(ui, pal, "4. Transparency & Metadata", |ui| {
            let mut changed = false;
            if self.target_format == FormatId::Jpeg {
                let alpha_files = self.inspection.as_ref().map_or(0, |insp| insp.alpha_count);
                if alpha_files > 0 {
                    ui.label(
                        RichText::new(format!(
                            "⚠ {alpha_files} selected file(s) contain transparency and will be flattened onto the background color below."
                        ))
                        .size(12.0)
                        .color(pal.warn_fg),
                    );
                }
                ui.horizontal(|ui| {
                    ui.label(RichText::new("JPEG background (#RRGGBB):").size(12.0));
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.background_hex)
                                .desired_width(90.0),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                    if ui.button(RichText::new("White").size(11.5)).clicked() {
                        self.background_hex = "#FFFFFF".to_string();
                        changed = true;
                    }
                    if ui.button(RichText::new("Black").size(11.5)).clicked() {
                        self.background_hex = "#000000".to_string();
                        changed = true;
                    }
                });
                ui.add_space(4.0);
            }

            ui.horizontal_wrapped(|ui| {
                if ui
                    .checkbox(&mut self.auto_orient, "Auto-orient from EXIF")
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .checkbox(&mut self.keep_icc, "Keep ICC profile")
                    .changed()
                {
                    changed = true;
                }
                if ui
                    .checkbox(&mut self.strip_exif, "Strip EXIF metadata (recommended)")
                    .changed()
                {
                    changed = true;
                }
            });

            if !self.strip_exif {
                ui.label(
                    RichText::new(
                        "⚠ Preserving EXIF metadata may retain GPS coordinates and camera serial numbers.",
                    )
                    .size(11.5)
                    .color(pal.warn_fg),
                );
            }

            if changed {
                self.recompute_validation();
            }
        });
    }

    fn render_destination_card(&mut self, ui: &mut egui::Ui, pal: &BrandPalette) {
        section_card(ui, pal, "5. Destination & Naming", |ui| {
            let mut changed = false;
            ui.horizontal(|ui| {
                ui.label(RichText::new("Save to:").size(12.5));
                for (mode, label) in [
                    (UiDestMode::SameFolder, "Same folder as source"),
                    (UiDestMode::Subfolder, "Subfolder"),
                    (UiDestMode::CustomFolder, "Custom folder"),
                ] {
                    if ui.selectable_label(self.dest_mode == mode, label).clicked() {
                        self.dest_mode = mode;
                        changed = true;
                        self.schedule_dest_probe();
                    }
                }
            });

            if self.dest_mode == UiDestMode::Subfolder {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Subfolder name:").size(12.0));
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.dest_subfolder_text)
                                .desired_width(220.0),
                        )
                        .changed()
                    {
                        changed = true;
                    }
                });
            } else if self.dest_mode == UiDestMode::CustomFolder {
                ui.horizontal(|ui| {
                    ui.label(RichText::new("Folder path:").size(12.0));
                    if ui
                        .add(
                            egui::TextEdit::singleline(&mut self.dest_folder_text)
                                .desired_width(340.0),
                        )
                        .changed()
                    {
                        changed = true;
                        self.schedule_dest_probe();
                    }
                });
            }

            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if ui
                    .checkbox(&mut self.append_suffix, "Append filename suffix:")
                    .changed()
                {
                    changed = true;
                }
                if self.append_suffix
                    && ui
                        .add(egui::TextEdit::singleline(&mut self.suffix_text).desired_width(130.0))
                        .changed()
                {
                    changed = true;
                }

                ui.add_space(12.0);
                ui.label(RichText::new("If output exists:").size(12.0));
                for (pol, label) in [
                    (CollisionPolicy::Suffix, "Rename (file (1))"),
                    (CollisionPolicy::Skip, "Skip"),
                    (CollisionPolicy::Fail, "Fail item"),
                ] {
                    if ui
                        .selectable_label(self.collision_policy == pol, label)
                        .clicked()
                    {
                        self.collision_policy = pol;
                        changed = true;
                    }
                }
            });

            if changed {
                self.recompute_validation();
            }
        });
    }
}

impl eframe::App for AdvancedConvertApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.render_frame(ui);
    }
}

fn probe_custom_destination(path: &Path) -> Result<(), String> {
    if !path.is_absolute() {
        return Err("Custom output folder must be an absolute path.".to_string());
    }
    let check_dir = if path.exists() {
        if !path.is_dir() {
            return Err(format!("'{}' is not a directory.", path.display()));
        }
        path.to_path_buf()
    } else if let Some(parent) = path.parent() {
        if !parent.exists() {
            return Err(format!(
                "Parent directory '{}' does not exist.",
                parent.display()
            ));
        }
        parent.to_path_buf()
    } else {
        return Err(format!("Cannot write to '{}'.", path.display()));
    };

    let meta = std::fs::metadata(&check_dir)
        .map_err(|e| format!("Cannot access '{}': {e}", check_dir.display()))?;
    if meta.permissions().readonly() {
        return Err(format!("Directory '{}' is read-only.", check_dir.display()));
    }
    Ok(())
}

fn submit_advanced_job(
    socket_path: &Path,
    state_dir: &Path,
    items: &[WirePath],
    options: &ConversionOptions,
) -> Result<(JobId, bool), String> {
    let paths = crate::ui::jobs::worker_paths_for(socket_path, state_dir);
    let mut client = JobsClient::connect_or_launch(&paths).map_err(|e| e.to_string())?;
    let req_id = RequestId::generate();
    let (job_id, _state, dedup) = client
        .submit_typed(req_id, Origin::Advanced, CONVERT_IMAGE_KIND, items, options)
        .map_err(|e| e.to_string())?;
    Ok((job_id, dedup))
}

fn open_control_center_jobs_page(service_paths: &ServicePaths) {
    let exe = std::env::current_exe().unwrap_or_else(|_| PathBuf::from("radivine-control"));
    let mut cmd = Command::new(exe);
    cmd.arg("--page").arg("jobs");
    if let Some(ref cfg) = service_paths.config_path {
        cmd.arg("--config").arg(cfg);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    let _ = cmd.spawn();
}

/// Launches the standalone `radivine-control convert` window (`640×720`, resizable, `AC-C4.5`).
pub fn run_advanced_convert_window(
    input_paths: Vec<WirePath>,
    service_paths: ServicePaths,
) -> Result<(), String> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_title("Radivine — Convert images")
        .with_app_id("radivine-control-convert")
        .with_inner_size(Vec2::new(640.0, 720.0))
        .with_min_inner_size(Vec2::new(560.0, 560.0));

    if let Some(icon_data) = decode_png_to_icon_data(EMBEDDED_APP_ICON_PNG) {
        viewport = viewport.with_icon(Arc::new(icon_data));
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "radivine-control-convert",
        native_options,
        Box::new(move |cc| {
            let app =
                AdvancedConvertApp::new(input_paths, service_paths, Some(cc.egui_ctx.clone()));
            Ok(Box::new(app))
        }),
    )
    .map_err(|e| e.to_string())
}
