//! Persistent conversion settings (`convert.toml`), built-in and user presets, and 3-layer option resolution.

use std::collections::BTreeMap;
use std::fs;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use toml_edit::{ArrayOfTables, DocumentMut, Item, Table, value};

use crate::backend::BackendPrefs;
use crate::format::FormatId;
use crate::options::{
    AlphaPolicy, ChromaSubsampling, CollisionPolicy, ConversionOptions, ExifPolicy, FormatQuality,
    IccPolicy, JpegExt, NamingPolicy, OrientationPolicy, OutputLocation, PngCompression,
    QualityIntent, QualitySetting, ResizeSpec, Rgb8, SameFormatPolicy, WebPMode,
};

/// Current schema version for `convert.toml`.
pub const CURRENT_CONVERT_SCHEMA_VERSION: u32 = 1;

/// Partial overrides layered onto base [`ConversionOptions`] by a preset or per-invocation flags.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct PresetOverrides {
    pub quality: Option<QualitySetting>,
    pub resize: Option<ResizeSpec>,
    pub alpha_background: Option<Rgb8>,
    pub orientation: Option<OrientationPolicy>,
    pub icc: Option<IccPolicy>,
    pub exif: Option<ExifPolicy>,
    pub output_location: Option<OutputLocation>,
    pub naming: Option<NamingPolicy>,
    pub collision: Option<CollisionPolicy>,
    pub preserve_timestamps: Option<bool>,
    pub same_format: Option<SameFormatPolicy>,
}

impl PresetOverrides {
    /// Applies every `Some(_)` field in `self` onto `opts`.
    pub fn apply_to(&self, opts: &mut ConversionOptions) {
        if let Some(q) = self.quality {
            opts.quality = q;
        }
        if let Some(r) = self.resize {
            opts.resize = r;
        }
        if let Some(bg) = self.alpha_background {
            opts.alpha = AlphaPolicy { background: bg };
        }
        if let Some(orient) = self.orientation {
            opts.metadata.orientation = orient;
        }
        if let Some(icc) = self.icc {
            opts.metadata.icc = icc;
        }
        if let Some(exif) = self.exif {
            opts.metadata.exif = exif;
        }
        if let Some(ref loc) = self.output_location {
            opts.output.location = loc.clone();
        }
        if let Some(ref naming) = self.naming {
            opts.output.naming = naming.clone();
        }
        if let Some(coll) = self.collision {
            opts.output.collision = coll;
        }
        if let Some(ts) = self.preserve_timestamps {
            opts.output.preserve_timestamps = ts;
        }
        if let Some(sf) = self.same_format {
            opts.same_format = sf;
        }
    }
}

/// Named conversion preset (built-in or user-defined).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Preset {
    pub id: String,
    pub name: String,
    pub description: String,
    pub builtin: bool,
    pub target: FormatId,
    #[serde(default)]
    pub overrides: PresetOverrides,
}

/// Reference to a preset or quick-convert target when resolving [`ConversionOptions`].
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum PresetRef {
    #[default]
    QuickConvert,
    DefaultPreset,
    Target(FormatId),
    PresetId(String),
}

/// Returns the four canonical built-in presets (`CONVERSION_ARCHITECTURE.md` §5.1).
#[must_use]
pub fn builtin_presets() -> Vec<Preset> {
    vec![
        Preset {
            id: "builtin.web-webp".to_string(),
            name: "Web-ready WebP".to_string(),
            description: "Balanced lossy WebP (q=80, method=4) preserving ICC and stripping EXIF."
                .to_string(),
            builtin: true,
            target: FormatId::WebP,
            overrides: PresetOverrides {
                quality: Some(QualitySetting::PerFormat(FormatQuality::WebP {
                    mode: WebPMode::Lossy,
                    quality: 80,
                    alpha_quality: 100,
                    method: 4,
                })),
                resize: Some(ResizeSpec::Original),
                icc: Some(IccPolicy::Preserve),
                exif: Some(ExifPolicy::Strip),
                orientation: Some(OrientationPolicy::AutoOrient),
                ..PresetOverrides::default()
            },
        },
        Preset {
            id: "builtin.smaller-jpeg".to_string(),
            name: "Smaller JPEG".to_string(),
            description: "Progressive JPEG (q=82, 4:2:0) flattening transparency onto white."
                .to_string(),
            builtin: true,
            target: FormatId::Jpeg,
            overrides: PresetOverrides {
                quality: Some(QualitySetting::PerFormat(FormatQuality::Jpeg {
                    quality: 82,
                    progressive: true,
                    optimize_coding: true,
                    subsampling: ChromaSubsampling::Auto,
                })),
                resize: Some(ResizeSpec::Original),
                alpha_background: Some(Rgb8::WHITE),
                icc: Some(IccPolicy::Preserve),
                exif: Some(ExifPolicy::Strip),
                orientation: Some(OrientationPolicy::AutoOrient),
                ..PresetOverrides::default()
            },
        },
        Preset {
            id: "builtin.lossless-png".to_string(),
            name: "Lossless PNG".to_string(),
            description: "Pixel-exact PNG preserving alpha and ICC profile.".to_string(),
            builtin: true,
            target: FormatId::Png,
            overrides: PresetOverrides {
                quality: Some(QualitySetting::PerFormat(FormatQuality::Png {
                    compression: PngCompression::Default,
                })),
                resize: Some(ResizeSpec::Original),
                icc: Some(IccPolicy::Preserve),
                exif: Some(ExifPolicy::Strip),
                orientation: Some(OrientationPolicy::AutoOrient),
                ..PresetOverrides::default()
            },
        },
        Preset {
            id: "builtin.social-1920".to_string(),
            name: "Social / 1920 px".to_string(),
            description: "High-quality WebP (q=85) fit inside 1920x1920 without upscaling."
                .to_string(),
            builtin: true,
            target: FormatId::WebP,
            overrides: PresetOverrides {
                quality: Some(QualitySetting::PerFormat(FormatQuality::WebP {
                    mode: WebPMode::Lossy,
                    quality: 85,
                    alpha_quality: 100,
                    method: 4,
                })),
                resize: Some(ResizeSpec::Fit {
                    width: NonZeroU32::new(1920).unwrap(),
                    height: NonZeroU32::new(1920).unwrap(),
                    allow_upscale: false,
                }),
                icc: Some(IccPolicy::Preserve),
                exif: Some(ExifPolicy::Strip),
                orientation: Some(OrientationPolicy::AutoOrient),
                ..PresetOverrides::default()
            },
        },
    ]
}

/// Configurable resource limits for conversion safety (`CONVERSION_ARCHITECTURE.md` §5.3, `SECURITY_AND_FILE_SAFETY.md` §6).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ResourceLimitsConfig {
    pub max_dimension: u32,
    pub max_pixels: u64,
    pub max_alloc_bytes: u64,
    pub max_source_bytes: u64,
    pub per_item_timeout_secs: u64,
    pub worker_memory_max_bytes: u64,
    pub worker_cpu_quota_percent: u32,
    pub max_concurrency: u32,
}

