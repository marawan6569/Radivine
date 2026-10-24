//! Corpus regression integration tests (`IMPLEMENTATION_PLAN.md` §3 & §4, `AC-C1.1`, `AC-C1.7`, `AC-C1.8`).

#![forbid(unsafe_code)]

use std::fs;
use std::path::Path;
use std::sync::Arc;

use convert_corpus::{
    generate_basic_set, generate_edge_set, generate_fs_set, generate_malformed_set,
    generate_meta_set,
};
use radivine_convert::{
    CapabilityCatalog, CollisionPolicy, ConversionEngine, ConversionError, ConversionOptions,
    FormatId, NoopControl, OutputLocation, OutputPolicy, ResourceLimitsConfig, SameFormatPolicy,
    SourceSpec, UnsupportedFeature,
};
use radivine_convert_image::{ImageBackend, descriptor};

fn test_engine() -> ConversionEngine {
    let limits = ResourceLimitsConfig::default();
    let backend = Arc::new(ImageBackend::with_limits(limits.clone()));
    let catalog = CapabilityCatalog::builtin_only(descriptor());
    ConversionEngine::new(catalog, vec![backend], limits)
}

#[test]
fn corpus_basic_and_meta_all_cross_format_conversions_succeed() {
    let engine = test_engine();
    let tmp = tempfile::tempdir().unwrap();
    let basic_files = generate_basic_set(tmp.path(), 1).unwrap();
    let meta_files = generate_meta_set(tmp.path(), 1).unwrap();

    for src in basic_files.iter().chain(meta_files.iter()) {
        for &target in FormatId::ALL {
            let out_dir = tmp.path().join(format!("out_{}", target.as_str()));
            fs::create_dir_all(&out_dir).unwrap();
            let opts = ConversionOptions {
                target,
                same_format: SameFormatPolicy::Reencode,
                output: OutputPolicy {
                    location: OutputLocation::Directory(out_dir),
                    collision: CollisionPolicy::Suffix,
                    ..OutputPolicy::default()
                },
                ..ConversionOptions::default()
            };
            let (validated, _) = opts.validate(engine.catalog()).unwrap();
            let plan = engine.plan(&[SourceSpec::from_path(src)], &validated);
            assert_eq!(plan.items.len(), 1, "expected 1 item for {}", src.display());
            let res = engine
                .convert_item(&plan.items[0], &validated, &mut NoopControl)
                .unwrap_or_else(|e| panic!("failed {} -> {}: {e}", src.display(), target));
            let out_path = res.output_path.unwrap();
            let decoded = image::open(&out_path).unwrap();
            assert_eq!(decoded.width(), res.output_width);
            assert_eq!(decoded.height(), res.output_height);
        }
    }
}

