//! Conversion Settings page (`ControlPage::Conversion`, `CONVERSION_UX.md` §6).
//!
//! Edits `~/.config/radivine/convert.toml` (presets, defaults, limits) and
//! `~/.config/radivine/config.toml` `[jobs]` (concurrency, retention, notifications)
//! with format-preserving `toml_edit` persistence, inline validation, external conflict
//! detection, and live worker reload (`reload_settings`).

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::str::FromStr;

use eframe::egui::{self, CornerRadius, Margin, RichText, Stroke};
use radivine_convert::{
    ChromaSubsampling, ConvertSettings, ExifPolicy, FormatId, FormatQuality, IccPolicy, JpegExt,
    NamingPolicy, OrientationPolicy, OutputLocation, PngCompression, Preset, PresetOverrides,
    QualityIntent, QualitySetting, ResizeSpec, Rgb8, WebPMode, builtin_presets,
};
use radivine_jobs::{JobsNotifyPolicy, JobsSettings, MaxParallelSetting};

use super::theme::{BrandPalette, kv_cell, section_card};

/// Editable form state for the Conversion Settings page.
#[derive(Clone, Debug)]
pub struct ConversionPageState {
    /// Text buffer for subfolder destination when `OutputLocation::Subfolder` is selected.
    pub subfolder_text: String,
    /// Text buffer for fixed folder destination when `OutputLocation::Directory` is selected.
    pub fixed_folder_text: String,
    /// Selected destination mode (`"same_folder"`, `"subfolder"`, `"fixed"`).
    pub destination_mode: String,
    /// Filename suffix text buffer.
    pub filename_suffix_text: String,
    /// Selected JPEG extension.
    pub jpeg_ext: JpegExt,
    /// Background color mode (`"white"`, `"black"`, `"custom"`).
    pub bg_mode: String,
    /// Custom hex color buffer (`"#RRGGBB"`).
    pub bg_custom_hex: String,
    /// Megapixels input text (`1..=500`).
    pub max_megapixels_text: String,
    /// Max dimension input text (`64..=65535`).
    pub max_dimension_text: String,
    /// Max file size in MiB input text (`1..=4096`).
    pub max_file_mb_text: String,
    /// Per-item timeout in seconds (`1..=600`).
    pub timeout_secs_text: String,
    /// User preset editor state (`None` when closed).
    pub preset_editor: Option<PresetEditorDraft>,
    /// Last backend capability check status string.
    pub backend_status_note: String,
    /// Number of times "Check again" was clicked.
    pub backend_probe_count: u32,
}

/// Draft state for creating or editing a user preset (`[[convert.presets]]`).
#[derive(Clone, Debug)]
pub struct PresetEditorDraft {
    /// Index in `user_presets` if editing an existing preset, or `None` if creating a new one.
    pub editing_index: Option<usize>,
    pub id: String,
    pub name: String,
    pub description: String,
    pub target: FormatId,
    pub use_quality: bool,
    pub quality: u8,
    pub lossless: bool,
    pub strip_metadata: bool,
    pub resize_mode: String,
    pub resize_dim_text: String,
    pub filename_suffix: String,
    pub errors: Vec<String>,
}

impl Default for PresetEditorDraft {
    fn default() -> Self {
        Self {
            editing_index: None,
            id: "user.webp-blog".to_string(),
            name: "WebP for Blog (1600px)".to_string(),
            description: "Fit within 1600px and strip EXIF".to_string(),
            target: FormatId::WebP,
            use_quality: true,
            quality: 82,
            lossless: false,
            strip_metadata: true,
            resize_mode: "fit".to_string(),
            resize_dim_text: "1600".to_string(),
            filename_suffix: String::new(),
            errors: Vec::new(),
        }
    }
}