impl Default for ResourceLimitsConfig {
    fn default() -> Self {
        Self {
            max_dimension: 16_384,
            max_pixels: 100_000_000,
            max_alloc_bytes: 268_435_456,
            max_source_bytes: 536_870_912,
            per_item_timeout_secs: 60,
            worker_memory_max_bytes: 1_073_741_824,
            worker_cpu_quota_percent: 200,
            max_concurrency: 0,
        }
    }
}

/// Diagnostic warning produced during lenient `convert.toml` loading.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingsWarning {
    pub field: String,
    pub message: String,
    pub schema_newer_than_supported: bool,
}

/// Complete persistent conversion settings (`~/.config/radivine/convert.toml`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConvertSettings {
    pub schema_version: u32,
    pub quick_convert_target: FormatId,
    pub default_preset: String,
    #[serde(default)]
    pub quick_convert_presets: BTreeMap<FormatId, String>,
    pub defaults: ConversionOptions,
    pub limits: ResourceLimitsConfig,
    pub backends: BackendPrefs,
    pub user_presets: Vec<Preset>,
}

impl Default for ConvertSettings {
    fn default() -> Self {
        Self {
            schema_version: CURRENT_CONVERT_SCHEMA_VERSION,
            quick_convert_target: FormatId::WebP,
            default_preset: "builtin.web-webp".to_string(),
            quick_convert_presets: BTreeMap::new(),
            defaults: ConversionOptions::default(),
            limits: ResourceLimitsConfig::default(),
            backends: BackendPrefs::default(),
            user_presets: Vec::new(),
        }
    }
}

