//! Backend capability descriptors, probing, and deterministic conversion routing.

use std::collections::BTreeMap;
use std::fmt;
use std::ops::{BitAnd, BitOr, BitOrAssign, Sub};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::format::FormatId;

/// Unique identifier for a conversion backend (e.g. `"builtin.image"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct BackendId(pub &'static str);

impl BackendId {
    #[must_use]
    pub const fn new(id: &'static str) -> Self {
        Self(id)
    }

    #[must_use]
    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for BackendId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl AsRef<str> for BackendId {
    fn as_ref(&self) -> &str {
        self.0
    }
}

/// Bitset of conversion features supported by a backend capability or required by options.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
pub struct FeatureSet(pub u32);

impl FeatureSet {
    pub const LOSSY_QUALITY: Self = Self(1 << 0);
    pub const LOSSLESS: Self = Self(1 << 1);
    pub const ALPHA: Self = Self(1 << 2);
    pub const RESIZE: Self = Self(1 << 3);
    pub const ICC_PRESERVE: Self = Self(1 << 4);
    pub const EXIF_PRESERVE: Self = Self(1 << 5);
    pub const EXIF_ORIENT: Self = Self(1 << 6);
    pub const PROGRESSIVE_JPEG: Self = Self(1 << 7);
    pub const WEBP_ALPHA_Q: Self = Self(1 << 8);
    pub const WEBP_METHOD: Self = Self(1 << 9);
    pub const STREAMING_LARGE: Self = Self(1 << 10);

    #[must_use]
    pub const fn empty() -> Self {
        Self(0)
    }

    #[must_use]
    pub const fn bits(self) -> u32 {
        self.0
    }

    #[must_use]
    pub const fn from_bits_truncate(bits: u32) -> Self {
        Self(bits)
    }

    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    #[must_use]
    pub const fn intersection(self, other: Self) -> Self {
        Self(self.0 & other.0)
    }

    #[must_use]
    pub const fn difference(self, other: Self) -> Self {
        Self(self.0 & !other.0)
    }
}

impl BitOr for FeatureSet {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        self.union(rhs)
    }
}

impl BitOrAssign for FeatureSet {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl BitAnd for FeatureSet {
    type Output = Self;

    fn bitand(self, rhs: Self) -> Self::Output {
        self.intersection(rhs)
    }
}

impl Sub for FeatureSet {
    type Output = Self;

    fn sub(self, rhs: Self) -> Self::Output {
        self.difference(rhs)
    }
}

/// Directed `(from -> to)` conversion capability advertised by a backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Capability {
    pub from: FormatId,
    pub to: FormatId,
    pub features: FeatureSet,
    pub max_dimension: u32,
    pub max_pixels: u64,
    pub priority: u8,
}

/// Execution model of a conversion backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendKind {
    BuiltIn,
    SharedLibrary {
        soname: &'static str,
        min_version: &'static str,
    },
    ExternalProgram {
        program: &'static str,
        probe_args: &'static [&'static str],
        min_version: &'static str,
    },
}

/// Runtime availability probe specification for a backend.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProbeSpec {
    AlwaysAvailable,
    Command {
        program: &'static str,
        args: &'static [&'static str],
        version_prefix: &'static str,
        min_version: (u32, u32, u32),
    },
}

/// Static metadata and capability matrix for a conversion backend.
#[derive(Debug, Clone)]
pub struct BackendDescriptor {
    pub id: BackendId,
    pub display_name: &'static str,
    pub kind: BackendKind,
    pub version: &'static str,
    pub capabilities: &'static [Capability],
    pub probe: ProbeSpec,
    pub install_hint_debian: Option<&'static str>,
}

/// Runtime probe result for a backend.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "snake_case")]
pub enum Availability {
    Available {
        detected_version: String,
    },
    Unavailable {
        reason: String,
        install_hint: Option<String>,
    },
    DisabledByPolicy,
}

impl Availability {
    #[must_use]
    pub const fn is_available(&self) -> bool {
        matches!(self, Availability::Available { .. })
    }
}