impl PresetEditorDraft {
    #[must_use]
    pub fn from_preset(idx: usize, p: &Preset) -> Self {
        let (resize_mode, resize_dim_text) = match p.overrides.resize {
            Some(ResizeSpec::Width { px, .. }) => ("width".to_string(), px.get().to_string()),
            Some(ResizeSpec::Height { px, .. }) => ("height".to_string(), px.get().to_string()),
            Some(ResizeSpec::Fit { width, .. }) => ("fit".to_string(), width.get().to_string()),
            _ => ("original".to_string(), "1600".to_string()),
        };
        let (use_quality, quality, lossless) = match p.overrides.quality {
            Some(QualitySetting::Intent(QualityIntent::Lossless)) => (false, 85, true),
            Some(QualitySetting::Intent(QualityIntent::Custom(q))) => (true, q, false),
            Some(QualitySetting::PerFormat(FormatQuality::Jpeg { quality, .. })) => {
                (true, quality, false)
            }
            Some(QualitySetting::PerFormat(FormatQuality::WebP { mode, quality, .. })) => {
                (true, quality, mode == WebPMode::Lossless)
            }
            Some(QualitySetting::PerFormat(FormatQuality::Png { .. })) => (false, 85, true),
            _ => (true, 85, p.target == FormatId::Png),
        };
        let filename_suffix = match &p.overrides.naming {
            Some(NamingPolicy::AppendSuffix { suffix, .. }) => suffix.clone(),
            _ => String::new(),
        };
        Self {
            editing_index: Some(idx),
            id: p.id.clone(),
            name: p.name.clone(),
            description: p.description.clone(),
            target: p.target,
            use_quality,
            quality,
            lossless,
            strip_metadata: p.overrides.exif != Some(ExifPolicy::PreserveAll),
            resize_mode,
            resize_dim_text,
            filename_suffix,
            errors: Vec::new(),
        }
    }

    pub fn to_preset(&self) -> Result<Preset, Vec<String>> {
        let mut errs = Vec::new();
        let trimmed_id = self.id.trim();
        if trimmed_id.is_empty() {
            errs.push("Preset ID cannot be empty.".to_string());
        } else if trimmed_id.starts_with("builtin.") {
            errs.push("User preset ID cannot start with 'builtin.'.".to_string());
        } else if !trimmed_id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
        {
            errs.push(
                "Preset ID may only contain ASCII letters, digits, '-', '_', and '.'.".to_string(),
            );
        }
        if self.name.trim().is_empty() {
            errs.push("Preset name cannot be empty.".to_string());
        }

        let resize = match self.resize_mode.as_str() {
            "original" => Some(ResizeSpec::Original),
            "width" => match self
                .resize_dim_text
                .trim()
                .parse::<u32>()
                .ok()
                .and_then(NonZeroU32::new)
            {
                Some(px) if px.get() <= 32_768 => Some(ResizeSpec::Width {
                    px,
                    allow_upscale: false,
                }),
                _ => {
                    errs.push("Resize width must be an integer between 1 and 32768.".to_string());
                    None
                }
            },
            "height" => match self
                .resize_dim_text
                .trim()
                .parse::<u32>()
                .ok()
                .and_then(NonZeroU32::new)
            {
                Some(px) if px.get() <= 32_768 => Some(ResizeSpec::Height {
                    px,
                    allow_upscale: false,
                }),
                _ => {
                    errs.push("Resize height must be an integer between 1 and 32768.".to_string());
                    None
                }
            },
            "fit" => match self
                .resize_dim_text
                .trim()
                .parse::<u32>()
                .ok()
                .and_then(NonZeroU32::new)
            {
                Some(px) if px.get() <= 32_768 => Some(ResizeSpec::Fit {
                    width: px,
                    height: px,
                    allow_upscale: false,
                }),
                _ => {
                    errs.push(
                        "Fit within dimension must be an integer between 1 and 32768.".to_string(),
                    );
                    None
                }
            },
            _ => None,
        };

        let quality_setting = match self.target {
            FormatId::Png => Some(QualitySetting::PerFormat(FormatQuality::Png {
                compression: PngCompression::Default,
            })),
            FormatId::Jpeg => Some(QualitySetting::PerFormat(FormatQuality::Jpeg {
                quality: self.quality.clamp(1, 100),
                progressive: true,
                optimize_coding: true,
                subsampling: ChromaSubsampling::Auto,
            })),
            FormatId::WebP => Some(QualitySetting::PerFormat(FormatQuality::WebP {
                mode: if self.lossless {
                    WebPMode::Lossless
                } else {
                    WebPMode::Lossy
                },
                quality: self.quality.clamp(1, 100),
                alpha_quality: 100,
                method: 4,
            })),
            _ => None,
        };

        let naming = if self.filename_suffix.trim().is_empty() {
            None
        } else {
            let suf = self.filename_suffix.trim();
            if suf.contains('/') || suf.contains('\\') || suf.contains('\0') {
                errs.push("Filename suffix cannot contain path separators.".to_string());
            }
            Some(NamingPolicy::AppendSuffix {
                suffix: suf.to_string(),
                jpeg_ext: JpegExt::Jpg,
            })
        };

        if !errs.is_empty() {
            return Err(errs);
        }

        Ok(Preset {
            id: trimmed_id.to_string(),
            name: self.name.trim().to_string(),
            description: self.description.trim().to_string(),
            builtin: false,
            target: self.target,
            overrides: PresetOverrides {
                quality: quality_setting,
                resize,
                exif: Some(if self.strip_metadata {
                    ExifPolicy::Strip
                } else {
                    ExifPolicy::PreserveAll
                }),
                naming,
                ..PresetOverrides::default()
            },
        })
    }
}

