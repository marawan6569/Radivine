//! Persistent `[jobs]` configuration from `$XDG_CONFIG_HOME/radivine/config.toml` (`JOB_SYSTEM_DESIGN.md` §10).

use std::fs;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// Notification verbosity policy for completed background jobs (`[jobs] notify`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobsNotifyPolicy {
    #[default]
    All,
    ErrorsOnly,
    Off,
}

impl JobsNotifyPolicy {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::All => "all",
            Self::ErrorsOnly => "errors_only",
            Self::Off => "off",
        }
    }
}

/// Configured value for `max_parallel` (`"auto"` or explicit `1..=16`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MaxParallelSetting {
    #[default]
    Auto,
    Fixed(usize),
}

impl MaxParallelSetting {
    /// Resolves the effective worker thread count (`(available_parallelism / 2).clamp(1, 4)` for `Auto`,
    /// or `n.clamp(1, 16)` for `Fixed(n)`).
    #[must_use]
    pub fn resolve(self) -> usize {
        match self {
            Self::Auto => {
                let cores = std::thread::available_parallelism()
                    .map(usize::from)
                    .unwrap_or(2);
                (cores / 2).clamp(1, 4)
            }
            Self::Fixed(n) => n.clamp(1, 16),
        }
    }

    #[must_use]
    pub fn to_toml_string(self) -> String {
        match self {
            Self::Auto => "auto".to_string(),
            Self::Fixed(n) => n.clamp(1, 16).to_string(),
        }
    }
}

/// Configured value for `memory_budget_mb` (`"auto"` or explicit `64..=4096` MiB).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum MemoryBudgetSetting {
    #[default]
    Auto,
    FixedMb(u64),
}

impl MemoryBudgetSetting {
    /// Resolves the effective memory budget in MiB (`(MemTotal_MiB / 8).clamp(128, 512)` for `Auto`,
    /// or `mb.clamp(64, 4096)` for `FixedMb(mb)`).
    #[must_use]
    pub fn resolve_mb(self) -> u64 {
        match self {
            Self::Auto => {
                let total_mib = read_proc_memtotal_mib().unwrap_or(2048);
                (total_mib / 8).clamp(128, 512)
            }
            Self::FixedMb(mb) => mb.clamp(64, 4096),
        }
    }

    /// Resolves the effective memory budget in bytes.
    #[must_use]
    pub fn resolve_bytes(self) -> u64 {
        self.resolve_mb() * 1024 * 1024
    }

    #[must_use]
    pub fn to_toml_string(self) -> String {
        match self {
            Self::Auto => "auto".to_string(),
            Self::FixedMb(mb) => mb.clamp(64, 4096).to_string(),
        }
    }
}

/// Validated `[jobs]` settings plus global `[notifications] enabled` flag from `config.toml`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobsSettings {
    pub max_parallel: MaxParallelSetting,
    pub memory_budget_mb: MemoryBudgetSetting,
    pub idle_timeout_secs: u64,
    pub retention_max_jobs: usize,
    pub retention_days: u32,
    pub notify: JobsNotifyPolicy,
    pub global_notifications_enabled: bool,
}

impl Default for JobsSettings {
    fn default() -> Self {
        Self {
            max_parallel: MaxParallelSetting::Auto,
            memory_budget_mb: MemoryBudgetSetting::Auto,
            idle_timeout_secs: 30,
            retention_max_jobs: 200,
            retention_days: 14,
            notify: JobsNotifyPolicy::All,
            global_notifications_enabled: true,
        }
    }
}

impl JobsSettings {
    /// Returns the default `config.toml` path (`$XDG_CONFIG_HOME/radivine/config.toml`).
    #[must_use]
    pub fn default_config_path() -> PathBuf {
        if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME")
            && !xdg.trim().is_empty()
        {
            return PathBuf::from(xdg).join("radivine").join("config.toml");
        }
        if let Ok(home) = std::env::var("HOME")
            && !home.trim().is_empty()
        {
            return PathBuf::from(home)
                .join(".config")
                .join("radivine")
                .join("config.toml");
        }
        PathBuf::from("/tmp/radivine/config.toml")
    }

    /// Leniently loads `[jobs]` and `[notifications] enabled` from `path`, clamping out-of-range
    /// values and returning warnings without failing on unknown keys or missing files.
    #[must_use]
    pub fn load_lenient(path: &Path) -> (Self, Vec<String>) {
        let Ok(raw) = fs::read_to_string(path) else {
            return (Self::default(), Vec::new());
        };
        Self::parse_toml_lenient(&raw)
    }

