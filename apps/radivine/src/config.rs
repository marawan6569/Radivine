//! Configuration schema (`config.toml`) and validation (`ARCHITECTURE.md` §7.4).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Deserializer};

use radivine_actions::PreviewSettings;
use radivine_platform::{OverlayWindowType, TriggerDetectionMode, TriggerModifier, X11Config};
use radivine_render::Theme;
use radivine_wheel::WheelConfig;

/// Scale configuration (`"auto"` or explicit multiplier in `0.5..=4.0`).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub enum ScaleSetting {
    /// Use platform-detected scale (`Xft.dpi / 96.0`).
    #[default]
    Auto,
    /// Explicit user scale factor (`0.5..=4.0`).
    Fixed(f32),
}

impl<'de> Deserialize<'de> for ScaleSetting {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RawScale {
            Float(f32),
            Int(i64),
            Str(String),
        }

        match RawScale::deserialize(deserializer)? {
            RawScale::Float(v) => Ok(Self::Fixed(v)),
            RawScale::Int(v) => Ok(Self::Fixed(v as f32)),
            RawScale::Str(s) => {
                if s.trim().eq_ignore_ascii_case("auto") {
                    Ok(Self::Auto)
                } else if let Ok(v) = s.trim().parse::<f32>() {
                    Ok(Self::Fixed(v))
                } else {
                    tracing::warn!("invalid appearance.scale={s:?}; falling back to \"auto\"");
                    Ok(Self::Auto)
                }
            }
        }
    }
}

/// Reduced-motion setting (`"auto"` or explicit `true`/`false`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ReducedMotionSetting {
    /// Auto-detect from desktop environment.
    #[default]
    Auto,
    /// Explicit boolean override.
    Fixed(bool),
}

impl<'de> Deserialize<'de> for ReducedMotionSetting {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        #[serde(untagged)]
        enum RawMotion {
            Bool(bool),
            Str(String),
        }

        match RawMotion::deserialize(deserializer)? {
            RawMotion::Bool(b) => Ok(Self::Fixed(b)),
            RawMotion::Str(s) => match s.trim().to_ascii_lowercase().as_str() {
                "auto" => Ok(Self::Auto),
                "true" => Ok(Self::Fixed(true)),
                "false" => Ok(Self::Fixed(false)),
                _ => {
                    tracing::warn!(
                        "invalid appearance.reduced_motion={s:?}; falling back to \"auto\""
                    );
                    Ok(Self::Auto)
                }
            },
        }
    }
}

/// Top-level `config.toml` structure (`ARCHITECTURE.md` §7.4).
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    /// Schema version (`1`).
    pub version: u32,
    /// Drag trigger settings.
    pub trigger: TriggerSection,
    /// Radial wheel geometry and timing settings.
    pub wheel: WheelSection,
    /// Theme, scale, and motion settings.
    pub appearance: AppearanceSection,
    /// File preview line count settings (`[preview] head_lines`, `tail_lines`).
    pub preview: PreviewSection,
    /// Input region and outside-release behavior.
    pub input: InputSection,
    /// X11 backend settings.
    pub x11: X11Section,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            version: 1,
            trigger: TriggerSection::default(),
            wheel: WheelSection::default(),
            appearance: AppearanceSection::default(),
            preview: PreviewSection::default(),
            input: InputSection::default(),
            x11: X11Section::default(),
        }
    }
}

/// `[trigger]` configuration section.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct TriggerSection {
    /// `"shift"` | `"ctrl"` | `"alt"` | `"super"` | `"disabled"`.
    #[serde(alias = "key")]
    pub drag_modifier: String,
    /// Whether pressing the trigger modifier a second time hides the wheel while keeping the drag alive.
    pub toggle_off_on_repress: bool,
    /// Whether releasing the trigger modifier dismisses the wheel.
    #[serde(alias = "dismiss_on_release")]
    pub hold_to_keep_open: bool,
    /// Minimum drag distance in pixels before arming.
    pub min_drag_distance_px: u32,
}

impl Default for TriggerSection {
    fn default() -> Self {
        Self {
            drag_modifier: "shift".to_string(),
            toggle_off_on_repress: true,
            hold_to_keep_open: false,
            min_drag_distance_px: 6,
        }
    }
}