/// Availability of a target format in the catalog.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TargetStatus {
    Available {
        backend: BackendId,
        features: FeatureSet,
    },
    BackendMissing {
        backend: BackendId,
        install_hint: Option<String>,
        reason: String,
    },
    Unsupported,
}

/// Structured reason why no backend route satisfied a request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NoRoute {
    UnsupportedPair {
        from: FormatId,
        to: FormatId,
    },
    BackendUnavailable {
        backend: BackendId,
        reason: String,
        install_hint: Option<String>,
    },
    MissingFeatures {
        backend: BackendId,
        missing: FeatureSet,
    },
    DimensionsExceedBackend {
        backend: BackendId,
        max_dimension: u32,
        max_pixels: u64,
    },
}

/// User preferences for backend selection and external backend policy.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BackendPrefs {
    pub allow_external_backends: bool,
    pub preferred_for: BTreeMap<FormatId, String>,
}

/// Probed catalog of conversion backends and deterministic routing table.
#[derive(Debug, Clone)]
pub struct CapabilityCatalog {
    entries: Vec<(BackendDescriptor, Availability)>,
    prefs: BackendPrefs,
}

impl CapabilityCatalog {
    /// Builds a catalog from probed backend descriptors and user preferences.
    #[must_use]
    pub fn build(
        descriptors: Vec<(BackendDescriptor, Availability)>,
        prefs: &BackendPrefs,
    ) -> Self {
        Self {
            entries: descriptors,
            prefs: prefs.clone(),
        }
    }

    /// Convenience constructor for a catalog containing a single always-available built-in backend.
    #[must_use]
    pub fn builtin_only(builtin_desc: BackendDescriptor) -> Self {
        let ver = builtin_desc.version.to_string();
        Self::build(
            vec![(
                builtin_desc,
                Availability::Available {
                    detected_version: ver,
                },
            )],
            &BackendPrefs::default(),
        )
    }

    /// Returns all backend descriptors and their probed availability statuses.
    #[must_use]
    pub fn backend_statuses(&self) -> &[(BackendDescriptor, Availability)] {
        &self.entries
    }

    /// Returns all input formats supported by at least one available backend.
    #[must_use]
    pub fn inputs(&self) -> Vec<FormatId> {
        let mut out = Vec::new();
        for &fmt in FormatId::ALL {
            let supported = self.entries.iter().any(|(desc, avail)| {
                avail.is_available() && desc.capabilities.iter().any(|cap| cap.from == fmt)
            });
            if supported {
                out.push(fmt);
            }
        }
        out
    }

    /// Returns the status of converting `(from -> to)` with no extra feature requirements.
    #[must_use]
    pub fn target_status(&self, from: FormatId, to: FormatId) -> TargetStatus {
        match self.route(from, to, FeatureSet::empty()) {
            Ok((backend, cap)) => TargetStatus::Available {
                backend,
                features: cap.features,
            },
            Err(NoRoute::BackendUnavailable {
                backend,
                reason,
                install_hint,
            }) => TargetStatus::BackendMissing {
                backend,
                install_hint,
                reason,
            },
            Err(_) => TargetStatus::Unsupported,
        }
    }

    /// Returns the aggregate status for every registered target format.
    #[must_use]
    pub fn targets(&self) -> Vec<(FormatId, TargetStatus)> {
        let mut result = Vec::with_capacity(FormatId::ALL.len());
        for &to in FormatId::ALL {
            let mut best_available: Option<(BackendId, FeatureSet, u8)> = None;
            let mut first_missing: Option<(BackendId, Option<String>, String)> = None;

            for (desc, avail) in &self.entries {
                for cap in desc.capabilities.iter().filter(|c| c.to == to) {
                    match avail {
                        Availability::Available { .. } => {
                            let replace = match best_available {
                                None => true,
                                Some((prev_id, _, prev_prio)) => {
                                    cap.priority > prev_prio
                                        || (cap.priority == prev_prio && desc.id < prev_id)
                                }
                            };
                            if replace {
                                best_available = Some((desc.id, cap.features, cap.priority));
                            }
                        }
                        Availability::Unavailable {
                            reason,
                            install_hint,
                        } => {
                            if first_missing.is_none() {
                                first_missing =
                                    Some((desc.id, install_hint.clone(), reason.clone()));
                            }
                        }
                        Availability::DisabledByPolicy => {}
                    }
                }
            }

            let status = if let Some((backend, features, _)) = best_available {
                TargetStatus::Available { backend, features }
            } else if let Some((backend, install_hint, reason)) = first_missing {
                TargetStatus::BackendMissing {
                    backend,
                    install_hint,
                    reason,
                }
            } else {
                TargetStatus::Unsupported
            };
            result.push((to, status));
        }
        result
    }