    /// Leniently parses `[jobs]` and `[notifications]` from a TOML string.
    #[must_use]
    pub fn parse_toml_lenient(raw: &str) -> (Self, Vec<String>) {
        let mut settings = Self::default();
        let mut warnings = Vec::new();

        let table: toml::Table = match toml::from_str(raw) {
            Ok(t) => t,
            Err(e) => {
                warnings.push(format!("failed to parse config.toml: {e}; using defaults"));
                return (settings, warnings);
            }
        };

        if let Some(notif_table) = table.get("notifications").and_then(toml::Value::as_table)
            && let Some(enabled) = notif_table.get("enabled").and_then(toml::Value::as_bool)
        {
            settings.global_notifications_enabled = enabled;
        }

        let Some(jobs_table) = table.get("jobs").and_then(toml::Value::as_table) else {
            return (settings, warnings);
        };

        if let Some(mp_val) = jobs_table.get("max_parallel") {
            match mp_val {
                toml::Value::String(s) if s.trim().eq_ignore_ascii_case("auto") => {
                    settings.max_parallel = MaxParallelSetting::Auto;
                }
                toml::Value::String(s) => match s.trim().parse::<i64>() {
                    Ok(n) => {
                        let clamped = n.clamp(1, 16) as usize;
                        if i64::try_from(clamped) != Ok(n) {
                            warnings.push(format!(
                                "jobs.max_parallel={n} clamped to {clamped} (valid range 1..=16)"
                            ));
                        }
                        settings.max_parallel = MaxParallelSetting::Fixed(clamped);
                    }
                    Err(_) => warnings.push(format!(
                        "invalid jobs.max_parallel={s:?}; falling back to 'auto'"
                    )),
                },
                toml::Value::Integer(n) => {
                    let clamped = (*n).clamp(1, 16) as usize;
                    if i64::try_from(clamped) != Ok(*n) {
                        warnings.push(format!(
                            "jobs.max_parallel={n} clamped to {clamped} (valid range 1..=16)"
                        ));
                    }
                    settings.max_parallel = MaxParallelSetting::Fixed(clamped);
                }
                other => warnings.push(format!(
                    "invalid type for jobs.max_parallel ({other}); falling back to 'auto'"
                )),
            }
        }

        if let Some(mb_val) = jobs_table.get("memory_budget_mb") {
            match mb_val {
                toml::Value::String(s) if s.trim().eq_ignore_ascii_case("auto") => {
                    settings.memory_budget_mb = MemoryBudgetSetting::Auto;
                }
                toml::Value::String(s) => match s.trim().parse::<i64>() {
                    Ok(n) => {
                        let clamped = n.clamp(64, 4096) as u64;
                        if i64::try_from(clamped) != Ok(n) {
                            warnings.push(format!(
                                "jobs.memory_budget_mb={n} clamped to {clamped} (valid range 64..=4096)"
                            ));
                        }
                        settings.memory_budget_mb = MemoryBudgetSetting::FixedMb(clamped);
                    }
                    Err(_) => warnings.push(format!(
                        "invalid jobs.memory_budget_mb={s:?}; falling back to 'auto'"
                    )),
                },
                toml::Value::Integer(n) => {
                    let clamped = (*n).clamp(64, 4096) as u64;
                    if i64::try_from(clamped) != Ok(*n) {
                        warnings.push(format!(
                            "jobs.memory_budget_mb={n} clamped to {clamped} (valid range 64..=4096)"
                        ));
                    }
                    settings.memory_budget_mb = MemoryBudgetSetting::FixedMb(clamped);
                }
                other => warnings.push(format!(
                    "invalid type for jobs.memory_budget_mb ({other}); falling back to 'auto'"
                )),
            }
        }

        if let Some(idle_val) = jobs_table
            .get("idle_timeout_secs")
            .and_then(toml::Value::as_integer)
        {
            let clamped = idle_val.clamp(5, 3600) as u64;
            if i64::try_from(clamped) != Ok(idle_val) {
                warnings.push(format!(
                    "jobs.idle_timeout_secs={idle_val} clamped to {clamped} (valid range 5..=3600)"
                ));
            }
            settings.idle_timeout_secs = clamped;
        }

        if let Some(ret_jobs) = jobs_table
            .get("retention_max_jobs")
            .and_then(toml::Value::as_integer)
        {
            let clamped = ret_jobs.clamp(10, 1000) as usize;
            if i64::try_from(clamped) != Ok(ret_jobs) {
                warnings.push(format!(
                    "jobs.retention_max_jobs={ret_jobs} clamped to {clamped} (valid range 10..=1000)"
                ));
            }
            settings.retention_max_jobs = clamped;
        }

        if let Some(ret_days) = jobs_table
            .get("retention_days")
            .and_then(toml::Value::as_integer)
        {
            let clamped = ret_days.clamp(1, 90) as u32;
            if i64::from(clamped) != ret_days {
                warnings.push(format!(
                    "jobs.retention_days={ret_days} clamped to {clamped} (valid range 1..=90)"
                ));
            }
            settings.retention_days = clamped;
        }

        if let Some(notify_str) = jobs_table.get("notify").and_then(toml::Value::as_str) {
            match notify_str.trim() {
                "all" => settings.notify = JobsNotifyPolicy::All,
                "errors_only" => settings.notify = JobsNotifyPolicy::ErrorsOnly,
                "off" => settings.notify = JobsNotifyPolicy::Off,
                other => warnings.push(format!(
                    "unknown jobs.notify={other:?}; keeping default 'all'"
                )),
            }
        }

        (settings, warnings)
    }