/// `[wheel]` configuration section.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct WheelSection {
    /// Root wheel inner dead-zone radius in logical px.
    pub root_inner_radius: f32,
    /// Root wheel outer radius in logical px.
    pub root_outer_radius: f32,
    /// Child wheel inner dead-zone radius in logical px.
    pub child_inner_radius: f32,
    /// Child wheel outer radius in logical px.
    pub child_outer_radius: f32,
    /// Radial gap between parent outer radius and child inner radius in logical px.
    pub child_offset_gap: f32,
    /// Hover dwell time in milliseconds before opening a submenu.
    pub hover_dwell_ms: u64,
    /// Grace period in milliseconds before closing a submenu.
    pub submenu_grace_ms: u64,
    /// Grace period in milliseconds after pointer leaves the shaped input region in drag mode (`350 ms` default).
    pub leave_grace_ms: u64,
    /// Angular hysteresis in degrees.
    pub angular_hysteresis_deg: f32,
}

impl Default for WheelSection {
    fn default() -> Self {
        Self {
            root_inner_radius: 40.0,
            root_outer_radius: 128.0,
            child_inner_radius: 34.0,
            child_outer_radius: 108.0,
            child_offset_gap: 20.0,
            hover_dwell_ms: 110,
            submenu_grace_ms: 180,
            leave_grace_ms: 350,
            angular_hysteresis_deg: 3.5,
        }
    }
}

/// `[appearance]` configuration section.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct AppearanceSection {
    /// `"dark"` | `"light"` | `"auto"`.
    pub theme: String,
    /// `"auto"` or explicit float multiplier (`0.5..=4.0`).
    pub scale: ScaleSetting,
    /// `"auto"` or boolean (`true` / `false`).
    pub reduced_motion: ReducedMotionSetting,
    /// Full-monitor backdrop dim alpha (`0.0..=0.5`).
    pub backdrop_dim: f32,
}

impl Default for AppearanceSection {
    fn default() -> Self {
        Self {
            theme: "dark".to_string(),
            scale: ScaleSetting::Auto,
            reduced_motion: ReducedMotionSetting::Auto,
            backdrop_dim: 0.0,
        }
    }
}

/// `[preview]` configuration section (`ARCHITECTURE.md` §7.4).
#[derive(Clone, Debug, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct PreviewSection {
    /// Number of lines displayed by `Head` preview (`default: 50`).
    pub head_lines: usize,
    /// Number of lines displayed by `Tail` preview (`default: 50`).
    pub tail_lines: usize,
}

impl Default for PreviewSection {
    fn default() -> Self {
        Self {
            head_lines: 50,
            tail_lines: 50,
        }
    }
}

/// `[input]` configuration section (`AGENTS.md` §2.4: `passthrough` defaults to `false`).
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct InputSection {
    /// Whether pointer clicks outside the wheel pass through to underlying windows (`false` default).
    pub passthrough: bool,
    /// Whether releasing outside any active wheel cancels the session.
    pub release_outside_cancels: bool,
}

impl Default for InputSection {
    fn default() -> Self {
        Self {
            passthrough: false,
            release_outside_cancels: true,
        }
    }
}

/// `[x11]` configuration section.
#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
pub struct X11Section {
    /// `"raw"` | `"poll"`.
    pub trigger_detection: String,
    /// `"dnd"` | `"popup_menu"` | `"notification"` | `"utility"`.
    pub window_type: String,
    /// Whether to issue a 0-px `WarpPointer` nudge on map/unmap.
    pub pointer_nudge: bool,
}

impl Default for X11Section {
    fn default() -> Self {
        Self {
            trigger_detection: "raw".to_string(),
            window_type: "dnd".to_string(),
            pointer_nudge: true,
        }
    }
}