#[test]
fn corpus_edge_and_fs_sets_behave_as_specified() {
    let engine = test_engine();
    let tmp = tempfile::tempdir().unwrap();
    let edge_files = generate_edge_set(tmp.path(), 1).unwrap();

    // 1x1.png and 1x16383_webp_limit.png succeed to WebP; 20000x1_exceeds_webp_limit.png is rejected for WebP (>16383) but succeeds to JPEG!
    for src in &edge_files {
        let name = src.file_name().unwrap().to_string_lossy();
        let opts_webp = ConversionOptions {
            target: FormatId::WebP,
            ..ConversionOptions::default()
        };
        let (val_webp, _) = opts_webp.validate(engine.catalog()).unwrap();
        let plan_w = engine.plan(&[SourceSpec::from_path(src)], &val_webp);
        if name.contains("16384x1") {
            if !plan_w.items.is_empty() {
                assert!(
                    engine
                        .convert_item(&plan_w.items[0], &val_webp, &mut NoopControl)
                        .is_err()
                );
            } else {
                assert_eq!(plan_w.skipped.len(), 1);
            }
            let opts_jpg = ConversionOptions {
                target: FormatId::Jpeg,
                ..ConversionOptions::default()
            };
            let (val_jpg, _) = opts_jpg.validate(engine.catalog()).unwrap();
            let plan_j = engine.plan(&[SourceSpec::from_path(src)], &val_jpg);
            let res_j = engine
                .convert_item(&plan_j.items[0], &val_jpg, &mut NoopControl)
                .unwrap();
            assert_eq!((res_j.output_width, res_j.output_height), (16384, 1));
        } else {
            let res = engine
                .convert_item(&plan_w.items[0], &val_webp, &mut NoopControl)
                .unwrap();
            assert!(res.output_path.unwrap().exists());
        }
    }

    // `fs` set: `collide.png` -> `collide (4).webp` (since 0..=3 exist) and `symlink_out.png` -> `symlink_out (1).webp` without touching `precious_target.txt`
    let fs_files = generate_fs_set(tmp.path(), 1).unwrap();
    let opts = ConversionOptions {
        same_format: SameFormatPolicy::Reencode,
        ..ConversionOptions::default()
    };
    let (validated, _) = opts.validate(engine.catalog()).unwrap();
    for src in &fs_files {
        let plan = engine.plan(&[SourceSpec::from_path(src)], &validated);
        let res = engine
            .convert_item(&plan.items[0], &validated, &mut NoopControl)
            .unwrap();
        let out_name = res
            .output_path
            .unwrap()
            .file_name()
            .unwrap()
            .to_string_lossy()
            .into_owned();
        if src
            .file_name()
            .unwrap()
            .to_string_lossy()
            .starts_with("collide")
        {
            assert_eq!(out_name, "collide (4).webp");
        } else {
            assert_eq!(out_name, "symlink_out (1).webp");
        }
    }
    assert_eq!(
        fs::read(tmp.path().join("fs/precious_target.txt")).unwrap(),
        b"do-not-overwrite-via-symlink"
    );
}

#[test]
fn corpus_malformed_and_committed_seeds_never_panic_and_return_typed_errors() {
    let engine = test_engine();
    let tmp = tempfile::tempdir().unwrap();
    let generated_malformed = generate_malformed_set(tmp.path(), 1).unwrap();

    let committed_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/malformed");
    let mut all_malformed = generated_malformed;
    if committed_dir.exists() {
        for entry in fs::read_dir(&committed_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_file() {
                let dest = tmp
                    .path()
                    .join(path.file_name().unwrap().to_string_lossy().into_owned());
                fs::copy(&path, &dest).unwrap();
                all_malformed.push(dest);
            }
        }
    }

    let opts = ConversionOptions {
        same_format: SameFormatPolicy::Reencode,
        ..ConversionOptions::default()
    };
    let (validated, _) = opts.validate(engine.catalog()).unwrap();

    for path in &all_malformed {
        let res = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            let plan = engine.plan(&[SourceSpec::from_path(path)], &validated);
            if plan.items.is_empty() {
                return Err(ConversionError::CorruptInput {
                    format: FormatId::Png,
                    detail: "skipped by planner".to_string(),
                });
            }
            engine.convert_item(&plan.items[0], &validated, &mut NoopControl)
        }));
        assert!(
            res.is_ok(),
            "panic escaped on malformed input {}",
            path.display()
        );
        let outcome = res.unwrap();
        assert!(
            outcome.is_err(),
            "malformed input {} unexpectedly succeeded",
            path.display()
        );
        let name = path.file_name().unwrap().to_string_lossy();
        if name.contains("animated") {
            assert!(matches!(
                outcome.unwrap_err(),
                ConversionError::UnsupportedFeature {
                    feature: UnsupportedFeature::Animation
                }
            ));
        } else if name.contains("bomb") {
            assert!(matches!(
                outcome.unwrap_err(),
                ConversionError::LimitExceeded { .. }
            ));
        }
    }
}
