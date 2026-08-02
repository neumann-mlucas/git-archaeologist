use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;

mod app;
mod cache;
mod config;
mod index;
mod query;
mod repo;
mod ui;

use crate::index::bucket::BucketSize;

#[derive(Parser, Debug)]
#[command(name = "git-archaeologist", version, about)]
struct Cli {
    /// Path to the git repository (defaults to current directory)
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Force full reindex, ignoring cache
    #[arg(long)]
    reindex: bool,

    /// Override bucket size (commit|day|week|month; omit for auto)
    #[arg(long, value_enum)]
    bucket: Option<BucketSize>,

    /// Export cache tables as Parquet files into DIR and exit (no TUI).
    /// Indexes first if the cache is empty.
    #[arg(long, value_name = "DIR")]
    export_parquet: Option<PathBuf>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    let repo = repo::open(&cli.path)?;
    let cfg = config::load()?;
    let mut cache = cache::open(repo.cache_path())?;

    if let Some(out_dir) = cli.export_parquet {
        return run_export_parquet(&repo, &mut cache, cli.reindex, cli.bucket, &out_dir);
    }

    app::run(repo, cfg, cache, cli.reindex, cli.bucket)
}

fn run_export_parquet(
    repo: &repo::Repo,
    cache: &mut cache::Cache,
    force_reindex: bool,
    bucket_override: Option<BucketSize>,
    out_dir: &std::path::Path,
) -> Result<()> {
    let empty = query::cache_stats(&cache.conn)
        .map(|s| s.commits == 0)
        .unwrap_or(true);
    if empty || force_reindex {
        eprintln!("indexing repo…");
        index::run(
            repo,
            cache,
            index::IndexOptions {
                force_full: force_reindex,
                bucket_override,
            },
            None,
        )?;
    }

    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("creating {}", out_dir.display()))?;

    for table in [
        "commits",
        "churn",
        "file_stats",
        "blame",
        "authors",
        "author_aliases",
    ] {
        let path = out_dir.join(format!("{table}.parquet"));
        let sql = format!(
            "COPY {table} TO '{}' (FORMAT PARQUET)",
            path.display().to_string().replace('\'', "''")
        );
        cache
            .conn
            .execute_batch(&sql)
            .with_context(|| format!("exporting {table} to parquet"))?;
        eprintln!("  wrote {}", path.display());
    }
    Ok(())
}