    /// Deterministic backend selection for `(from, to, required)`.
    ///
    /// Selection order:
    /// 1. User preference (`prefs.preferred_for[to]`) if available and satisfies `required`.
    /// 2. Available backend with highest `Capability::priority`.
    /// 3. Lexical [`BackendId`] tie-breaker.
    pub fn route(
        &self,
        from: FormatId,
        to: FormatId,
        required: FeatureSet,
    ) -> Result<(BackendId, Capability), NoRoute> {
        self.route_with_dimensions(from, to, required, 1, 1)
    }

    /// Deterministic backend selection that also checks `width`, `height`, and `width * height`
    /// against each backend capability's `max_dimension` and `max_pixels`.
    pub fn route_with_dimensions(
        &self,
        from: FormatId,
        to: FormatId,
        required: FeatureSet,
        width: u32,
        height: u32,
    ) -> Result<(BackendId, Capability), NoRoute> {
        let pixels = u64::from(width) * u64::from(height);
        let preferred_id = self.prefs.preferred_for.get(&to).map(String::as_str);

        let mut candidates: Vec<(BackendId, Capability, bool)> = Vec::new();
        let mut missing_feature_err: Option<NoRoute> = None;
        let mut dim_exceeded_err: Option<NoRoute> = None;
        let mut unavailable_err: Option<NoRoute> = None;
        let mut saw_pair = false;

        for (desc, avail) in &self.entries {
            if matches!(desc.kind, BackendKind::ExternalProgram { .. })
                && !self.prefs.allow_external_backends
            {
                continue;
            }
            for &cap in desc
                .capabilities
                .iter()
                .filter(|c| c.from == from && c.to == to)
            {
                saw_pair = true;
                match avail {
                    Availability::Available { .. } => {
                        if !cap.features.contains(required) {
                            if missing_feature_err.is_none() {
                                missing_feature_err = Some(NoRoute::MissingFeatures {
                                    backend: desc.id,
                                    missing: required - cap.features,
                                });
                            }
                            continue;
                        }
                        if width > cap.max_dimension
                            || height > cap.max_dimension
                            || pixels > cap.max_pixels
                        {
                            if dim_exceeded_err.is_none() {
                                dim_exceeded_err = Some(NoRoute::DimensionsExceedBackend {
                                    backend: desc.id,
                                    max_dimension: cap.max_dimension,
                                    max_pixels: cap.max_pixels,
                                });
                            }
                            continue;
                        }
                        let is_preferred = preferred_id == Some(desc.id.0);
                        candidates.push((desc.id, cap, is_preferred));
                    }
                    Availability::Unavailable {
                        reason,
                        install_hint,
                    } => {
                        if unavailable_err.is_none() {
                            unavailable_err = Some(NoRoute::BackendUnavailable {
                                backend: desc.id,
                                reason: reason.clone(),
                                install_hint: install_hint.clone(),
                            });
                        }
                    }
                    Availability::DisabledByPolicy => {}
                }
            }
        }

        if !candidates.is_empty() {
            candidates.sort_by(|(id_a, cap_a, pref_a), (id_b, cap_b, pref_b)| {
                pref_b
                    .cmp(pref_a)
                    .then_with(|| cap_b.priority.cmp(&cap_a.priority))
                    .then_with(|| id_a.cmp(id_b))
            });
            let (id, cap, _) = candidates[0];
            return Ok((id, cap));
        }

        if let Some(err) = dim_exceeded_err {
            return Err(err);
        }
        if let Some(err) = missing_feature_err {
            return Err(err);
        }
        if let Some(err) = unavailable_err {
            return Err(err);
        }
        let _ = saw_pair;
        Err(NoRoute::UnsupportedPair { from, to })
    }
}

