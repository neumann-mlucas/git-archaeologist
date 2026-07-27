use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use duckdb::Connection;

pub mod queries;
pub mod schema;

pub struct Cache {
    pub conn: Connection,
}

/// Open (or create) the cache file. On unrecoverable open error we wipe and
/// re-try once — DuckDB is generally crash-resilient (it maintains its own
/// WAL), but a partially-written or ABI-mismatched file from an older tool
/// version can still occur, and the honest recovery is: rebuild from git.
pub fn open(path: impl AsRef<Path>) -> Result<Cache> {
    let path = path.as_ref();
    match try_open(path) {
        Ok(c) => Ok(c),
        Err(e) => {
            eprintln!("cache at {} could not be opened: {e}", path.display());
            eprintln!("wiping and re-creating…");
            wipe(path)?;
            try_open(path).context("re-opening cache after wipe")
        }
    }
}

fn try_open(path: &Path) -> Result<Cache> {
    let conn = Connection::open(path)
        .with_context(|| format!("opening cache at {}", path.display()))?;
    schema::migrate(&conn)?;
    Ok(Cache { conn })
}

fn wipe(path: &Path) -> Result<()> {
    // DuckDB may also leave a `-wal` file next to the main one.
    for suffix in ["", ".wal", ".tmp"] {
        let p: PathBuf = if suffix.is_empty() {
            path.to_path_buf()
        } else {
            let mut s = path.as_os_str().to_owned();
            s.push(suffix);
            PathBuf::from(s)
        };
        if p.exists() {
            std::fs::remove_file(&p)
                .with_context(|| format!("removing {}", p.display()))?;
        }
    }
    Ok(())
}