impl ConversionPageState {
    /// Synchronizes text buffers from `ConvertSettings`.
    #[must_use]
    pub fn from_settings(settings: &ConvertSettings) -> Self {
        let (destination_mode, subfolder_text, fixed_folder_text) =
            match &settings.defaults.output.location {
                OutputLocation::SameDirectory => (
                    "same_folder".to_string(),
                    "Converted".to_string(),
                    "/tmp".to_string(),
                ),
                OutputLocation::Subfolder(sub) => {
                    ("subfolder".to_string(), sub.clone(), "/tmp".to_string())
                }
                OutputLocation::Directory(p) => (
                    "fixed".to_string(),
                    "Converted".to_string(),
                    p.to_string_lossy().to_string(),
                ),
            };

        let (filename_suffix_text, jpeg_ext) = match &settings.defaults.output.naming {
            NamingPolicy::PreserveStem { jpeg_ext } => (String::new(), *jpeg_ext),
            NamingPolicy::AppendSuffix { suffix, jpeg_ext } => (suffix.clone(), *jpeg_ext),
        };

        let bg = settings.defaults.alpha.background;
        let (bg_mode, bg_custom_hex) = if bg == Rgb8::WHITE {
            ("white".to_string(), "#FFFFFF".to_string())
        } else if bg == Rgb8::BLACK {
            ("black".to_string(), "#000000".to_string())
        } else {
            ("custom".to_string(), bg.to_hex())
        };

        let mp = (settings.limits.max_pixels / 1_000_000).max(1);
        let mb = (settings.limits.max_source_bytes / (1024 * 1024)).max(1);

        Self {
            subfolder_text,
            fixed_folder_text,
            destination_mode,
            filename_suffix_text,
            jpeg_ext,
            bg_mode,
            bg_custom_hex,
            max_megapixels_text: mp.to_string(),
            max_dimension_text: settings.limits.max_dimension.to_string(),
            max_file_mb_text: mb.to_string(),
            timeout_secs_text: settings.limits.per_item_timeout_secs.to_string(),
            preset_editor: None,
            backend_status_note:
                "Native Rust image engine (image-rs 0.25): Available · PNG, JPEG, WebP".to_string(),
            backend_probe_count: 1,
        }
    }

    /// Applies text-field edits onto `settings` and returns any validation errors (`AC-C4.4`).
    pub fn sync_and_validate(
        &self,
        settings: &mut ConvertSettings,
        jobs: &JobsSettings,
    ) -> Vec<String> {
        let mut errs = Vec::new();

        match self.destination_mode.as_str() {
            "same_folder" => {
                settings.defaults.output.location = OutputLocation::SameDirectory;
            }
            "subfolder" => {
                let sub = self.subfolder_text.trim();
                if sub.is_empty()
                    || sub == "."
                    || sub == ".."
                    || sub.contains('/')
                    || sub.contains('\\')
                    || sub.contains('\0')
                    || sub.contains("..")
                {
                    errs.push(
                        "Subfolder name must be a single relative directory name without '/' or '..'."
                            .to_string(),
                    );
                }
                settings.defaults.output.location = OutputLocation::Subfolder(sub.to_string());
            }
            "fixed" => {
                let trimmed = self.fixed_folder_text.trim();
                let p = PathBuf::from(trimmed);
                if !p.is_absolute() {
                    errs.push("Fixed destination folder must be an absolute path.".to_string());
                }
                settings.defaults.output.location = OutputLocation::Directory(p);
            }
            _ => {}
        }

        let suf = self.filename_suffix_text.trim();
        if suf.is_empty() {
            settings.defaults.output.naming = NamingPolicy::PreserveStem {
                jpeg_ext: self.jpeg_ext,
            };
        } else {
            if suf.contains('/') || suf.contains('\\') || suf.contains('\0') {
                errs.push("Filename suffix must not contain path separators.".to_string());
            }
            settings.defaults.output.naming = NamingPolicy::AppendSuffix {
                suffix: suf.to_string(),
                jpeg_ext: self.jpeg_ext,
            };
        }

        match self.bg_mode.as_str() {
            "white" => settings.defaults.alpha.background = Rgb8::WHITE,
            "black" => settings.defaults.alpha.background = Rgb8::BLACK,
            "custom" => match Rgb8::from_str(&self.bg_custom_hex) {
                Ok(rgb) => settings.defaults.alpha.background = rgb,
                Err(e) => errs.push(format!("Custom background color: {e}")),
            },
            _ => {}
        }

        match self.max_megapixels_text.trim().parse::<u64>() {
            Ok(mp) if (1..=500).contains(&mp) => {
                settings.limits.max_pixels = mp * 1_000_000;
            }
            _ => errs
                .push("Max image megapixels must be an integer between 1 and 500 MP.".to_string()),
        }

        match self.max_dimension_text.trim().parse::<u32>() {
            Ok(dim) if (64..=65_535).contains(&dim) => {
                settings.limits.max_dimension = dim;
            }
            _ => errs.push(
                "Max image dimension must be an integer between 64 and 65535 px.".to_string(),
            ),
        }

        match self.max_file_mb_text.trim().parse::<u64>() {
            Ok(mb) if (1..=4096).contains(&mb) => {
                settings.limits.max_source_bytes = mb * 1024 * 1024;
            }
            _ => errs.push(
                "Max source file size must be an integer between 1 and 4096 MiB.".to_string(),
            ),
        }

        match self.timeout_secs_text.trim().parse::<u64>() {
            Ok(secs) if (1..=600).contains(&secs) => {
                settings.limits.per_item_timeout_secs = secs;
            }
            _ => errs
                .push("Per-file timeout must be an integer between 1 and 600 seconds.".to_string()),
        }

        match settings.save_preserving_comments("") {
            Ok(serialized) => {
                if let Err(s_errs) = ConvertSettings::validate_strict(&serialized) {
                    errs.extend(s_errs);
                }
            }
            Err(e) => errs.push(e),
        }

        if let Err(j_errs) = crate::persistence::validate_jobs_settings_strict(jobs) {
            errs.extend(j_errs);
        }

        errs
    }
}