/// Environment settings for probing external backends.
#[derive(Debug, Clone)]
pub struct ProbeEnv {
    /// Fixed directories searched for external backend binaries (defaults to `/usr/bin`, `/usr/local/bin`).
    pub search_dirs: Vec<PathBuf>,
    /// Maximum wall-clock time allowed for a single probe subprocess.
    pub timeout: Duration,
}

impl Default for ProbeEnv {
    fn default() -> Self {
        Self {
            search_dirs: vec![PathBuf::from("/usr/bin"), PathBuf::from("/usr/local/bin")],
            timeout: Duration::from_millis(500),
        }
    }
}

/// Probes all registered backend descriptors and returns a [`CapabilityCatalog`].
#[must_use]
pub fn probe_all(
    descriptors: &[BackendDescriptor],
    prefs: &BackendPrefs,
    env: &ProbeEnv,
) -> CapabilityCatalog {
    let mut results = Vec::with_capacity(descriptors.len());
    for desc in descriptors {
        let avail = probe_single(desc, prefs, env);
        results.push((desc.clone(), avail));
    }
    CapabilityCatalog::build(results, prefs)
}

fn probe_single(desc: &BackendDescriptor, prefs: &BackendPrefs, env: &ProbeEnv) -> Availability {
    if matches!(desc.kind, BackendKind::ExternalProgram { .. }) && !prefs.allow_external_backends {
        return Availability::DisabledByPolicy;
    }

    match &desc.probe {
        ProbeSpec::AlwaysAvailable => Availability::Available {
            detected_version: desc.version.to_string(),
        },
        ProbeSpec::Command {
            program,
            args,
            version_prefix,
            min_version,
        } => {
            if !prefs.allow_external_backends {
                return Availability::DisabledByPolicy;
            }
            let hint = desc.install_hint_debian.map(ToOwned::to_owned);
            let Some(exe_path) = resolve_in_dirs(program, &env.search_dirs) else {
                return Availability::Unavailable {
                    reason: format!("program {program:?} not found in {:?}", env.search_dirs),
                    install_hint: hint,
                };
            };

            match run_probe_command(&exe_path, args, env.timeout) {
                Ok(output) => {
                    if let Some(found_ver) = extract_semver(&output, version_prefix) {
                        if found_ver >= *min_version {
                            Availability::Available {
                                detected_version: format!(
                                    "{}.{}.{}",
                                    found_ver.0, found_ver.1, found_ver.2
                                ),
                            }
                        } else {
                            Availability::Unavailable {
                                reason: format!(
                                    "detected version {}.{}.{} is older than required {}.{}.{}",
                                    found_ver.0,
                                    found_ver.1,
                                    found_ver.2,
                                    min_version.0,
                                    min_version.1,
                                    min_version.2
                                ),
                                install_hint: hint,
                            }
                        }
                    } else {
                        Availability::Unavailable {
                            reason: format!(
                                "could not parse version with prefix {version_prefix:?} from output"
                            ),
                            install_hint: hint,
                        }
                    }
                }
                Err(reason) => Availability::Unavailable {
                    reason,
                    install_hint: hint,
                },
            }
        }
    }
}

fn resolve_in_dirs(program: &str, dirs: &[PathBuf]) -> Option<PathBuf> {
    let prog_path = Path::new(program);
    if prog_path.is_absolute() {
        return prog_path.is_file().then(|| prog_path.to_path_buf());
    }
    for dir in dirs {
        let candidate = dir.join(program);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

fn run_probe_command(exe: &Path, args: &[&str], timeout: Duration) -> Result<String, String> {
    let mut child = Command::new(exe)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("failed to spawn {}: {e}", exe.display()))?;

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let output = child
                    .wait_with_output()
                    .map_err(|e| format!("failed to read probe output: {e}"))?;
                if !status.success() {
                    return Err(format!("probe exited with status {status}"));
                }
                let mut combined = String::from_utf8_lossy(&output.stdout).into_owned();
                if !output.stderr.is_empty() {
                    combined.push('\n');
                    combined.push_str(&String::from_utf8_lossy(&output.stderr));
                }
                return Ok(combined);
            }
            Ok(None) => {
                if Instant::now() >= deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(format!(
                        "probe command {} timed out after {}ms",
                        exe.display(),
                        timeout.as_millis()
                    ));
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(format!("error waiting for probe command: {e}"));
            }
        }
    }
}

