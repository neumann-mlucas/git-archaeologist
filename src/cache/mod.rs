use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::Connection;

pub mod queries;
pub mod schema;

pub struct Cache {
    pub conn: Connection,
}

pub fn open(path: impl AsRef<Path>) -> Result<Cache> {
    let path = path.as_ref();
    match try_open(path) {
        Ok(c) => Ok(c),
        Err(e) => {
            // SQLite reports corruption via `SQLITE_CORRUPT` / `SQLITE_NOTADB`;
            // schema mismatches surface as prepare/exec errors. In either case
            // the honest recovery is: wipe + rebuild from git.
            eprintln!("cache at {} looks corrupt: {e}", path.display());
            eprintln!("wiping and re-creating…");
            wipe(path)?;
            try_open(path).context("re-opening cache after wipe")
        }
    }
}

fn try_open(path: &Path) -> Result<Cache> {
    let conn = Connection::open(path)
        .with_context(|| format!("opening cache at {}", path.display()))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    // Perf pragmas: keep temp tables and index scans in RAM, memory-map the
    // main db file so reads on big caches don't hit read() syscalls.
    conn.pragma_update(None, "temp_store", "MEMORY")?;
    conn.pragma_update(None, "cache_size", -20_000i64)?; // ~20 MiB
    conn.pragma_update(None, "mmap_size", 268_435_456i64)?; // 256 MiB
    // Fast integrity probe — pull the pragma result. Returns "ok" when healthy.
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .unwrap_or_else(|_| "corrupt".into());
    if integrity != "ok" {
        anyhow::bail!("integrity_check returned {integrity:?}");
    }
    schema::migrate(&conn)?;
    Ok(Cache { conn })
}

fn wipe(path: &Path) -> Result<()> {
    for suffix in ["", "-wal", "-shm", "-journal"] {
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