impl ConvertSettings {
    /// Returns the default built-in preset ID for `fmt`.
    #[must_use]
    pub const fn default_quick_convert_preset_id(fmt: FormatId) -> &'static str {
        match fmt {
            FormatId::Png => "builtin.lossless-png",
            FormatId::Jpeg => "builtin.smaller-jpeg",
            FormatId::WebP => "builtin.web-webp",
        }
    }

    /// Returns the active Quick Convert preset ID for `fmt` (falling back to the canonical built-in preset).
    #[must_use]
    pub fn quick_convert_preset_for(&self, fmt: FormatId) -> String {
        if let Some(id) = self.quick_convert_presets.get(&fmt)
            && let Some(p) = self.find_preset(id)
            && p.target == fmt
        {
            return id.clone();
        }
        Self::default_quick_convert_preset_id(fmt).to_string()
    }

    /// Sets the Quick Convert preset ID for `fmt`.
    pub fn set_quick_convert_preset(&mut self, fmt: FormatId, preset_id: impl Into<String>) {
        let id = preset_id.into();
        if id == Self::default_quick_convert_preset_id(fmt) {
            self.quick_convert_presets.remove(&fmt);
        } else {
            self.quick_convert_presets.insert(fmt, id);
        }
    }

    /// Returns all available presets (built-in presets followed by `user_presets`).
    #[must_use]
    pub fn all_presets(&self) -> Vec<Preset> {
        let mut all = builtin_presets();
        all.extend(self.user_presets.iter().cloned());
        all
    }

    /// Finds a preset by `id` across built-in and user presets.
    #[must_use]
    pub fn find_preset(&self, id: &str) -> Option<Preset> {
        builtin_presets()
            .into_iter()
            .find(|p| p.id == id)
            .or_else(|| self.user_presets.iter().find(|p| p.id == id).cloned())
    }

    /// Resolves final [`ConversionOptions`] via 3-layer inheritance:
    ///
    /// `self.defaults` <- `preset_ref` <- `explicit_overrides`
    pub fn resolve(
        &self,
        preset_ref: &PresetRef,
        explicit_overrides: Option<&PresetOverrides>,
    ) -> Result<ConversionOptions, String> {
        let mut opts = self.defaults.clone();

        match preset_ref {
            PresetRef::QuickConvert => {
                opts.target = self.quick_convert_target;
                opts.quality = QualitySetting::Intent(opts.quality_intent_or_balanced());
            }
            PresetRef::DefaultPreset => {
                if let Some(preset) = self.find_preset(&self.default_preset) {
                    opts.target = preset.target;
                    preset.overrides.apply_to(&mut opts);
                } else {
                    opts.target = self.quick_convert_target;
                }
            }
            PresetRef::Target(fmt) => {
                opts.target = *fmt;
                if let Some(id) = self.quick_convert_presets.get(fmt)
                    && let Some(preset) = self.find_preset(id)
                    && preset.target == *fmt
                {
                    preset.overrides.apply_to(&mut opts);
                } else if let QualitySetting::PerFormat(fq) = opts.quality
                    && fq.format_id() != *fmt
                {
                    opts.quality = QualitySetting::Intent(QualityIntent::Balanced);
                }
            }
            PresetRef::PresetId(id) => {
                let preset = self
                    .find_preset(id)
                    .ok_or_else(|| format!("unknown preset id: {id:?}"))?;
                opts.target = preset.target;
                preset.overrides.apply_to(&mut opts);
            }
        }

        if let Some(overrides) = explicit_overrides {
            overrides.apply_to(&mut opts);
        }

        Ok(opts)
    }

    /// Loads `convert.toml` from `path`, falling back to [`ConvertSettings::default()`] if the
    /// file does not exist or has a syntax error.
    #[must_use]
    pub fn load_or_default(path: &Path) -> (Self, Vec<SettingsWarning>) {
        match fs::read_to_string(path) {
            Ok(content) => match Self::from_toml_str(&content) {
                Ok((settings, warnings)) => (settings, warnings),
                Err(err) => (
                    Self::default(),
                    vec![SettingsWarning {
                        field: "convert.toml".to_string(),
                        message: format!("TOML syntax error, using defaults: {err}"),
                        schema_newer_than_supported: false,
                    }],
                ),
            },
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => (Self::default(), Vec::new()),
            Err(err) => (
                Self::default(),
                vec![SettingsWarning {
                    field: "convert.toml".to_string(),
                    message: format!("failed to read file, using defaults: {err}"),
                    schema_newer_than_supported: false,
                }],
            ),
        }
    }

    /// Lenient runtime parser for `convert.toml`.
    ///
    /// - Unknown keys are ignored.
    /// - Missing keys use [`ConvertSettings::default()`].
    /// - Out-of-range fields emit a [`SettingsWarning`] and fall back to safe defaults for that field only.
    /// - If `schema_version > CURRENT_CONVERT_SCHEMA_VERSION`, a warning with
    ///   `schema_newer_than_supported = true` is returned while preserving recognized fields.
    pub fn from_toml_str(s: &str) -> Result<(Self, Vec<SettingsWarning>), String> {
        let root: toml::Table = toml::from_str(s).map_err(|e| e.to_string())?;
        let mut settings = Self::default();
        let mut warnings = Vec::new();

        let conv_table = root
            .get("convert")
            .and_then(toml::Value::as_table)
            .unwrap_or(&root);

        if let Some(ver_val) = conv_table.get("schema_version") {
            match ver_val.as_integer() {
                Some(v) if v >= 1 => {
                    let v_u32 = u32::try_from(v).unwrap_or(u32::MAX);
                    settings.schema_version = v_u32;
                    if v_u32 > CURRENT_CONVERT_SCHEMA_VERSION {
                        warnings.push(SettingsWarning {
                            field: "convert.schema_version".to_string(),
                            message: format!(
                                "schema_version {v_u32} is newer than supported version {CURRENT_CONVERT_SCHEMA_VERSION}"
                            ),
                            schema_newer_than_supported: true,
                        });
                    }
                }
                _ => warnings.push(SettingsWarning {
                    field: "convert.schema_version".to_string(),
                    message: "invalid schema_version, defaulting to 1".to_string(),
                    schema_newer_than_supported: false,
                }),
            }
        }

        if let Some(qct) = conv_table.get("quick_convert_target") {
            match qct.as_str().and_then(|v| v.parse::<FormatId>().ok()) {
                Some(fmt) => {
                    settings.quick_convert_target = fmt;
                }
                None => warnings.push(SettingsWarning {
                    field: "convert.quick_convert_target".to_string(),
                    message: format!("invalid quick_convert_target {qct:?}, defaulting to webp"),
                    schema_newer_than_supported: false,
                }),
            }
        }

        if let Some(dp) = conv_table.get("default_preset") {
            if let Some(dp_str) = dp.as_str() {
                if !dp_str.trim().is_empty() {
                    settings.default_preset = dp_str.trim().to_string();
                }
            } else {
                warnings.push(SettingsWarning {
                    field: "convert.default_preset".to_string(),
                    message: "default_preset must be a string".to_string(),
                    schema_newer_than_supported: false,
                });
            }
        }

        if let Some(qcp_tbl) = conv_table
            .get("quick_convert_presets")
            .and_then(toml::Value::as_table)
        {
            for (k, v) in qcp_tbl {
                if let (Ok(fmt), Some(pid)) = (k.parse::<FormatId>(), v.as_str()) {
                    let pid = pid.trim();
                    if !pid.is_empty() {
                        settings.quick_convert_presets.insert(fmt, pid.to_string());
                    }
                } else {
                    warnings.push(SettingsWarning {
                        field: format!("convert.quick_convert_presets.{k}"),
                        message: "invalid format or preset id in quick_convert_presets".to_string(),
                        schema_newer_than_supported: false,
                    });
                }
            }
        }

        if let Some(defs) = conv_table.get("defaults").and_then(toml::Value::as_table) {
            parse_defaults_table(defs, &mut settings.defaults, &mut warnings);
        }

        if let Some(lims) = conv_table.get("limits").and_then(toml::Value::as_table) {
            parse_limits_table(lims, &mut settings.limits, &mut warnings);
        }

        if let Some(bks) = conv_table.get("backends").and_then(toml::Value::as_table) {
            parse_backends_table(bks, &mut settings.backends, &mut warnings);
        }

        if let Some(presets_arr) = conv_table.get("presets").and_then(toml::Value::as_array) {
            for (idx, item) in presets_arr.iter().enumerate() {
                if let Some(tbl) = item.as_table()
                    && let Some(preset) = parse_user_preset_table(tbl, idx, &mut warnings)
                {
                    settings.user_presets.push(preset);
                }
            }
        }

        // Validate default_preset exists in builtin or user_presets; otherwise warn and reset
        if settings.find_preset(&settings.default_preset).is_none() {
            warnings.push(SettingsWarning {
                field: "convert.default_preset".to_string(),
                message: format!(
                    "unknown preset {:?}, falling back to \"builtin.web-webp\"",
                    settings.default_preset
                ),
                schema_newer_than_supported: false,
            });
            settings.default_preset = "builtin.web-webp".to_string();
        }

        // Validate quick_convert_presets entries exist and match their format
        let qcp_keys: Vec<(FormatId, String)> = settings
            .quick_convert_presets
            .iter()
            .map(|(k, v)| (*k, v.clone()))
            .collect();
        for (fmt, pid) in qcp_keys {
            match settings.find_preset(&pid) {
                Some(p) if p.target == fmt => {}
                Some(p) => {
                    warnings.push(SettingsWarning {
                        field: format!("convert.quick_convert_presets.{fmt}"),
                        message: format!("preset {pid:?} targets {}, not {fmt}", p.target),
                        schema_newer_than_supported: false,
                    });
                    settings.quick_convert_presets.remove(&fmt);
                }
                None => {
                    warnings.push(SettingsWarning {
                        field: format!("convert.quick_convert_presets.{fmt}"),
                        message: format!("unknown preset {pid:?}"),
                        schema_newer_than_supported: false,
                    });
                    settings.quick_convert_presets.remove(&fmt);
                }
            }
        }

        Ok((settings, warnings))
    }

    /// Strict validator for `convert.toml` used by `radivine-control` save and `radivine config validate`.
    ///
    /// Rejects any syntax error or field warning as an error list.
    pub fn validate_strict(s: &str) -> Result<Self, Vec<String>> {
        match Self::from_toml_str(s) {
            Ok((settings, warnings)) => {
                let errs: Vec<String> = warnings
                    .into_iter()
                    .filter(|w| !w.schema_newer_than_supported)
                    .map(|w| format!("{}: {}", w.field, w.message))
                    .collect();
                if errs.is_empty() {
                    Ok(settings)
                } else {
                    Err(errs)
                }
            }
            Err(syntax_err) => Err(vec![syntax_err]),
        }
    }

    /// Serializes `self` into `existing_toml` using `toml_edit`, preserving user comments and
    /// unrelated top-level sections (`AC-C1.13`).
    pub fn save_preserving_comments(&self, existing_toml: &str) -> Result<String, String> {
        let mut doc: DocumentMut = if existing_toml.trim().is_empty() {
            DocumentMut::new()
        } else {
            existing_toml
                .parse::<DocumentMut>()
                .map_err(|e| e.to_string())?
        };

        if !doc.contains_key("convert") {
            doc["convert"] = Item::Table(Table::new());
        }
        let conv = doc["convert"]
            .as_table_mut()
            .ok_or_else(|| "[convert] is not a table".to_string())?;

        conv["schema_version"] = value(i64::from(self.schema_version));
        conv["quick_convert_target"] = value(self.quick_convert_target.as_str());
        conv["default_preset"] = value(self.default_preset.as_str());

        if self.quick_convert_presets.is_empty() {
            conv.remove("quick_convert_presets");
        } else {
            if !conv.contains_key("quick_convert_presets") {
                conv["quick_convert_presets"] = Item::Table(Table::new());
            }
            if let Some(qcp_tbl) = conv["quick_convert_presets"].as_table_mut() {
                qcp_tbl.clear();
                for (fmt, pid) in &self.quick_convert_presets {
                    qcp_tbl[fmt.as_str()] = value(pid.as_str());
                }
            }
        }

        // [convert.defaults]
        if !conv.contains_key("defaults") {
            conv["defaults"] = Item::Table(Table::new());
        }
        if let Some(defs) = conv["defaults"].as_table_mut() {
            let intent_str = match self.defaults.quality {
                QualitySetting::Intent(QualityIntent::Lossless) => "lossless".to_string(),
                QualitySetting::Intent(QualityIntent::Maximum) => "maximum".to_string(),
                QualitySetting::Intent(QualityIntent::High) => "high".to_string(),
                QualitySetting::Intent(QualityIntent::Balanced) => "balanced".to_string(),
                QualitySetting::Intent(QualityIntent::Compact) => "compact".to_string(),
                QualitySetting::Intent(QualityIntent::Custom(q)) => format!("custom:{q}"),
                QualitySetting::PerFormat(_) => "balanced".to_string(),
            };
            defs["quality_intent"] = value(intent_str);
            defs["same_format"] = value(match self.defaults.same_format {
                SameFormatPolicy::Skip => "skip",
                SameFormatPolicy::Reencode => "reencode",
            });
            defs["alpha_background"] = value(self.defaults.alpha.background.to_hex());
            defs["orientation"] = value(match self.defaults.metadata.orientation {
                OrientationPolicy::AutoOrient => "auto_orient",
                OrientationPolicy::PreserveRaw => "preserve_raw",
            });
            defs["icc"] = value(match self.defaults.metadata.icc {
                IccPolicy::Preserve => "preserve",
                IccPolicy::Strip => "strip",
            });
            defs["exif"] = value(match self.defaults.metadata.exif {
                ExifPolicy::Strip => "strip",
                ExifPolicy::KeepSafe => "keep_safe",
                ExifPolicy::PreserveAll => "preserve_all",
            });
            match &self.defaults.output.location {
                OutputLocation::SameDirectory => {
                    defs["output_location"] = value("same_directory");
                    defs.remove("output_subfolder");
                    defs.remove("output_directory");
                }
                OutputLocation::Subfolder(sub) => {
                    defs["output_location"] = value("subfolder");
                    defs["output_subfolder"] = value(sub.as_str());
                    defs.remove("output_directory");
                }
                OutputLocation::Directory(dir) => {
                    defs["output_location"] = value("directory");
                    defs["output_directory"] = value(dir.to_string_lossy().as_ref());
                    defs.remove("output_subfolder");
                }
            }
            match &self.defaults.output.naming {
                NamingPolicy::PreserveStem { jpeg_ext } => {
                    defs["jpeg_extension"] = value(jpeg_ext.as_str());
                    defs.remove("filename_suffix");
                }
                NamingPolicy::AppendSuffix { suffix, jpeg_ext } => {
                    defs["jpeg_extension"] = value(jpeg_ext.as_str());
                    defs["filename_suffix"] = value(suffix.as_str());
                }
            }
            defs["collision"] = value(match self.defaults.output.collision {
                CollisionPolicy::Suffix => "suffix",
                CollisionPolicy::Skip => "skip",
                CollisionPolicy::Fail => "fail",
            });
            defs["preserve_timestamps"] = value(self.defaults.output.preserve_timestamps);
        }

        // [convert.limits]
        if !conv.contains_key("limits") {
            conv["limits"] = Item::Table(Table::new());
        }
        if let Some(lims) = conv["limits"].as_table_mut() {
            lims["max_dimension"] = value(i64::from(self.limits.max_dimension));
            lims["max_pixels"] = value(i64::try_from(self.limits.max_pixels).unwrap_or(i64::MAX));
            lims["max_alloc_bytes"] =
                value(i64::try_from(self.limits.max_alloc_bytes).unwrap_or(i64::MAX));
            lims["max_source_bytes"] =
                value(i64::try_from(self.limits.max_source_bytes).unwrap_or(i64::MAX));
            lims["per_item_timeout_secs"] =
                value(i64::try_from(self.limits.per_item_timeout_secs).unwrap_or(60));
            lims["worker_memory_max_bytes"] =
                value(i64::try_from(self.limits.worker_memory_max_bytes).unwrap_or(i64::MAX));
            lims["worker_cpu_quota_percent"] =
                value(i64::from(self.limits.worker_cpu_quota_percent));
            lims["max_concurrency"] = value(i64::from(self.limits.max_concurrency));
        }

        // [convert.backends]
        if !conv.contains_key("backends") {
            conv["backends"] = Item::Table(Table::new());
        }
        if let Some(bks) = conv["backends"].as_table_mut() {
            bks["allow_external_backends"] = value(self.backends.allow_external_backends);
            if !bks.contains_key("preferred_for") {
                bks["preferred_for"] = Item::Table(Table::new());
            }
            if let Some(pref_tbl) = bks["preferred_for"].as_table_mut() {
                pref_tbl.clear();
                for (fmt, backend_id) in &self.backends.preferred_for {
                    pref_tbl[fmt.as_str()] = value(backend_id.as_str());
                }
            }
        }

        // [[convert.presets]]
        if self.user_presets.is_empty() {
            conv.remove("presets");
        } else {
            let mut aot = ArrayOfTables::new();
            for p in &self.user_presets {
                let mut t = Table::new();
                t["id"] = value(p.id.as_str());
                t["name"] = value(p.name.as_str());
                t["description"] = value(p.description.as_str());
                t["target"] = value(p.target.as_str());
                serialize_preset_overrides_into_table(&p.overrides, &mut t);
                aot.push(t);
            }
            conv["presets"] = Item::ArrayOfTables(aot);
        }

        Ok(doc.to_string())
    }
}