    /// Updates the `[jobs]` section of a `config.toml` document in place using `toml_edit`,
    /// preserving comments and unrelated sections.
    pub fn apply_to_toml_document(&self, existing_toml: &str) -> Result<String, String> {
        let mut doc = existing_toml
            .parse::<toml_edit::DocumentMut>()
            .map_err(|e| format!("failed to parse config.toml with toml_edit: {e}"))?;

        if !doc.contains_table("jobs") {
            doc["jobs"] = toml_edit::Item::Table(toml_edit::Table::new());
        }
        let jobs = doc["jobs"]
            .as_table_mut()
            .ok_or_else(|| "[jobs] is not a table".to_string())?;
        jobs["max_parallel"] = toml_edit::value(self.max_parallel.to_toml_string());
        jobs["memory_budget_mb"] = toml_edit::value(self.memory_budget_mb.to_toml_string());
        jobs["idle_timeout_secs"] = toml_edit::value(self.idle_timeout_secs as i64);
        jobs["retention_max_jobs"] = toml_edit::value(self.retention_max_jobs as i64);
        jobs["retention_days"] = toml_edit::value(self.retention_days as i64);
        jobs["notify"] = toml_edit::value(self.notify.as_str());

        Ok(doc.to_string())
    }
}

fn read_proc_memtotal_mib() -> Option<u64> {
    let content = fs::read_to_string("/proc/meminfo").ok()?;
    for line in content.lines() {
        if let Some(rest) = line.strip_prefix("MemTotal:") {
            let kb_str = rest.split_whitespace().next()?;
            let kb: u64 = kb_str.parse().ok()?;
            return Some(kb / 1024);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_and_clamp_jobs_settings_and_preserve_comments() {
        let raw = r#"
# User comment at top
[notifications]
enabled = false

[jobs]
max_parallel = 99
memory_budget_mb = "32"
idle_timeout_secs = 1
retention_max_jobs = 5000
retention_days = 365
notify = "errors_only"
"#;
        let (settings, warnings) = JobsSettings::parse_toml_lenient(raw);
        assert!(!settings.global_notifications_enabled);
        assert_eq!(settings.max_parallel, MaxParallelSetting::Fixed(16));
        assert_eq!(settings.memory_budget_mb, MemoryBudgetSetting::FixedMb(64));
        assert_eq!(settings.idle_timeout_secs, 5);
        assert_eq!(settings.retention_max_jobs, 1000);
        assert_eq!(settings.retention_days, 90);
        assert_eq!(settings.notify, JobsNotifyPolicy::ErrorsOnly);
        assert_eq!(warnings.len(), 5);

        let updated = settings.apply_to_toml_document(raw).unwrap();
        assert!(updated.contains("# User comment at top"));
        assert!(updated.contains("[notifications]"));
        let (reloaded, rw) = JobsSettings::parse_toml_lenient(&updated);
        assert!(rw.is_empty());
        assert_eq!(reloaded, settings);
    }
}
