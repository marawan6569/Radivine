//! CLI entry point for `tools/convert-corpus`.

#![forbid(unsafe_code)]

use std::path::PathBuf;

use anyhow::Result;
use clap::{Parser, ValueEnum};
use convert_corpus::{
    generate_basic_set, generate_batch_set, generate_edge_set, generate_fs_set,
    generate_malformed_set, generate_meta_set, generate_photo_set,
};

#[derive(Debug, Clone, Copy, ValueEnum)]
enum SetArg {
    Basic,
    Photo,
    Meta,
    Edge,
    Malformed,
    Batch,
    Fs,
    All,
}

#[derive(Debug, Parser)]
#[command(
    name = "convert-corpus",
    about = "Generate deterministic synthetic test corpus for Radivine conversion"
)]
struct Cli {
    /// Deterministic PRNG seed.
    #[arg(long, default_value_t = 1)]
    seed: u64,

    /// Output directory where corpus subdirectories will be written.
    #[arg(long, default_value = "target/convert-corpus")]
    out_dir: PathBuf,

    /// Corpus set to generate.
    #[arg(long, value_enum, default_value_t = SetArg::All)]
    set: SetArg,

    /// Number of items in the `batch` set.
    #[arg(long, default_value_t = 100)]
    batch_count: u32,
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    std::fs::create_dir_all(&cli.out_dir)?;

    let mut total = 0usize;
    match cli.set {
        SetArg::Basic => {
            total += generate_basic_set(&cli.out_dir, cli.seed)?.len();
        }
        SetArg::Photo => {
            total += generate_photo_set(&cli.out_dir, cli.seed, &[2, 12, 24, 50])?.len();
        }
        SetArg::Meta => {
            total += generate_meta_set(&cli.out_dir, cli.seed)?.len();
        }
        SetArg::Edge => {
            total += generate_edge_set(&cli.out_dir, cli.seed)?.len();
        }
        SetArg::Malformed => {
            total += generate_malformed_set(&cli.out_dir, cli.seed)?.len();
        }
        SetArg::Batch => {
            total += generate_batch_set(&cli.out_dir, cli.seed, cli.batch_count)?.len();
        }
        SetArg::Fs => {
            total += generate_fs_set(&cli.out_dir, cli.seed)?.len();
        }
        SetArg::All => {
            total += generate_basic_set(&cli.out_dir, cli.seed)?.len();
            total += generate_meta_set(&cli.out_dir, cli.seed)?.len();
            total += generate_edge_set(&cli.out_dir, cli.seed)?.len();
            total += generate_malformed_set(&cli.out_dir, cli.seed)?.len();
            total += generate_batch_set(&cli.out_dir, cli.seed, cli.batch_count)?.len();
            total += generate_fs_set(&cli.out_dir, cli.seed)?.len();
        }
    }

    println!(
        "Generated {total} deterministic corpus files (seed={}) in {}",
        cli.seed,
        cli.out_dir.display()
    );
    Ok(())
}