impl ConversionOptions {
    fn quality_intent_or_balanced(&self) -> QualityIntent {
        match self.quality {
            QualitySetting::Intent(intent) => intent,
            QualitySetting::PerFormat(_) => QualityIntent::Balanced,
        }
    }
}

fn parse_quality_intent_str(s: &str) -> Option<QualityIntent> {
    let trimmed = s.trim();
    if trimmed.eq_ignore_ascii_case("lossless") {
        Some(QualityIntent::Lossless)
    } else if trimmed.eq_ignore_ascii_case("maximum") {
        Some(QualityIntent::Maximum)
    } else if trimmed.eq_ignore_ascii_case("high") {
        Some(QualityIntent::High)
    } else if trimmed.eq_ignore_ascii_case("balanced") {
        Some(QualityIntent::Balanced)
    } else if trimmed.eq_ignore_ascii_case("compact") {
        Some(QualityIntent::Compact)
    } else if let Some(rest) = trimmed.strip_prefix("custom:") {
        let q: u8 = rest.trim().parse().ok()?;
        (1..=100).contains(&q).then_some(QualityIntent::Custom(q))
    } else {
        None
    }
}

fn parse_defaults_table(
    defs: &toml::Table,
    out: &mut ConversionOptions,
    warnings: &mut Vec<SettingsWarning>,
) {
    if let Some(qi) = defs.get("quality_intent") {
        match qi.as_str().and_then(parse_quality_intent_str) {
            Some(intent) => out.quality = QualitySetting::Intent(intent),
            None => warnings.push(SettingsWarning {
                field: "convert.defaults.quality_intent".to_string(),
                message: format!("invalid quality_intent {qi:?}, using balanced"),
                schema_newer_than_supported: false,
            }),
        }
    }

    if let Some(sf) = defs.get("same_format") {
        match sf.as_str() {
            Some("skip") => out.same_format = SameFormatPolicy::Skip,
            Some("reencode") => out.same_format = SameFormatPolicy::Reencode,
            _ => warnings.push(SettingsWarning {
                field: "convert.defaults.same_format".to_string(),
                message: format!("invalid same_format {sf:?}, using skip"),
                schema_newer_than_supported: false,
            }),
        }
    }

    if let Some(bg) = defs.get("alpha_background") {
        match bg.as_str().and_then(|s| s.parse::<Rgb8>().ok()) {
            Some(rgb) => out.alpha.background = rgb,
            None => warnings.push(SettingsWarning {
                field: "convert.defaults.alpha_background".to_string(),
                message: format!("invalid alpha_background {bg:?}, using #FFFFFF"),
                schema_newer_than_supported: false,
            }),
        }
    }

    if let Some(or_val) = defs.get("orientation") {
        match or_val.as_str() {
            Some("auto_orient") => out.metadata.orientation = OrientationPolicy::AutoOrient,
            Some("preserve_raw") => out.metadata.orientation = OrientationPolicy::PreserveRaw,
            _ => warnings.push(SettingsWarning {
                field: "convert.defaults.orientation".to_string(),
                message: format!("invalid orientation {or_val:?}, using auto_orient"),
                schema_newer_than_supported: false,
            }),
        }
    }

    if let Some(icc_val) = defs.get("icc") {
        match icc_val.as_str() {
            Some("preserve") => out.metadata.icc = IccPolicy::Preserve,
            Some("strip") => out.metadata.icc = IccPolicy::Strip,
            _ => warnings.push(SettingsWarning {
                field: "convert.defaults.icc".to_string(),
                message: format!("invalid icc {icc_val:?}, using preserve"),
                schema_newer_than_supported: false,
            }),
        }
    }

    if let Some(exif_val) = defs.get("exif") {
        match exif_val.as_str() {
            Some("strip") => out.metadata.exif = ExifPolicy::Strip,
            Some("keep_safe") => out.metadata.exif = ExifPolicy::KeepSafe,
            Some("preserve_all") => out.metadata.exif = ExifPolicy::PreserveAll,
            _ => warnings.push(SettingsWarning {
                field: "convert.defaults.exif".to_string(),
                message: format!("invalid exif {exif_val:?}, using strip"),
                schema_newer_than_supported: false,
            }),
        }
    }

    if let Some(loc_val) = defs.get("output_location") {
        match loc_val.as_str() {
            Some("same_directory") => out.output.location = OutputLocation::SameDirectory,
            Some("subfolder") => {
                let sub = defs
                    .get("output_subfolder")
                    .and_then(toml::Value::as_str)
                    .unwrap_or("Converted")
                    .trim();
                if sub.is_empty()
                    || sub == "."
                    || sub == ".."
                    || sub.contains('/')
                    || sub.contains('\\')
                    || sub.contains("..")
                {
                    warnings.push(SettingsWarning {
                        field: "convert.defaults.output_subfolder".to_string(),
                        message: format!(
                            "invalid output_subfolder {sub:?}, falling back to same_directory"
                        ),
                        schema_newer_than_supported: false,
                    });
                    out.output.location = OutputLocation::SameDirectory;
                } else {
                    out.output.location = OutputLocation::Subfolder(sub.to_string());
                }
            }
            Some("directory") => {
                let dir_str = defs
                    .get("output_directory")
                    .and_then(toml::Value::as_str)
                    .unwrap_or("");
                let pb = PathBuf::from(dir_str);
                if pb.is_absolute() {
                    out.output.location = OutputLocation::Directory(pb);
                } else {
                    warnings.push(SettingsWarning {
                        field: "convert.defaults.output_directory".to_string(),
                        message: format!(
                            "output_directory {dir_str:?} must be absolute, falling back to same_directory"
                        ),
                        schema_newer_than_supported: false,
                    });
                    out.output.location = OutputLocation::SameDirectory;
                }
            }
            _ => warnings.push(SettingsWarning {
                field: "convert.defaults.output_location".to_string(),
                message: format!("invalid output_location {loc_val:?}, using same_directory"),
                schema_newer_than_supported: false,
            }),
        }
    }

    let mut jpeg_ext = out.output.naming.jpeg_ext();
    if let Some(jext) = defs.get("jpeg_extension") {
        match jext.as_str() {
            Some("jpg") => {
                jpeg_ext = JpegExt::Jpg;
                out.output.naming = NamingPolicy::PreserveStem { jpeg_ext };
            }
            Some("jpeg") => {
                jpeg_ext = JpegExt::Jpeg;
                out.output.naming = NamingPolicy::PreserveStem { jpeg_ext };
            }
            _ => warnings.push(SettingsWarning {
                field: "convert.defaults.jpeg_extension".to_string(),
                message: format!("invalid jpeg_extension {jext:?}, using jpg"),
                schema_newer_than_supported: false,
            }),
        }
    }

    if let Some(suf_val) = defs.get("filename_suffix") {
        if let Some(suf) = suf_val.as_str() {
            let trimmed = suf.trim();
            if trimmed.is_empty() {
                out.output.naming = NamingPolicy::PreserveStem { jpeg_ext };
            } else if trimmed.contains('/')
                || trimmed.contains('\\')
                || trimmed.contains('\0')
                || trimmed.contains("..")
            {
                warnings.push(SettingsWarning {
                    field: "convert.defaults.filename_suffix".to_string(),
                    message: format!(
                        "invalid filename_suffix {trimmed:?} (must not contain path separators or '..')"
                    ),
                    schema_newer_than_supported: false,
                });
            } else {
                out.output.naming = NamingPolicy::AppendSuffix {
                    suffix: trimmed.to_string(),
                    jpeg_ext,
                };
            }
        } else {
            warnings.push(SettingsWarning {
                field: "convert.defaults.filename_suffix".to_string(),
                message: "filename_suffix must be a string".to_string(),
                schema_newer_than_supported: false,
            });
        }
    }

    if let Some(coll_val) = defs.get("collision") {
        match coll_val.as_str() {
            Some("suffix") => out.output.collision = CollisionPolicy::Suffix,
            Some("skip") => out.output.collision = CollisionPolicy::Skip,
            Some("fail") => out.output.collision = CollisionPolicy::Fail,
            _ => warnings.push(SettingsWarning {
                field: "convert.defaults.collision".to_string(),
                message: format!("invalid collision {coll_val:?}, using suffix"),
                schema_newer_than_supported: false,
            }),
        }
    }

    if let Some(ts_val) = defs.get("preserve_timestamps") {
        match ts_val.as_bool() {
            Some(b) => out.output.preserve_timestamps = b,
            None => warnings.push(SettingsWarning {
                field: "convert.defaults.preserve_timestamps".to_string(),
                message: "preserve_timestamps must be a boolean".to_string(),
                schema_newer_than_supported: false,
            }),
        }
    }
}

