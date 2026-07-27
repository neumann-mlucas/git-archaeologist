use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use rusqlite::{Connection, ErrorCode};

pub mod queries;
pub mod schema;

pub struct Cache {
    pub conn: Connection,
}

/// Marker error: wrapping this in `anyhow::Error` signals that the cache is
/// unrecoverably corrupt and should be wiped, not that some transient sqlite
/// failure occurred (e.g. lock contention).
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
struct CorruptMarker(String);

pub fn open(path: impl AsRef<Path>) -> Result<Cache> {
    let path = path.as_ref();
    match try_open(path) {
        Ok(c) => Ok(c),
        Err(e) if is_corruption(&e) => {
            eprintln!("cache at {} is corrupt: {e}", path.display());
            eprintln!("wiping and re-creating…");
            wipe(path)?;
            try_open(path).context("re-opening cache after wipe")
        }
        Err(e) => Err(e),
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
    // Fast integrity probe — pull the pragma result. Returns "ok" when healthy;
    // anything else means corruption. Errors from the PRAGMA itself are only
    // treated as corruption when SQLite explicitly says so (see is_corruption).
    let integrity: String = conn
        .query_row("PRAGMA integrity_check", [], |r| r.get(0))
        .context("running integrity_check")?;
    if integrity != "ok" {
        return Err(anyhow::Error::new(CorruptMarker(format!(
            "integrity_check returned {integrity:?}"
        ))));
    }
    schema::migrate(&conn)?;
    Ok(Cache { conn })
}

fn is_corruption(err: &anyhow::Error) -> bool {
    for cause in err.chain() {
        if cause.downcast_ref::<CorruptMarker>().is_some() {
            return true;
        }
        if let Some(rusqlite::Error::SqliteFailure(f, _)) =
            cause.downcast_ref::<rusqlite::Error>()
        {
            if matches!(f.code, ErrorCode::DatabaseCorrupt | ErrorCode::NotADatabase) {
                return true;
            }
        }
    }
    false
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