fn extract_semver(text: &str, prefix: &str) -> Option<(u32, u32, u32)> {
    let idx = if prefix.is_empty() {
        0
    } else {
        text.find(prefix)? + prefix.len()
    };
    let tail = &text[idx..];
    let start = tail.find(|c: char| c.is_ascii_digit())?;
    let ver_str: String = tail[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    let mut parts = ver_str.split('.');
    let major: u32 = parts.next()?.parse().ok()?;
    let minor: u32 = parts.next().unwrap_or("0").parse().ok()?;
    let patch: u32 = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    static CAPS_A: &[Capability] = &[Capability {
        from: FormatId::Png,
        to: FormatId::WebP,
        features: FeatureSet(FeatureSet::LOSSY_QUALITY.0 | FeatureSet::LOSSLESS.0),
        max_dimension: 16_383,
        max_pixels: 100_000_000,
        priority: 100,
    }];

    static CAPS_B: &[Capability] = &[Capability {
        from: FormatId::Png,
        to: FormatId::WebP,
        features: FeatureSet(
            FeatureSet::LOSSY_QUALITY.0 | FeatureSet::LOSSLESS.0 | FeatureSet::STREAMING_LARGE.0,
        ),
        max_dimension: 65_535,
        max_pixels: 500_000_000,
        priority: 100,
    }];

    static CAPS_HIGH: &[Capability] = &[Capability {
        from: FormatId::Png,
        to: FormatId::WebP,
        features: FeatureSet(FeatureSet::LOSSY_QUALITY.0),
        max_dimension: 16_383,
        max_pixels: 100_000_000,
        priority: 200,
    }];

    fn make_desc(
        id: &'static str,
        caps: &'static [Capability],
        kind: BackendKind,
        probe: ProbeSpec,
    ) -> BackendDescriptor {
        BackendDescriptor {
            id: BackendId(id),
            display_name: id,
            kind,
            version: "1.0.0",
            capabilities: caps,
            probe,
            install_hint_debian: Some("sudo apt install libvips-tools"),
        }
    }

    #[test]
    fn deterministic_routing_priority_and_lexical_tiebreak() {
        let desc_b = make_desc(
            "b.backend",
            CAPS_B,
            BackendKind::BuiltIn,
            ProbeSpec::AlwaysAvailable,
        );
        let desc_a = make_desc(
            "a.backend",
            CAPS_A,
            BackendKind::BuiltIn,
            ProbeSpec::AlwaysAvailable,
        );

        let catalog = CapabilityCatalog::build(
            vec![
                (
                    desc_b.clone(),
                    Availability::Available {
                        detected_version: "1.0.0".into(),
                    },
                ),
                (
                    desc_a.clone(),
                    Availability::Available {
                        detected_version: "1.0.0".into(),
                    },
                ),
            ],
            &BackendPrefs::default(),
        );

        // Equal priority (100) -> lexical tie-break picks "a.backend"
        let (chosen, _) = catalog
            .route(FormatId::Png, FormatId::WebP, FeatureSet::LOSSY_QUALITY)
            .unwrap();
        assert_eq!(chosen, BackendId("a.backend"));

        // Requiring STREAMING_LARGE routes to "b.backend"
        let (chosen_stream, _) = catalog
            .route(FormatId::Png, FormatId::WebP, FeatureSet::STREAMING_LARGE)
            .unwrap();
        assert_eq!(chosen_stream, BackendId("b.backend"));

        // Higher priority wins over lexical order
        let desc_high = make_desc(
            "z.high",
            CAPS_HIGH,
            BackendKind::BuiltIn,
            ProbeSpec::AlwaysAvailable,
        );
        let catalog2 = CapabilityCatalog::build(
            vec![
                (
                    desc_a.clone(),
                    Availability::Available {
                        detected_version: "1.0.0".into(),
                    },
                ),
                (
                    desc_high,
                    Availability::Available {
                        detected_version: "1.0.0".into(),
                    },
                ),
            ],
            &BackendPrefs::default(),
        );
        let (chosen_high, _) = catalog2
            .route(FormatId::Png, FormatId::WebP, FeatureSet::LOSSY_QUALITY)
            .unwrap();
        assert_eq!(chosen_high, BackendId("z.high"));

        // User preference overrides priority when preferred backend satisfies required features
        let mut prefs = BackendPrefs::default();
        prefs
            .preferred_for
            .insert(FormatId::WebP, "a.backend".to_string());
        let catalog3 = CapabilityCatalog::build(catalog2.backend_statuses().to_vec(), &prefs);
        let (chosen_pref, _) = catalog3
            .route(FormatId::Png, FormatId::WebP, FeatureSet::LOSSY_QUALITY)
            .unwrap();
        assert_eq!(chosen_pref, BackendId("a.backend"));
    }

    #[test]
    fn no_route_variants_and_dimension_routing() {
        let desc_a = make_desc(
            "builtin.image",
            CAPS_A,
            BackendKind::BuiltIn,
            ProbeSpec::AlwaysAvailable,
        );
        let catalog = CapabilityCatalog::builtin_only(desc_a);

        // Missing feature
        let err = catalog
            .route(FormatId::Png, FormatId::WebP, FeatureSet::PROGRESSIVE_JPEG)
            .unwrap_err();
        assert_eq!(
            err,
            NoRoute::MissingFeatures {
                backend: BackendId("builtin.image"),
                missing: FeatureSet::PROGRESSIVE_JPEG,
            }
        );

        // Exceeding dimensions
        let err_dim = catalog
            .route_with_dimensions(
                FormatId::Png,
                FormatId::WebP,
                FeatureSet::LOSSY_QUALITY,
                20_000,
                1000,
            )
            .unwrap_err();
        assert_eq!(
            err_dim,
            NoRoute::DimensionsExceedBackend {
                backend: BackendId("builtin.image"),
                max_dimension: 16_383,
                max_pixels: 100_000_000,
            }
        );

        // Unsupported pair
        let err_pair = catalog
            .route(FormatId::WebP, FormatId::Jpeg, FeatureSet::empty())
            .unwrap_err();
        assert_eq!(
            err_pair,
            NoRoute::UnsupportedPair {
                from: FormatId::WebP,
                to: FormatId::Jpeg
            }
        );
    }

    #[test]
    fn probe_command_and_external_policy_gate() {
        let tmp = tempfile::tempdir().unwrap();
        let script_path = tmp.path().join("fake-vips");
        fs::write(&script_path, "#!/bin/sh\necho 'vips-8.15.1'\n").unwrap();
        let mut perms = fs::metadata(&script_path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&script_path, perms).unwrap();

        let desc = make_desc(
            "external.vips",
            CAPS_B,
            BackendKind::ExternalProgram {
                program: "fake-vips",
                probe_args: &["--version"],
                min_version: "8.15.0",
            },
            ProbeSpec::Command {
                program: "fake-vips",
                args: &["--version"],
                version_prefix: "vips-",
                min_version: (8, 15, 0),
            },
        );

        let env = ProbeEnv {
            search_dirs: vec![tmp.path().to_path_buf()],
            timeout: Duration::from_millis(500),
        };

        // Default prefs: allow_external_backends = false -> DisabledByPolicy
        let cat_disabled = probe_all(std::slice::from_ref(&desc), &BackendPrefs::default(), &env);
        assert_eq!(
            cat_disabled.backend_statuses()[0].1,
            Availability::DisabledByPolicy
        );

        // Enabled external backends -> Available { detected_version: "8.15.1" }
        let prefs_enabled = BackendPrefs {
            allow_external_backends: true,
            preferred_for: BTreeMap::new(),
        };
        let cat_enabled = probe_all(&[desc], &prefs_enabled, &env);
        assert_eq!(
            cat_enabled.backend_statuses()[0].1,
            Availability::Available {
                detected_version: "8.15.1".to_string()
            }
        );
    }
}