fn serialize_preset_overrides_into_table(ov: &PresetOverrides, t: &mut Table) {
    if let Some(q) = ov.quality {
        match q {
            QualitySetting::Intent(intent) => {
                let s = match intent {
                    QualityIntent::Lossless => "lossless".to_string(),
                    QualityIntent::Maximum => "maximum".to_string(),
                    QualityIntent::High => "high".to_string(),
                    QualityIntent::Balanced => "balanced".to_string(),
                    QualityIntent::Compact => "compact".to_string(),
                    QualityIntent::Custom(q) => format!("custom:{q}"),
                };
                t["quality_intent"] = value(s);
            }
            QualitySetting::PerFormat(FormatQuality::Jpeg {
                quality,
                progressive,
                optimize_coding,
                subsampling,
            }) => {
                t["jpeg_quality"] = value(i64::from(quality));
                t["jpeg_progressive"] = value(progressive);
                t["jpeg_optimize_coding"] = value(optimize_coding);
                t["jpeg_subsampling"] = value(match subsampling {
                    ChromaSubsampling::Auto => "auto",
                    ChromaSubsampling::Cs444 => "444",
                    ChromaSubsampling::Cs422 => "422",
                    ChromaSubsampling::Cs420 => "420",
                });
            }
            QualitySetting::PerFormat(FormatQuality::WebP {
                mode,
                quality,
                alpha_quality,
                method,
            }) => {
                t["webp_mode"] = value(match mode {
                    WebPMode::Lossy => "lossy",
                    WebPMode::Lossless => "lossless",
                });
                t["webp_quality"] = value(i64::from(quality));
                t["webp_alpha_quality"] = value(i64::from(alpha_quality));
                t["webp_method"] = value(i64::from(method));
            }
            QualitySetting::PerFormat(FormatQuality::Png { compression }) => {
                t["png_compression"] = value(match compression {
                    PngCompression::Fast => "fast",
                    PngCompression::Default => "default",
                    PngCompression::Best => "best",
                });
            }
        }
    }
    if let Some(resize) = ov.resize {
        match resize {
            ResizeSpec::Original => {
                t["resize_mode"] = value("original");
            }
            ResizeSpec::Width { px, allow_upscale } => {
                t["resize_mode"] = value("width");
                t["resize_width"] = value(i64::from(px.get()));
                t["allow_upscale"] = value(allow_upscale);
            }
            ResizeSpec::Height { px, allow_upscale } => {
                t["resize_mode"] = value("height");
                t["resize_height"] = value(i64::from(px.get()));
                t["allow_upscale"] = value(allow_upscale);
            }
            ResizeSpec::Fit {
                width,
                height,
                allow_upscale,
            } => {
                t["resize_mode"] = value("fit");
                t["resize_width"] = value(i64::from(width.get()));
                t["resize_height"] = value(i64::from(height.get()));
                t["allow_upscale"] = value(allow_upscale);
            }
        }
    }
    if let Some(bg) = ov.alpha_background {
        t["alpha_background"] = value(bg.to_hex());
    }
    if let Some(or_pol) = ov.orientation {
        t["orientation"] = value(match or_pol {
            OrientationPolicy::AutoOrient => "auto_orient",
            OrientationPolicy::PreserveRaw => "preserve_raw",
        });
    }
    if let Some(icc_pol) = ov.icc {
        t["icc"] = value(match icc_pol {
            IccPolicy::Preserve => "preserve",
            IccPolicy::Strip => "strip",
        });
    }
    if let Some(exif_pol) = ov.exif {
        t["exif"] = value(match exif_pol {
            ExifPolicy::Strip => "strip",
            ExifPolicy::KeepSafe => "keep_safe",
            ExifPolicy::PreserveAll => "preserve_all",
        });
    }
    if let Some(sf) = ov.same_format {
        t["same_format"] = value(match sf {
            SameFormatPolicy::Skip => "skip",
            SameFormatPolicy::Reencode => "reencode",
        });
    }
}

