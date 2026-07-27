use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;

mod app;
mod cache;
mod config;
mod index;
mod query;
mod repo;
mod ui;

#[derive(Parser, Debug)]
#[command(name = "git-archaeologist", version, about)]
struct Cli {
    /// Path to the git repository (defaults to current directory)
    #[arg(default_value = ".")]
    path: PathBuf,

    /// Force full reindex, ignoring cache
    #[arg(long)]
    reindex: bool,

    /// Override bucket size (auto|commit|day|week|month)
    #[arg(long)]
    bucket: Option<String>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    let repo = repo::open(&cli.path)?;
    let cfg = config::load()?;
    let cache = cache::open(repo.cache_path())?;

    app::run(repo, cfg, cache, cli.reindex, cli.bucket)
}
