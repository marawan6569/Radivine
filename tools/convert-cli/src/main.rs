//! Developer CLI and benchmark harness for `radivine-convert` and `radivine-convert-image`.
//!
//! Supports:
//! - `convert-cli [--to webp|jpg|png] [--preset <id>] [--quality <tier|1..100>] [--resize <WxH>] [--out-dir <dir>] FILES...`
//! - `convert-cli inspect FILES...`
//! - `convert-cli bench [--runs 10] [--warmup 3] [--json <out.json>]`

#![forbid(unsafe_code)]

use std::fs;
use std::num::NonZeroU32;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use convert_corpus::generate_photo_set;
use radivine_convert::{
    CapabilityCatalog, ChromaSubsampling, CollisionPolicy, ConversionEngine, ConversionOptions,
    ConvertSettings, FormatId, FormatQuality, NoopControl, OutputLocation, OutputPolicy,
    PngCompression, PresetRef, QualityIntent, QualitySetting, ResizeSpec, ResourceLimitsConfig,
    SourceSpec, WebPMode,
};
use radivine_convert_image::{ImageBackend, descriptor};
use serde::{Deserialize, Serialize};

#[derive(Debug, Parser)]
#[command(
    name = "convert-cli",
    about = "Radivine developer conversion CLI and benchmark runner"
)]
struct Cli {
    #[command(subcommand)]
    subcommand: Option<SubCmd>,

    /// Target format (`webp`, `jpg`, `png`).
    #[arg(long, default_value = "webp")]
    to: String,

    /// Built-in or user preset ID (`web-optimized`, `balanced-photo`, `smaller-size`, `lossless-archive`).
    #[arg(long)]
    preset: Option<String>,

    /// Quality intent (`compact`, `balanced`, `high`, `maximum`, `lossless`) or numeric `1..=100`.
    #[arg(long)]
    quality: Option<String>,

    /// Resize spec (`WxH` for Fit, `Wx` for Width, `xH` for Height).
    #[arg(long)]
    resize: Option<String>,

    /// Output directory (defaults to same directory as each source file).
    #[arg(long)]
    out_dir: Option<PathBuf>,

    /// Collision policy (`suffix`, `skip`, `fail`).
    #[arg(long, default_value = "suffix")]
    collision: String,

    /// Input files when running without a subcommand.
    #[arg(value_name = "FILES")]
    files: Vec<PathBuf>,
}