fn parse_limits_table(
    lims: &toml::Table,
    out: &mut ResourceLimitsConfig,
    warnings: &mut Vec<SettingsWarning>,
) {
    if let Some(v) = lims.get("max_dimension") {
        match v.as_integer().and_then(|n| u32::try_from(n).ok()) {
            Some(d) if (64..=65_535).contains(&d) => out.max_dimension = d,
            _ => warnings.push(SettingsWarning {
                field: "convert.limits.max_dimension".to_string(),
                message: format!("invalid max_dimension {v:?}, using {}", out.max_dimension),
                schema_newer_than_supported: false,
            }),
        }
    }
    if let Some(v) = lims.get("max_pixels") {
        match v.as_integer().and_then(|n| u64::try_from(n).ok()) {
            Some(p) if (4_096..=1_000_000_000).contains(&p) => out.max_pixels = p,
            _ => warnings.push(SettingsWarning {
                field: "convert.limits.max_pixels".to_string(),
                message: format!("invalid max_pixels {v:?}, using {}", out.max_pixels),
                schema_newer_than_supported: false,
            }),
        }
    }
    if let Some(v) = lims.get("max_alloc_bytes") {
        match v.as_integer().and_then(|n| u64::try_from(n).ok()) {
            Some(b) if b >= 16 * 1024 * 1024 => out.max_alloc_bytes = b,
            _ => warnings.push(SettingsWarning {
                field: "convert.limits.max_alloc_bytes".to_string(),
                message: format!(
                    "invalid max_alloc_bytes {v:?}, using {}",
                    out.max_alloc_bytes
                ),
                schema_newer_than_supported: false,
            }),
        }
    }
    if let Some(v) = lims.get("max_source_bytes") {
        match v.as_integer().and_then(|n| u64::try_from(n).ok()) {
            Some(b) if b >= 1024 * 1024 => out.max_source_bytes = b,
            _ => warnings.push(SettingsWarning {
                field: "convert.limits.max_source_bytes".to_string(),
                message: format!(
                    "invalid max_source_bytes {v:?}, using {}",
                    out.max_source_bytes
                ),
                schema_newer_than_supported: false,
            }),
        }
    }
    if let Some(v) = lims.get("per_item_timeout_secs") {
        match v.as_integer().and_then(|n| u64::try_from(n).ok()) {
            Some(s) if (1..=3600).contains(&s) => out.per_item_timeout_secs = s,
            _ => warnings.push(SettingsWarning {
                field: "convert.limits.per_item_timeout_secs".to_string(),
                message: format!(
                    "invalid per_item_timeout_secs {v:?}, using {}",
                    out.per_item_timeout_secs
                ),
                schema_newer_than_supported: false,
            }),
        }
    }
    if let Some(v) = lims.get("worker_memory_max_bytes") {
        match v.as_integer().and_then(|n| u64::try_from(n).ok()) {
            Some(b) if b >= 64 * 1024 * 1024 => out.worker_memory_max_bytes = b,
            _ => warnings.push(SettingsWarning {
                field: "convert.limits.worker_memory_max_bytes".to_string(),
                message: format!(
                    "invalid worker_memory_max_bytes {v:?}, using {}",
                    out.worker_memory_max_bytes
                ),
                schema_newer_than_supported: false,
            }),
        }
    }
    if let Some(v) = lims.get("worker_cpu_quota_percent") {
        match v.as_integer().and_then(|n| u32::try_from(n).ok()) {
            Some(p) if (10..=6400).contains(&p) => out.worker_cpu_quota_percent = p,
            _ => warnings.push(SettingsWarning {
                field: "convert.limits.worker_cpu_quota_percent".to_string(),
                message: format!(
                    "invalid worker_cpu_quota_percent {v:?}, using {}",
                    out.worker_cpu_quota_percent
                ),
                schema_newer_than_supported: false,
            }),
        }
    }
    if let Some(v) = lims.get("max_concurrency") {
        match v.as_integer().and_then(|n| u32::try_from(n).ok()) {
            Some(c) if c <= 64 => out.max_concurrency = c,
            _ => warnings.push(SettingsWarning {
                field: "convert.limits.max_concurrency".to_string(),
                message: format!(
                    "invalid max_concurrency {v:?}, using {}",
                    out.max_concurrency
                ),
                schema_newer_than_supported: false,
            }),
        }
    }
}