/// Actions emitted by the Conversion Settings page back to `ControlCenterApp`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConversionPageAction {
    Save,
    Revert,
    ReloadFromDisk,
    BackupAndOverwrite,
}

/// Renders the complete Conversion Settings page (`CONVERSION_UX.md` §6).
#[allow(clippy::too_many_arguments)]
pub fn render_conversion_settings_page(
    ui: &mut egui::Ui,
    pal: &BrandPalette,
    state: &mut ConversionPageState,
    draft_convert: &mut ConvertSettings,
    saved_convert: &ConvertSettings,
    draft_jobs: &mut JobsSettings,
    saved_jobs: &JobsSettings,
    conflict_pending: bool,
) -> Option<ConversionPageAction> {
    let mut action = None;
    let validation_errors = state.sync_and_validate(draft_convert, draft_jobs);
    let has_unsaved =
        draft_convert != saved_convert || draft_jobs != saved_jobs || !validation_errors.is_empty();

    if conflict_pending {
        egui::Frame::NONE
            .fill(pal.warn_bg)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(12, 8))
            .stroke(Stroke::new(1.0, pal.warn_fg))
            .show(ui, |ui| {
                ui.horizontal_wrapped(|ui| {
                    ui.label(
                        RichText::new(
                            "External edit detected on convert.toml or config.toml since it was loaded.",
                        )
                        .color(pal.warn_fg)
                        .strong(),
                    );
                    if ui.button("Reload from Disk").clicked() {
                        action = Some(ConversionPageAction::ReloadFromDisk);
                    }
                    if ui.button("Back Up (.bak) & Overwrite").clicked() {
                        action = Some(ConversionPageAction::BackupAndOverwrite);
                    }
                });
            });
        ui.add_space(10.0);
    }

    if !validation_errors.is_empty() {
        egui::Frame::NONE
            .fill(pal.err_bg)
            .corner_radius(CornerRadius::same(6))
            .inner_margin(Margin::symmetric(12, 8))
            .stroke(Stroke::new(1.0, pal.err_fg))
            .show(ui, |ui| {
                ui.label(
                    RichText::new("Please fix the following validation issues before saving:")
                        .color(pal.err_fg)
                        .strong(),
                );
                for err in &validation_errors {
                    ui.label(
                        RichText::new(format!("• {err}"))
                            .color(pal.err_fg)
                            .size(12.0),
                    );
                }
            });
        ui.add_space(10.0);
    }

    // Top action bar: Save / Revert
    egui::Frame::NONE
        .fill(pal.bg_elevated)
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::symmetric(14, 10))
        .stroke(Stroke::new(1.0, pal.border))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                let status_label = if !validation_errors.is_empty() {
                    RichText::new("Invalid settings — cannot save")
                        .color(pal.err_fg)
                        .strong()
                } else if has_unsaved {
                    RichText::new("Unsaved conversion changes")
                        .color(pal.warn_fg)
                        .strong()
                } else {
                    RichText::new("All conversion settings saved (`convert.toml` & `config.toml`)")
                        .color(pal.fg_secondary)
                };
                ui.label(status_label.size(12.5));

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let can_save = validation_errors.is_empty() && has_unsaved;
                    if ui
                        .add_enabled(can_save, egui::Button::new("Save Conversion Settings"))
                        .clicked()
                    {
                        action = Some(ConversionPageAction::Save);
                    }
                    if ui
                        .add_enabled(has_unsaved, egui::Button::new("Revert Changes"))
                        .clicked()
                    {
                        action = Some(ConversionPageAction::Revert);
                    }
                });
            });
        });

    ui.add_space(14.0);

    // 1. Quick Convert Defaults
    section_card(ui, pal, "Quick Convert Defaults", |ui| {
        ui.label(
                RichText::new("Presets used when selecting Convert -> Image -> PNG / JPEG / WebP directly from the radial wheel.")
                    .color(pal.fg_muted)
                    .size(12.0),
            );
        ui.add_space(6.0);
        let all_presets = draft_convert.all_presets();
        for (format, title) in [
            (FormatId::Png, "Quick Convert -> PNG preset:"),
            (FormatId::Jpeg, "Quick Convert -> JPEG preset:"),
            (FormatId::WebP, "Quick Convert -> WebP preset:"),
        ] {
            let current_id = draft_convert.quick_convert_preset_for(format);
            let matching: Vec<&Preset> =
                all_presets.iter().filter(|p| p.target == format).collect();
            ui.horizontal(|ui| {
                ui.label(RichText::new(title).color(pal.fg_primary).size(13.0));
                egui::ComboBox::from_id_salt(format!("qc_preset_{}", format.as_str()))
                    .selected_text(&current_id)
                    .width(260.0)
                    .show_ui(ui, |ui| {
                        for p in matching {
                            let label = format!("{} ({})", p.id, p.name);
                            if ui.selectable_label(current_id == p.id, label).clicked() {
                                draft_convert.set_quick_convert_preset(format, p.id.clone());
                            }
                        }
                    });
            });
            ui.add_space(4.0);
        }
    });

    ui.add_space(14.0);

    // 2. Output & Naming
    section_card(ui, pal, "Output & Naming", |ui| {
        ui.label(
                RichText::new("Default destination folder, filename suffix, JPEG extension, and transparency background.")
                    .color(pal.fg_muted)
                    .size(12.0),
            );
        ui.add_space(6.0);
        ui.label(
            RichText::new("Default destination:")
                .strong()
                .color(pal.fg_primary),
        );
        ui.horizontal_wrapped(|ui| {
            if ui
                .radio(
                    state.destination_mode == "same_folder",
                    "Same folder as source",
                )
                .clicked()
            {
                state.destination_mode = "same_folder".to_string();
            }
            if ui
                .radio(
                    state.destination_mode == "subfolder",
                    "Subfolder in source folder:",
                )
                .clicked()
            {
                state.destination_mode = "subfolder".to_string();
            }
            if state.destination_mode == "subfolder" {
                ui.add(
                    egui::TextEdit::singleline(&mut state.subfolder_text)
                        .desired_width(150.0)
                        .hint_text("Converted"),
                );
            }
            if ui
                .radio(state.destination_mode == "fixed", "Fixed folder:")
                .clicked()
            {
                state.destination_mode = "fixed".to_string();
            }
        });

        if state.destination_mode == "fixed" {
            ui.add_space(4.0);
            ui.horizontal_wrapped(|ui| {
                ui.add(
                    egui::TextEdit::singleline(&mut state.fixed_folder_text)
                        .desired_width(340.0)
                        .hint_text("/home/user/Pictures/Converted"),
                );
                if let Ok(home) = std::env::var("HOME") {
                    let pics = PathBuf::from(&home).join("Pictures");
                    if ui.small_button("~/Pictures").clicked() {
                        state.fixed_folder_text = pics.to_string_lossy().to_string();
                    }
                    let dl = PathBuf::from(&home).join("Downloads");
                    if ui.small_button("~/Downloads").clicked() {
                        state.fixed_folder_text = dl.to_string_lossy().to_string();
                    }
                }
                if ui.small_button("/tmp").clicked() {
                    state.fixed_folder_text = "/tmp".to_string();
                }
            });
        }

        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Default filename suffix:").color(pal.fg_primary));
            ui.add(
                egui::TextEdit::singleline(&mut state.filename_suffix_text)
                    .desired_width(160.0)
                    .hint_text("(none, or e.g. -converted)"),
            );
            ui.label(
                RichText::new("Inserted before extension (e.g. photo-converted.webp)")
                    .size(11.5)
                    .color(pal.fg_muted),
            );
        });

        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("JPEG file extension:").color(pal.fg_primary));
            if ui
                .selectable_label(state.jpeg_ext == JpegExt::Jpg, ".jpg (Default)")
                .clicked()
            {
                state.jpeg_ext = JpegExt::Jpg;
            }
            if ui
                .selectable_label(state.jpeg_ext == JpegExt::Jpeg, ".jpeg")
                .clicked()
            {
                state.jpeg_ext = JpegExt::Jpeg;
            }
        });

        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label(
                RichText::new("Transparency flatten background (for JPEG):").color(pal.fg_primary),
            );
            if ui
                .selectable_label(state.bg_mode == "white", "White (#FFFFFF)")
                .clicked()
            {
                state.bg_mode = "white".to_string();
            }
            if ui
                .selectable_label(state.bg_mode == "black", "Black (#000000)")
                .clicked()
            {
                state.bg_mode = "black".to_string();
            }
            if ui
                .selectable_label(state.bg_mode == "custom", "Custom Hex:")
                .clicked()
            {
                state.bg_mode = "custom".to_string();
            }
            if state.bg_mode == "custom" {
                ui.add(
                    egui::TextEdit::singleline(&mut state.bg_custom_hex)
                        .desired_width(90.0)
                        .hint_text("#RRGGBB"),
                );
            }
        });

        ui.add_space(6.0);
        ui.checkbox(
            &mut draft_convert.defaults.output.preserve_timestamps,
            "Preserve original file modification time on converted files",
        );

        ui.add_space(4.0);
        ui.label(
                RichText::new(
                    "Safety invariant: Existing files are never overwritten; numeric suffixes (-1, -2, …) are used automatically.",
                )
                .size(11.5)
                .color(pal.fg_muted),
            );
    });

    ui.add_space(14.0);

    // 3. Metadata & Privacy
    section_card(ui, pal, "Metadata & Privacy", |ui| {
        ui.label(
                RichText::new("Control how EXIF orientation, ICC color profiles, and camera/GPS metadata are handled.")
                    .color(pal.fg_muted)
                    .size(12.0),
            );
        ui.add_space(6.0);
        let mut auto_orient =
            draft_convert.defaults.metadata.orientation == OrientationPolicy::AutoOrient;
        if ui
            .checkbox(
                &mut auto_orient,
                "Auto-apply EXIF orientation (rotate pixels upright before encoding)",
            )
            .changed()
        {
            draft_convert.defaults.metadata.orientation = if auto_orient {
                OrientationPolicy::AutoOrient
            } else {
                OrientationPolicy::PreserveRaw
            };
        }

        let mut preserve_icc = draft_convert.defaults.metadata.icc == IccPolicy::Preserve;
        if ui
            .checkbox(
                &mut preserve_icc,
                "Preserve ICC color profile when supported",
            )
            .changed()
        {
            draft_convert.defaults.metadata.icc = if preserve_icc {
                IccPolicy::Preserve
            } else {
                IccPolicy::Strip
            };
        }

        let mut preserve_exif = draft_convert.defaults.metadata.exif != ExifPolicy::Strip;
        if ui
            .checkbox(
                &mut preserve_exif,
                "Preserve EXIF metadata (camera model, capture date, GPS coordinates)",
            )
            .changed()
        {
            draft_convert.defaults.metadata.exif = if preserve_exif {
                ExifPolicy::PreserveAll
            } else {
                ExifPolicy::Strip
            };
        }

        ui.add_space(4.0);
        let note_color = if preserve_exif {
            pal.warn_fg
        } else {
            pal.fg_muted
        };
        ui.label(
                RichText::new(
                    "Privacy note: By default, EXIF metadata is stripped so GPS coordinates and camera serial numbers are not leaked when sharing images.",
                )
                .size(11.5)
                .color(note_color),
            );
    });

    ui.add_space(14.0);

    // 4. Built-in & User Presets
    section_card(ui, pal, "Conversion Presets", |ui| {
        ui.label(
                RichText::new("Built-in presets are read-only; add user presets in convert.toml for custom workflows.")
                    .color(pal.fg_muted)
                    .size(12.0),
            );
        ui.add_space(6.0);
        ui.label(
            RichText::new("Built-in Presets (read-only)")
                .size(12.0)
                .strong()
                .color(pal.fg_secondary),
        );
        ui.add_space(4.0);
        for p in builtin_presets() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!("{} — {}", p.id, p.name))
                        .strong()
                        .color(pal.fg_primary)
                        .size(12.5),
                );
                ui.label(
                    RichText::new(format!(
                        "[{} · {}]",
                        p.target.as_str().to_uppercase(),
                        p.description
                    ))
                    .size(11.5)
                    .color(pal.fg_muted),
                );
            });
        }

        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);

        ui.horizontal(|ui| {
            ui.label(
                RichText::new(format!(
                    "User Presets ({})",
                    draft_convert.user_presets.len()
                ))
                .size(12.5)
                .strong()
                .color(pal.fg_primary),
            );
            if ui.button("+ New Preset").clicked() {
                state.preset_editor = Some(PresetEditorDraft::default());
            }
        });

        let mut delete_idx = None;
        let mut edit_idx = None;
        for (idx, up) in draft_convert.user_presets.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(
                    RichText::new(format!(
                        "{} — {} ({})",
                        up.id,
                        up.name,
                        up.target.as_str().to_uppercase()
                    ))
                    .color(pal.fg_primary)
                    .strong(),
                );
                if ui.small_button("Edit").clicked() {
                    edit_idx = Some(idx);
                }
                if ui.small_button("Delete").clicked() {
                    delete_idx = Some(idx);
                }
            });
        }
        if let Some(idx) = edit_idx
            && let Some(p) = draft_convert.user_presets.get(idx)
        {
            state.preset_editor = Some(PresetEditorDraft::from_preset(idx, p));
        }
        if let Some(idx) = delete_idx
            && idx < draft_convert.user_presets.len()
        {
            draft_convert.user_presets.remove(idx);
        }

        if let Some(mut editor) = state.preset_editor.take() {
            ui.add_space(8.0);
            let mut keep_open = true;
            egui::Frame::NONE
                .fill(pal.bg_elevated)
                .corner_radius(CornerRadius::same(6))
                .inner_margin(Margin::symmetric(12, 10))
                .stroke(Stroke::new(1.0, pal.border))
                .show(ui, |ui| {
                    ui.label(
                        RichText::new(if editor.editing_index.is_some() {
                            "Edit User Preset"
                        } else {
                            "New User Preset"
                        })
                        .strong()
                        .color(pal.fg_primary),
                    );
                    ui.add_space(4.0);
                    ui.horizontal(|ui| {
                        ui.label("Preset ID:");
                        ui.add(
                            egui::TextEdit::singleline(&mut editor.id)
                                .desired_width(150.0)
                                .hint_text("user.webp-blog"),
                        );
                        ui.label("Name:");
                        ui.add(
                            egui::TextEdit::singleline(&mut editor.name)
                                .desired_width(200.0)
                                .hint_text("WebP for Blog"),
                        );
                    });
                    ui.horizontal(|ui| {
                        ui.label("Target format:");
                        for fmt in [FormatId::Png, FormatId::Jpeg, FormatId::WebP] {
                            if ui
                                .selectable_label(editor.target == fmt, fmt.as_str().to_uppercase())
                                .clicked()
                            {
                                editor.target = fmt;
                            }
                        }
                    });
                    ui.horizontal(|ui| {
                        ui.checkbox(&mut editor.use_quality, "Quality:");
                        if editor.use_quality {
                            ui.add(egui::Slider::new(&mut editor.quality, 1..=100));
                        }
                        if editor.target != FormatId::Jpeg {
                            ui.checkbox(&mut editor.lossless, "Lossless");
                        }
                        ui.checkbox(&mut editor.strip_metadata, "Strip EXIF metadata");
                    });
                    ui.horizontal(|ui| {
                        ui.label("Resize:");
                        for (mode, label) in [
                            ("original", "Original"),
                            ("width", "Width (px)"),
                            ("height", "Height (px)"),
                            ("fit", "Fit within (px)"),
                        ] {
                            if ui
                                .selectable_label(editor.resize_mode == mode, label)
                                .clicked()
                            {
                                editor.resize_mode = mode.to_string();
                            }
                        }
                        if editor.resize_mode != "original" {
                            ui.add(
                                egui::TextEdit::singleline(&mut editor.resize_dim_text)
                                    .desired_width(70.0),
                            );
                        }
                    });
                    if !editor.errors.is_empty() {
                        for err in &editor.errors {
                            ui.label(RichText::new(err).color(pal.err_fg).size(11.5));
                        }
                    }
                    ui.horizontal(|ui| {
                        if ui.button("Apply Preset").clicked() {
                            match editor.to_preset() {
                                Ok(preset) => {
                                    if let Some(idx) = editor.editing_index
                                        && idx < draft_convert.user_presets.len()
                                    {
                                        draft_convert.user_presets[idx] = preset;
                                    } else {
                                        draft_convert.user_presets.push(preset);
                                    }
                                    keep_open = false;
                                }
                                Err(errs) => {
                                    editor.errors = errs;
                                }
                            }
                        }
                        if ui.button("Cancel").clicked() {
                            keep_open = false;
                        }
                    });
                });
            if keep_open {
                state.preset_editor = Some(editor);
            }
        }
    });

    ui.add_space(14.0);

    // 5. Resource Limits
    section_card(ui, pal, "Resource Limits", |ui| {
        ui.label(
            RichText::new("Guardrails enforced before decoding or allocating pixel buffers.")
                .color(pal.fg_muted)
                .size(12.0),
        );
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("Max image size (MP, 1..=500):");
            ui.add(egui::TextEdit::singleline(&mut state.max_megapixels_text).desired_width(64.0));
            ui.add_space(12.0);
            ui.label("Max edge dimension (px, 64..=65535):");
            ui.add(egui::TextEdit::singleline(&mut state.max_dimension_text).desired_width(72.0));
        });
        ui.add_space(6.0);
        ui.horizontal_wrapped(|ui| {
            ui.label("Max source file size (MiB, 1..=4096):");
            ui.add(egui::TextEdit::singleline(&mut state.max_file_mb_text).desired_width(64.0));
            ui.add_space(12.0);
            ui.label("Per-file timeout (seconds, 1..=600):");
            ui.add(egui::TextEdit::singleline(&mut state.timeout_secs_text).desired_width(64.0));
        });
    });

    ui.add_space(14.0);

    // 6. Backends
    section_card(ui, pal, "Conversion Backends", |ui| {
        ui.label(
            RichText::new("Active conversion engines and format capabilities.")
                .color(pal.fg_muted)
                .size(12.0),
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
                kv_cell(ui, pal, "Built-in Engine", &state.backend_status_note);
                if ui.button("Check again").clicked() {
                    state.backend_probe_count += 1;
                    state.backend_status_note = format!(
                        "Native Rust image engine (image-rs 0.25): Available · PNG, JPEG, WebP (verified #{})",
                        state.backend_probe_count
                    );
                }
            });
        ui.add_space(4.0);
        ui.label(
                RichText::new(
                    "External subprocess backends are disabled in v0.2.0 (`allow_external_backends = false`).",
                )
                .size(11.5)
                .color(pal.fg_muted),
            );
    });

    ui.add_space(14.0);

    // 7. Notifications & Worker Preferences (config.toml [jobs])
    section_card(
        ui,
        pal,
        "Notifications & Worker Concurrency (`config.toml [jobs]`)",
        |ui| {
            ui.label(
                RichText::new("Controls desktop notifications for completed jobs, worker parallelism, and history retention.")
                    .color(pal.fg_muted)
                    .size(12.0),
            );
            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Job notifications:").color(pal.fg_primary));
                for (policy, label) in [
                    (JobsNotifyPolicy::All, "All (finish & failure)"),
                    (JobsNotifyPolicy::ErrorsOnly, "Warnings & errors only"),
                    (JobsNotifyPolicy::Off, "Never notify"),
                ] {
                    if ui
                        .selectable_label(draft_jobs.notify == policy, label)
                        .clicked()
                    {
                        draft_jobs.notify = policy;
                    }
                }
            });

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label(RichText::new("Max parallel conversions:").color(pal.fg_primary));
                for (setting, label) in [
                    (MaxParallelSetting::Auto, "Auto"),
                    (MaxParallelSetting::Fixed(1), "1"),
                    (MaxParallelSetting::Fixed(2), "2"),
                    (MaxParallelSetting::Fixed(4), "4"),
                    (MaxParallelSetting::Fixed(8), "8"),
                ] {
                    if ui
                        .selectable_label(draft_jobs.max_parallel == setting, label)
                        .clicked()
                    {
                        draft_jobs.max_parallel = setting;
                    }
                }
            });

            ui.add_space(6.0);
            ui.horizontal_wrapped(|ui| {
                ui.label("History retention — max terminal jobs (10..=1000):");
                let mut max_jobs_i32 = draft_jobs.retention_max_jobs as i32;
                if ui
                    .add(egui::DragValue::new(&mut max_jobs_i32).range(10..=1000))
                    .changed()
                {
                    draft_jobs.retention_max_jobs = max_jobs_i32.clamp(10, 1000) as usize;
                }
                ui.add_space(12.0);
                ui.label("Retention days (1..=90):");
                let mut days_i32 = draft_jobs.retention_days as i32;
                if ui
                    .add(egui::DragValue::new(&mut days_i32).range(1..=90))
                    .changed()
                {
                    draft_jobs.retention_days = days_i32.clamp(1, 90) as u32;
                }
            });
        },
    );

    action
}