#[derive(Debug, Subcommand)]
enum SubCmd {
    /// Header-only inspection of one or more media files.
    Inspect {
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Run deterministic C1 reference benchmarks (warmup + measured runs + child VmHWM RSS).
    Bench {
        /// Number of warm-up iterations per benchmark case.
        #[arg(long, default_value_t = 3)]
        warmup: usize,
        /// Number of measured iterations per benchmark case.
        #[arg(long, default_value_t = 10)]
        runs: usize,
        /// Optional path to write structured JSON benchmark report.
        #[arg(long)]
        json: Option<PathBuf>,
        /// Include 50 MP benchmark case in addition to 2 MP, 12 MP, and 24 MP.
        #[arg(long, default_value_t = true)]
        include_50mp: bool,
    },
    /// Internal child command used by `bench` to measure isolated per-item `VmHWM` peak RSS.
    #[command(hide = true)]
    BenchChild {
        #[arg(long)]
        source: PathBuf,
        #[arg(long)]
        to: String,
        #[arg(long)]
        quality: Option<String>,
        #[arg(long)]
        out_dir: PathBuf,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct ChildSample {
    duration_ms: f64,
    output_bytes: u64,
    vm_hwm_kib: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct BenchCaseReport {
    name: String,
    source_format: String,
    target_format: String,
    dimensions: String,
    source_bytes: u64,
    output_bytes: u64,
    runs: usize,
    warmup: usize,
    median_ms: f64,
    p95_ms: f64,
    min_ms: f64,
    max_ms: f64,
    peak_rss_mib: f64,
}

fn build_engine() -> ConversionEngine {
    let limits = ResourceLimitsConfig::default();
    let backend = Arc::new(ImageBackend::with_limits(limits.clone()));
    let catalog = CapabilityCatalog::builtin_only(descriptor());
    ConversionEngine::new(catalog, vec![backend], limits)
}

fn validate_opts(
    opts: ConversionOptions,
    catalog: &CapabilityCatalog,
) -> Result<(
    radivine_convert::ValidatedOptions,
    Vec<radivine_convert::OptionIssue>,
)> {
    opts.validate(catalog)
        .map_err(|issues| anyhow::anyhow!("invalid options: {issues:?}"))
}

fn parse_target_format(s: &str) -> Result<FormatId> {
    s.parse::<FormatId>()
        .map_err(|e| anyhow::anyhow!("{e} (expected webp, jpg, or png)"))
}

fn parse_collision(s: &str) -> Result<CollisionPolicy> {
    match s.trim().to_ascii_lowercase().as_str() {
        "suffix" => Ok(CollisionPolicy::Suffix),
        "skip" => Ok(CollisionPolicy::Skip),
        "fail" => Ok(CollisionPolicy::Fail),
        other => bail!("unknown collision policy '{other}' (expected suffix, skip, or fail)"),
    }
}

fn parse_quality(s: &str, target: FormatId) -> Result<QualitySetting> {
    match s.trim().to_ascii_lowercase().as_str() {
        "smaller" | "compact" => Ok(QualitySetting::Intent(QualityIntent::Compact)),
        "balanced" => Ok(QualitySetting::Intent(QualityIntent::Balanced)),
        "high" => Ok(QualitySetting::Intent(QualityIntent::High)),
        "maximum" => Ok(QualitySetting::Intent(QualityIntent::Maximum)),
        "lossless" => Ok(QualitySetting::Intent(QualityIntent::Lossless)),
        num_str => {
            let q: u8 = num_str
                .parse()
                .with_context(|| format!("invalid quality '{s}'"))?;
            let q = q.clamp(1, 100);
            match target {
                FormatId::WebP => Ok(QualitySetting::PerFormat(FormatQuality::WebP {
                    mode: WebPMode::Lossy,
                    quality: q,
                    alpha_quality: 90,
                    method: 4,
                })),
                FormatId::Jpeg => Ok(QualitySetting::PerFormat(FormatQuality::Jpeg {
                    quality: q,
                    progressive: false,
                    subsampling: ChromaSubsampling::Cs420,
                    optimize_coding: true,
                })),
                FormatId::Png => Ok(QualitySetting::PerFormat(FormatQuality::Png {
                    compression: PngCompression::Default,
                })),
                _ => Ok(QualitySetting::Intent(QualityIntent::Balanced)),
            }
        }
    }
}

fn parse_resize(s: &str) -> Result<ResizeSpec> {
    let trimmed = s.trim().to_ascii_lowercase();
    if let Some((w_str, h_str)) = trimmed.split_once('x') {
        match (w_str.is_empty(), h_str.is_empty()) {
            (false, false) => {
                let w = NonZeroU32::new(w_str.parse()?).context("width must be > 0")?;
                let h = NonZeroU32::new(h_str.parse()?).context("height must be > 0")?;
                Ok(ResizeSpec::Fit {
                    width: w,
                    height: h,
                    allow_upscale: false,
                })
            }
            (false, true) => {
                let w = NonZeroU32::new(w_str.parse()?).context("width must be > 0")?;
                Ok(ResizeSpec::Width {
                    px: w,
                    allow_upscale: false,
                })
            }
            (true, false) => {
                let h = NonZeroU32::new(h_str.parse()?).context("height must be > 0")?;
                Ok(ResizeSpec::Height {
                    px: h,
                    allow_upscale: false,
                })
            }
            (true, true) => bail!("invalid resize spec '{s}'"),
        }
    } else {
        bail!("invalid resize spec '{s}' (expected WxH, Wx, or xH)")
    }
}

fn read_vm_hwm_kib() -> u64 {
    let Ok(status) = fs::read_to_string("/proc/self/status") else {
        return 0;
    };
    for line in status.lines() {
        if let Some(rest) = line.strip_prefix("VmHWM:") {
            let digits: String = rest.chars().filter(char::is_ascii_digit).collect();
            if let Ok(v) = digits.parse::<u64>() {
                return v;
            }
        }
    }
    0
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let engine = build_engine();

    match cli.subcommand {
        Some(SubCmd::Inspect { files }) => {
            for path in files {
                let probe = engine.inspect(&path)?;
                println!(
                    "{}: format={} {}x{} bit_depth={} alpha={} animated={} orient={} icc={} exif={}",
                    path.display(),
                    probe.format,
                    probe.width,
                    probe.height,
                    probe.bit_depth,
                    probe.has_alpha,
                    probe.is_animated,
                    probe.orientation,
                    probe.has_icc,
                    probe.has_exif
                );
            }
            Ok(())
        }
        Some(SubCmd::BenchChild {
            source,
            to,
            quality,
            out_dir,
        }) => {
            let target = parse_target_format(&to)?;
            let mut opts = ConversionOptions {
                target,
                output: OutputPolicy {
                    location: OutputLocation::Directory(out_dir),
                    collision: CollisionPolicy::Suffix,
                    ..OutputPolicy::default()
                },
                ..ConversionOptions::default()
            };
            if let Some(q_str) = quality {
                opts.quality = parse_quality(&q_str, target)?;
            }
            let (validated, _) = validate_opts(opts, engine.catalog())?;
            let plan = engine.plan(&[SourceSpec::from_path(&source)], &validated);
            if plan.items.is_empty() {
                bail!("plan produced 0 items for {}", source.display());
            }
            let t0 = Instant::now();
            let item = engine.convert_item(&plan.items[0], &validated, &mut NoopControl)?;
            let duration_ms = t0.elapsed().as_secs_f64() * 1000.0;
            if let Some(out_path) = item.output_path {
                let _ = fs::remove_file(out_path);
            }
            let sample = ChildSample {
                duration_ms,
                output_bytes: item.output_bytes,
                vm_hwm_kib: read_vm_hwm_kib(),
            };
            println!("{}", serde_json::to_string(&sample)?);
            Ok(())
        }
        Some(SubCmd::Bench {
            warmup,
            runs,
            json,
            include_50mp,
        }) => run_benchmarks(&engine, warmup, runs, json.as_deref(), include_50mp),
        None => {
            if cli.files.is_empty() {
                bail!("no input files specified (try `convert-cli --help` or `convert-cli bench`)");
            }
            let settings = ConvertSettings::default();
            let mut opts = if let Some(preset_id) = &cli.preset {
                settings
                    .resolve(&PresetRef::PresetId(preset_id.clone()), None)
                    .map_err(|e| anyhow::anyhow!(e))?
            } else {
                ConversionOptions::default()
            };

            let target = parse_target_format(&cli.to)?;
            opts.target = target;
            if let Some(q_str) = &cli.quality {
                opts.quality = parse_quality(q_str, target)?;
            }
            if let Some(r_str) = &cli.resize {
                opts.resize = parse_resize(r_str)?;
            }
            opts.output.collision = parse_collision(&cli.collision)?;
            if let Some(dir) = cli.out_dir {
                fs::create_dir_all(&dir)?;
                opts.output.location = OutputLocation::Directory(dir);
            }

            let (validated, issues) = validate_opts(opts, engine.catalog())?;
            for issue in &issues {
                eprintln!("warning: {}", issue.message);
            }
            let specs: Vec<SourceSpec> = cli.files.iter().map(SourceSpec::from_path).collect();
            let plan = engine.plan(&specs, &validated);
            for (skipped_path, reason) in &plan.skipped {
                eprintln!("skipped {}: {:?}", skipped_path.display(), reason);
            }
            for item in &plan.items {
                let res = engine.convert_item(item, &validated, &mut NoopControl)?;
                if let Some(out_path) = &res.output_path {
                    println!(
                        "{} -> {} ({}x{}, {} bytes, {:.1} ms)",
                        res.source_path.display(),
                        out_path.display(),
                        res.output_width,
                        res.output_height,
                        res.output_bytes,
                        res.duration.as_secs_f64() * 1000.0
                    );
                } else {
                    println!("{} -> skipped existing", res.source_path.display());
                }
            }
            Ok(())
        }
    }
}

fn run_benchmarks(
    engine: &ConversionEngine,
    warmup: usize,
    runs: usize,
    json_out: Option<&Path>,
    include_50mp: bool,
) -> Result<()> {
    let tmp = tempfile::tempdir()?;
    let tiers = if include_50mp {
        vec![2, 12, 24, 50]
    } else {
        vec![2, 12, 24]
    };
    let generated = generate_photo_set(tmp.path(), 1, &tiers)?;
    let out_dir = tmp.path().join("bench_out");
    fs::create_dir_all(&out_dir)?;

    let find_file = |substr: &str| -> PathBuf {
        generated
            .iter()
            .find(|p| p.file_name().unwrap().to_string_lossy().contains(substr))
            .expect("generated photo fixture must exist")
            .clone()
    };

    let mut cases: Vec<(&str, PathBuf, FormatId, Option<&str>)> = vec![
        (
            "2 MP JPEG -> WebP q80 m4",
            find_file("photo_2mp_1920x1080.jpg"),
            FormatId::WebP,
            Some("80"),
        ),
        (
            "12 MP JPEG -> WebP q80 m4",
            find_file("photo_12mp_4000x3000.jpg"),
            FormatId::WebP,
            Some("80"),
        ),
        (
            "12 MP PNG -> JPEG q82",
            find_file("photo_12mp_4000x3000.png"),
            FormatId::Jpeg,
            Some("82"),
        ),
        (
            "24 MP JPEG -> WebP q80 m4",
            find_file("photo_24mp_6000x4000.jpg"),
            FormatId::WebP,
            Some("80"),
        ),
    ];
    if include_50mp {
        cases.push((
            "50 MP JPEG -> WebP q80 m4",
            find_file("photo_50mp_8192x6144.jpg"),
            FormatId::WebP,
            Some("80"),
        ));
    }

    let exe = std::env::current_exe()?;
    let mut reports = Vec::new();

    for (name, src_path, target, q_opt) in cases {
        let probe = engine.inspect(&src_path)?;
        let src_bytes = fs::metadata(&src_path)?.len();

        let mut opts = ConversionOptions {
            target,
            output: OutputPolicy {
                location: OutputLocation::Directory(out_dir.clone()),
                collision: CollisionPolicy::Suffix,
                ..OutputPolicy::default()
            },
            ..ConversionOptions::default()
        };
        if let Some(q_str) = q_opt {
            opts.quality = parse_quality(q_str, target)?;
        }
        let (validated, _) = validate_opts(opts, engine.catalog())?;
        let plan = engine.plan(&[SourceSpec::from_path(&src_path)], &validated);

        for _ in 0..warmup {
            let item = engine.convert_item(&plan.items[0], &validated, &mut NoopControl)?;
            if let Some(p) = item.output_path {
                let _ = fs::remove_file(p);
            }
        }

        let mut durations = Vec::with_capacity(runs);
        let mut last_output_bytes = 0u64;
        for _ in 0..runs {
            let t0 = Instant::now();
            let item = engine.convert_item(&plan.items[0], &validated, &mut NoopControl)?;
            durations.push(t0.elapsed().as_secs_f64() * 1000.0);
            last_output_bytes = item.output_bytes;
            if let Some(p) = item.output_path {
                let _ = fs::remove_file(p);
            }
        }

        durations.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let median_ms = durations[durations.len() / 2];
        let p95_idx = ((durations.len() as f64 * 0.95).ceil() as usize)
            .saturating_sub(1)
            .min(durations.len() - 1);
        let p95_ms = durations[p95_idx];
        let min_ms = durations[0];
        let max_ms = *durations.last().unwrap();

        // Measure isolated child process VmHWM peak RSS
        let mut cmd = Command::new(&exe);
        cmd.arg("bench-child")
            .arg("--source")
            .arg(&src_path)
            .arg("--to")
            .arg(target.as_str())
            .arg("--out-dir")
            .arg(&out_dir);
        if let Some(q_str) = q_opt {
            cmd.arg("--quality").arg(q_str);
        }
        let output = cmd.output()?;
        if !output.status.success() {
            bail!(
                "bench-child failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        let child_sample: ChildSample =
            serde_json::from_slice(&output.stdout).context("parse bench-child JSON")?;

        reports.push(BenchCaseReport {
            name: name.to_string(),
            source_format: probe.format.to_string(),
            target_format: target.to_string(),
            dimensions: format!("{}x{}", probe.width, probe.height),
            source_bytes: src_bytes,
            output_bytes: last_output_bytes,
            runs,
            warmup,
            median_ms,
            p95_ms,
            min_ms,
            max_ms,
            peak_rss_mib: child_sample.vm_hwm_kib as f64 / 1024.0,
        });
    }

    println!(
        "| Benchmark Case | Dimensions | Source Size | Output Size | Median (ms) | p95 (ms) | Peak RSS (VmHWM) |"
    );
    println!("|---|---|---|---|---:|---:|---:|");
    for r in &reports {
        println!(
            "| {} | `{}` | {:.1} KiB | {:.1} KiB | {:.1} | {:.1} | {:.1} MiB |",
            r.name,
            r.dimensions,
            r.source_bytes as f64 / 1024.0,
            r.output_bytes as f64 / 1024.0,
            r.median_ms,
            r.p95_ms,
            r.peak_rss_mib
        );
    }

    if let Some(path) = json_out {
        fs::write(path, serde_json::to_string_pretty(&reports)?)?;
    }

    Ok(())
}