fn parse_backends_table(
    bks: &toml::Table,
    out: &mut BackendPrefs,
    warnings: &mut Vec<SettingsWarning>,
) {
    if let Some(v) = bks.get("allow_external_backends") {
        match v.as_bool() {
            Some(b) => out.allow_external_backends = b,
            None => warnings.push(SettingsWarning {
                field: "convert.backends.allow_external_backends".to_string(),
                message: "allow_external_backends must be a boolean".to_string(),
                schema_newer_than_supported: false,
            }),
        }
    }
    if let Some(pref_tbl) = bks.get("preferred_for").and_then(toml::Value::as_table) {
        let mut map = BTreeMap::new();
        for (k, v) in pref_tbl {
            if let (Ok(fmt), Some(id_str)) = (k.parse::<FormatId>(), v.as_str()) {
                map.insert(fmt, id_str.to_string());
            } else {
                warnings.push(SettingsWarning {
                    field: format!("convert.backends.preferred_for.{k}"),
                    message: "invalid format or backend id in preferred_for".to_string(),
                    schema_newer_than_supported: false,
                });
            }
        }
        out.preferred_for = map;
    }
}

fn parse_user_preset_table(
    tbl: &toml::Table,
    idx: usize,
    warnings: &mut Vec<SettingsWarning>,
) -> Option<Preset> {
    let id = tbl.get("id").and_then(toml::Value::as_str)?.trim();
    if id.is_empty() || id.starts_with("builtin.") {
        warnings.push(SettingsWarning {
            field: format!("convert.presets[{idx}].id"),
            message: format!("invalid or reserved preset id {id:?}"),
            schema_newer_than_supported: false,
        });
        return None;
    }
    let name = tbl
        .get("name")
        .and_then(toml::Value::as_str)
        .unwrap_or(id)
        .to_string();
    let description = tbl
        .get("description")
        .and_then(toml::Value::as_str)
        .unwrap_or("")
        .to_string();
    let Some(target) = tbl
        .get("target")
        .and_then(toml::Value::as_str)
        .and_then(|s| s.parse::<FormatId>().ok())
    else {
        warnings.push(SettingsWarning {
            field: format!("convert.presets[{idx}].target"),
            message: "missing or invalid preset target format".to_string(),
            schema_newer_than_supported: false,
        });
        return None;
    };

    let mut overrides = PresetOverrides::default();
    if let Some(qi) = tbl.get("quality_intent").and_then(toml::Value::as_str) {
        match parse_quality_intent_str(qi) {
            Some(intent) => overrides.quality = Some(QualitySetting::Intent(intent)),
            None => warnings.push(SettingsWarning {
                field: format!("convert.presets[{idx}].quality_intent"),
                message: format!("invalid quality_intent {qi:?}"),
                schema_newer_than_supported: false,
            }),
        }
    }
    if let Some(q_num) = tbl.get("quality").and_then(toml::Value::as_integer)
        && let Ok(q_u8) = u8::try_from(q_num)
        && (1..=100).contains(&q_u8)
    {
        overrides.quality = Some(QualitySetting::Intent(QualityIntent::Custom(q_u8)));
    }
    if let Some(jq) = tbl.get("jpeg_quality").and_then(toml::Value::as_integer)
        && let Ok(q_u8) = u8::try_from(jq)
        && (1..=100).contains(&q_u8)
    {
        let progressive = tbl
            .get("jpeg_progressive")
            .and_then(toml::Value::as_bool)
            .unwrap_or(true);
        let optimize_coding = tbl
            .get("jpeg_optimize_coding")
            .and_then(toml::Value::as_bool)
            .unwrap_or(true);
        let subsampling = match tbl.get("jpeg_subsampling").and_then(toml::Value::as_str) {
            Some("444") => ChromaSubsampling::Cs444,
            Some("422") => ChromaSubsampling::Cs422,
            Some("420") => ChromaSubsampling::Cs420,
            _ => ChromaSubsampling::Auto,
        };
        overrides.quality = Some(QualitySetting::PerFormat(FormatQuality::Jpeg {
            quality: q_u8,
            progressive,
            optimize_coding,
            subsampling,
        }));
    }
    if let Some(wm) = tbl.get("webp_mode").and_then(toml::Value::as_str) {
        let mode = if wm.eq_ignore_ascii_case("lossless") {
            WebPMode::Lossless
        } else {
            WebPMode::Lossy
        };
        let quality = tbl
            .get("webp_quality")
            .and_then(toml::Value::as_integer)
            .and_then(|v| u8::try_from(v).ok())
            .unwrap_or(80)
            .min(100);
        let alpha_quality = tbl
            .get("webp_alpha_quality")
            .and_then(toml::Value::as_integer)
            .and_then(|v| u8::try_from(v).ok())
            .unwrap_or(100)
            .min(100);
        let method = tbl
            .get("webp_method")
            .and_then(toml::Value::as_integer)
            .and_then(|v| u8::try_from(v).ok())
            .unwrap_or(4)
            .min(6);
        overrides.quality = Some(QualitySetting::PerFormat(FormatQuality::WebP {
            mode,
            quality,
            alpha_quality,
            method,
        }));
    }
    if let Some(pc) = tbl.get("png_compression").and_then(toml::Value::as_str) {
        let compression = match pc {
            "fast" => PngCompression::Fast,
            "best" => PngCompression::Best,
            _ => PngCompression::Default,
        };
        overrides.quality = Some(QualitySetting::PerFormat(FormatQuality::Png {
            compression,
        }));
    }

    if let Some(rm) = tbl.get("resize_mode").and_then(toml::Value::as_str) {
        let allow_upscale = tbl
            .get("allow_upscale")
            .and_then(toml::Value::as_bool)
            .unwrap_or(false);
        let w = tbl
            .get("resize_width")
            .and_then(toml::Value::as_integer)
            .and_then(|v| u32::try_from(v).ok())
            .and_then(NonZeroU32::new);
        let h = tbl
            .get("resize_height")
            .and_then(toml::Value::as_integer)
            .and_then(|v| u32::try_from(v).ok())
            .and_then(NonZeroU32::new);
        overrides.resize = match rm {
            "original" => Some(ResizeSpec::Original),
            "width" => w.map(|px| ResizeSpec::Width { px, allow_upscale }),
            "height" => h.map(|px| ResizeSpec::Height { px, allow_upscale }),
            "fit" => w.zip(h).map(|(width, height)| ResizeSpec::Fit {
                width,
                height,
                allow_upscale,
            }),
            _ => None,
        };
    }
    if let Some(bg) = tbl
        .get("alpha_background")
        .and_then(toml::Value::as_str)
        .and_then(|s| s.parse::<Rgb8>().ok())
    {
        overrides.alpha_background = Some(bg);
    }
    if let Some(or_str) = tbl.get("orientation").and_then(toml::Value::as_str) {
        overrides.orientation = match or_str {
            "auto_orient" => Some(OrientationPolicy::AutoOrient),
            "preserve_raw" => Some(OrientationPolicy::PreserveRaw),
            _ => None,
        };
    }
    if let Some(icc_str) = tbl.get("icc").and_then(toml::Value::as_str) {
        overrides.icc = match icc_str {
            "preserve" => Some(IccPolicy::Preserve),
            "strip" => Some(IccPolicy::Strip),
            _ => None,
        };
    }
    if let Some(exif_str) = tbl.get("exif").and_then(toml::Value::as_str) {
        overrides.exif = match exif_str {
            "strip" => Some(ExifPolicy::Strip),
            "keep_safe" => Some(ExifPolicy::KeepSafe),
            "preserve_all" => Some(ExifPolicy::PreserveAll),
            _ => None,
        };
    }
    if let Some(sf_str) = tbl.get("same_format").and_then(toml::Value::as_str) {
        overrides.same_format = match sf_str {
            "skip" => Some(SameFormatPolicy::Skip),
            "reencode" => Some(SameFormatPolicy::Reencode),
            _ => None,
        };
    }

    Some(Preset {
        id: id.to_string(),
        name,
        description,
        builtin: false,
        target,
        overrides,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn default_presets_and_three_layer_resolution() {
        let settings = ConvertSettings::default();
        assert_eq!(builtin_presets().len(), 4);

        // Layer 1 + 2: resolve builtin.smaller-jpeg
        let opts_jpeg = settings
            .resolve(
                &PresetRef::PresetId("builtin.smaller-jpeg".to_string()),
                None,
            )
            .unwrap();
        assert_eq!(opts_jpeg.target, FormatId::Jpeg);
        assert_eq!(opts_jpeg.alpha.background, Rgb8::WHITE);

        // Layer 3 override: override collision policy and exif policy
        let explicit = PresetOverrides {
            exif: Some(ExifPolicy::PreserveAll),
            collision: Some(CollisionPolicy::Fail),
            ..PresetOverrides::default()
        };
        let opts_overridden = settings
            .resolve(
                &PresetRef::PresetId("builtin.smaller-jpeg".to_string()),
                Some(&explicit),
            )
            .unwrap();
        assert_eq!(opts_overridden.metadata.exif, ExifPolicy::PreserveAll);
        assert_eq!(opts_overridden.output.collision, CollisionPolicy::Fail);
    }

    #[test]
    fn toml_lenient_clamping_and_comment_preservation() {
        let raw_toml = r##"# User top-level comment
[unrelated_section]
keep_me = 42

[convert]
# Preserve this comment too
schema_version = 1
quick_convert_target = "jpeg"
default_preset = "builtin.smaller-jpeg"

[convert.defaults]
quality_intent = "high"
same_format = "reencode"
alpha_background = "#FF00AA"
icc = "strip"
exif = "preserve_all"
collision = "fail"
"##;

        let (mut settings, warnings) = ConvertSettings::from_toml_str(raw_toml).unwrap();
        assert!(warnings.is_empty(), "unexpected warnings: {warnings:?}");
        assert_eq!(settings.quick_convert_target, FormatId::Jpeg);
        assert_eq!(settings.defaults.same_format, SameFormatPolicy::Reencode);
        assert_eq!(
            settings.defaults.alpha.background,
            Rgb8 {
                r: 0xFF,
                g: 0x00,
                b: 0xAA
            }
        );

        settings.quick_convert_target = FormatId::WebP;
        let saved = settings.save_preserving_comments(raw_toml).unwrap();
        assert!(saved.contains("# User top-level comment"));
        assert!(saved.contains("[unrelated_section]"));
        assert!(saved.contains("keep_me = 42"));
        assert!(saved.contains("# Preserve this comment too"));

        let (reloaded, warnings2) = ConvertSettings::from_toml_str(&saved).unwrap();
        assert!(warnings2.is_empty());
        assert_eq!(reloaded, settings);
    }

    #[test]
    fn schema_newer_than_supported_emits_warning_not_error() {
        let newer = r#"
[convert]
schema_version = 99
quick_convert_target = "png"
unknown_future_key = "ignored"
"#;
        let (settings, warnings) = ConvertSettings::from_toml_str(newer).unwrap();
        assert_eq!(settings.quick_convert_target, FormatId::Png);
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].schema_newer_than_supported);
    }

    proptest! {
        #[test]
        fn proptest_convert_settings_toml_roundtrip(
            qct_idx in 0usize..3,
            intent_idx in 0usize..5,
            reencode in any::<bool>(),
            r in any::<u8>(),
            g in any::<u8>(),
            b in any::<u8>(),
            preserve_icc in any::<bool>(),
            exif_idx in 0usize..3,
            coll_idx in 0usize..3,
            preserve_ts in any::<bool>(),
            allow_ext in any::<bool>(),
        ) {
            let mut settings = ConvertSettings::default();
            let qct = FormatId::ALL[qct_idx];
            settings.quick_convert_target = qct;
            settings.defaults.quality = QualitySetting::Intent(match intent_idx {
                0 => QualityIntent::Lossless,
                1 => QualityIntent::Maximum,
                2 => QualityIntent::High,
                3 => QualityIntent::Balanced,
                _ => QualityIntent::Compact,
            });
            settings.defaults.same_format = if reencode {
                SameFormatPolicy::Reencode
            } else {
                SameFormatPolicy::Skip
            };
            settings.defaults.alpha.background = Rgb8 { r, g, b };
            settings.defaults.metadata.icc = if preserve_icc {
                IccPolicy::Preserve
            } else {
                IccPolicy::Strip
            };
            settings.defaults.metadata.exif = match exif_idx {
                0 => ExifPolicy::Strip,
                1 => ExifPolicy::KeepSafe,
                _ => ExifPolicy::PreserveAll,
            };
            settings.defaults.output.collision = match coll_idx {
                0 => CollisionPolicy::Suffix,
                1 => CollisionPolicy::Skip,
                _ => CollisionPolicy::Fail,
            };
            settings.defaults.output.preserve_timestamps = preserve_ts;
            settings.backends.allow_external_backends = allow_ext;

            let serialized = settings.save_preserving_comments("# Header comment\n").unwrap();
            let (parsed, warnings) = ConvertSettings::from_toml_str(&serialized).unwrap();
            prop_assert!(warnings.is_empty());
            prop_assert_eq!(parsed, settings);
        }
    }
}