impl AppConfig {
    /// Returns the default XDG configuration file path (`$XDG_CONFIG_HOME/radivine/config.toml`
    /// or `~/.config/radivine/config.toml`).
    #[must_use]
    pub fn default_path() -> PathBuf {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
            && !xdg.trim().is_empty()
        {
            return PathBuf::from(xdg).join("radivine").join("config.toml");
        }
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(home)
                .join(".config")
                .join("radivine")
                .join("config.toml");
        }
        PathBuf::from("config.toml")
    }

    /// Returns the `actions.toml` path corresponding to `config_path` (sibling `actions.toml` when
    /// `--config` is passed, or `$XDG_CONFIG_HOME/radivine/actions.toml` by default).
    #[must_use]
    pub fn actions_path_for(config_path: Option<&Path>) -> PathBuf {
        if let Some(cp) = config_path
            && let Some(parent) = cp.parent()
        {
            let sibling = parent.join("actions.toml");
            if sibling.exists() {
                return sibling;
            }
            return sibling;
        }
        Self::default_path().with_file_name("actions.toml")
    }

    /// Loads and validates `AppConfig` from `path` (or returns defaults if the file does not exist
    /// or fails to parse, logging a warning per `ARCHITECTURE.md` §7.4).
    #[must_use]
    pub fn load_or_default(path: Option<&Path>) -> Self {
        let target = path.map_or_else(Self::default_path, Path::to_path_buf);
        if !target.exists() {
            return Self::default();
        }
        match fs::read_to_string(&target) {
            Ok(contents) => Self::from_toml_str(&contents),
            Err(err) => {
                tracing::warn!(
                    "failed to read config file {}: {err}; using defaults",
                    target.display()
                );
                Self::default()
            }
        }
    }

    /// Parses and sanitizes a TOML string into [`AppConfig`].
    #[must_use]
    pub fn from_toml_str(toml_str: &str) -> Self {
        match toml::from_str::<Self>(toml_str) {
            Ok(mut cfg) => {
                cfg.sanitize();
                cfg
            }
            Err(err) => {
                tracing::warn!("invalid config.toml ({err}); falling back to defaults");
                Self::default()
            }
        }
    }

    /// Validates numeric ranges and resets out-of-range fields to safe defaults.
    pub fn sanitize(&mut self) {
        let def = WheelSection::default();
        if !(16.0..=200.0).contains(&self.wheel.root_inner_radius) {
            tracing::warn!("wheel.root_inner_radius out of range; resetting to default");
            self.wheel.root_inner_radius = def.root_inner_radius;
        }
        if !(60.0..=400.0).contains(&self.wheel.root_outer_radius)
            || self.wheel.root_outer_radius <= self.wheel.root_inner_radius + 20.0
        {
            tracing::warn!("wheel.root_outer_radius out of range; resetting to default");
            self.wheel.root_outer_radius = def.root_outer_radius;
        }
        if !(16.0..=200.0).contains(&self.wheel.child_inner_radius) {
            tracing::warn!("wheel.child_inner_radius out of range; resetting to default");
            self.wheel.child_inner_radius = def.child_inner_radius;
        }
        if !(50.0..=360.0).contains(&self.wheel.child_outer_radius)
            || self.wheel.child_outer_radius <= self.wheel.child_inner_radius + 20.0
        {
            tracing::warn!("wheel.child_outer_radius out of range; resetting to default");
            self.wheel.child_outer_radius = def.child_outer_radius;
        }
        if !(0.0..=120.0).contains(&self.wheel.child_offset_gap) {
            self.wheel.child_offset_gap = def.child_offset_gap;
        }
        if self.wheel.hover_dwell_ms > 2_000 {
            self.wheel.hover_dwell_ms = def.hover_dwell_ms;
        }
        if self.wheel.submenu_grace_ms > 2_000 {
            self.wheel.submenu_grace_ms = def.submenu_grace_ms;
        }
        if !(0.0..=30.0).contains(&self.wheel.angular_hysteresis_deg) {
            self.wheel.angular_hysteresis_deg = def.angular_hysteresis_deg;
        }
        if let ScaleSetting::Fixed(s) = self.appearance.scale
            && !(0.5..=4.0).contains(&s)
        {
            tracing::warn!("appearance.scale={s} out of [0.5, 4.0]; resetting to auto");
            self.appearance.scale = ScaleSetting::Auto;
        }
        self.appearance.backdrop_dim = self.appearance.backdrop_dim.clamp(0.0, 0.5);
        if !(1..=10_000).contains(&self.preview.head_lines) {
            tracing::warn!("preview.head_lines out of [1, 10000]; resetting to 50");
            self.preview.head_lines = 50;
        }
        if !(1..=10_000).contains(&self.preview.tail_lines) {
            tracing::warn!("preview.tail_lines out of [1, 10000]; resetting to 50");
            self.preview.tail_lines = 50;
        }
    }

    /// Builds [`PreviewSettings`] for built-in preview action registration.
    #[must_use]
    pub fn to_preview_settings(&self) -> PreviewSettings {
        PreviewSettings {
            head_lines: self.preview.head_lines,
            tail_lines: self.preview.tail_lines,
            theme: self.appearance.theme.clone(),
        }
    }

    /// Resolves the effective scale factor given the platform's auto-detected scale (`Xft.dpi / 96.0`).
    #[must_use]
    pub fn effective_scale(&self, platform_scale: f32) -> f32 {
        match self.appearance.scale {
            ScaleSetting::Auto => platform_scale.clamp(0.5, 4.0),
            ScaleSetting::Fixed(s) => s.clamp(0.5, 4.0),
        }
    }

    /// Resolves the effective reduced-motion flag given the platform's auto-detected preference.
    #[must_use]
    pub fn effective_reduced_motion(&self, platform_reduced: Option<bool>) -> bool {
        match self.appearance.reduced_motion {
            ReducedMotionSetting::Auto => platform_reduced.unwrap_or(false),
            ReducedMotionSetting::Fixed(b) => b,
        }
    }

    /// Resolves the [`Theme`] (`dark` or `light`).
    #[must_use]
    pub fn resolve_theme(&self) -> Theme {
        match self.appearance.theme.trim().to_ascii_lowercase().as_str() {
            "light" => Theme::light(),
            _ => Theme::dark(),
        }
    }

    /// Builds a [`WheelConfig`] for the radial wheel engine.
    #[must_use]
    pub fn to_wheel_config(&self, reduced_motion: bool) -> WheelConfig {
        let mut cfg = WheelConfig {
            radius_root: [self.wheel.root_inner_radius, self.wheel.root_outer_radius],
            radius_child: [self.wheel.child_inner_radius, self.wheel.child_outer_radius],
            child_gap: self.wheel.child_offset_gap,
            dwell_ms: self.wheel.hover_dwell_ms.min(u64::from(u16::MAX)) as u16,
            grace_ms: self.wheel.submenu_grace_ms.min(u64::from(u16::MAX)) as u16,
            hysteresis_deg: self.wheel.angular_hysteresis_deg,
            reduced_motion,
            ..WheelConfig::default()
        };
        cfg.validate();
        cfg
    }

    /// Builds the [`X11Config`] for the platform backend.
    #[must_use]
    pub fn to_x11_config(&self) -> X11Config {
        let trigger_detection = match self
            .x11
            .trigger_detection
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "poll" => TriggerDetectionMode::Poll,
            _ => TriggerDetectionMode::Raw,
        };
        let trigger_modifier = match self
            .trigger
            .drag_modifier
            .trim()
            .to_ascii_lowercase()
            .as_str()
        {
            "ctrl" | "control" => TriggerModifier::Ctrl,
            "alt" | "mod1" => TriggerModifier::Alt,
            "super" | "meta" | "mod4" => TriggerModifier::Super,
            "disabled" | "none" | "off" => TriggerModifier::Disabled,
            _ => TriggerModifier::Shift,
        };
        let window_type = match self.x11.window_type.trim().to_ascii_lowercase().as_str() {
            "popup_menu" => OverlayWindowType::PopupMenu,
            "notification" => OverlayWindowType::Notification,
            "utility" => OverlayWindowType::Utility,
            _ => OverlayWindowType::Dnd,
        };
        X11Config {
            trigger_detection,
            trigger_modifier,
            window_type,
            pointer_nudge: self.x11.pointer_nudge,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_config_disables_passthrough_and_parses_scale_variants() {
        let def = AppConfig::default();
        assert!(!def.input.passthrough);

        let cfg_float = AppConfig::from_toml_str(
            r#"
            [appearance]
            scale = 2.0
            theme = "light"
            "#,
        );
        assert_eq!(cfg_float.appearance.scale, ScaleSetting::Fixed(2.0));
        assert_eq!(cfg_float.effective_scale(1.0), 2.0);

        let cfg_auto = AppConfig::from_toml_str(
            r#"
            [appearance]
            scale = "auto"
            "#,
        );
        assert_eq!(cfg_auto.appearance.scale, ScaleSetting::Auto);
        assert_eq!(cfg_auto.effective_scale(1.5), 1.5);
    }

    #[test]
    fn out_of_range_values_fall_back_safely() {
        let cfg = AppConfig::from_toml_str(
            r#"
            [wheel]
            root_inner_radius = 999.0
            [appearance]
            scale = 99.0
            "#,
        );
        assert_eq!(cfg.wheel.root_inner_radius, 40.0);
        assert_eq!(cfg.appearance.scale, ScaleSetting::Auto);
    }
}
