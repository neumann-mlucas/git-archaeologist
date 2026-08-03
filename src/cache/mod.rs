use std::path::Path;

use anyhow::{Context, Result};
use duckdb::Connection;

pub mod queries;
pub mod schema;

pub struct Cache {
    pub conn: Connection,
}

pub fn open(path: impl AsRef<Path>) -> Result<Cache> {
    let path = path.as_ref();

    // Wipe stale caches from prior schema versions. DuckDB stores everything
    // in a single file, so a plain unlink is enough; the next open recreates.
    if path.exists() && is_stale(path) {
        eprintln!("cache: schema drift, wiped {}", path.display());
        std::fs::remove_file(path)
            .with_context(|| format!("removing stale cache {}", path.display()))?;
    }

    let conn =
        Connection::open(path).with_context(|| format!("opening cache at {}", path.display()))?;
    tune(&conn)?;
    schema::migrate(&conn)?;
    Ok(Cache { conn })
}

/// True iff the cache file exists but its `meta.schema_version` doesn't match
/// what this build ships. Any error (missing table, missing row, wrong type)
/// is treated as drift so first-run migration off older builds Just Works.
fn is_stale(path: &Path) -> bool {
    let Ok(conn) = Connection::open(path) else {
        return true;
    };
    let version: Option<String> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |r| r.get(0),
        )
        .ok();
    match version {
        Some(v) => v != schema::SCHEMA_VERSION,
        None => true,
    }
}

/// Cap DuckDB memory so large-repo indexing can't OOM the host, and
/// disable insertion-order preservation (Appender path doesn't need it).
/// Threads default to core count — nothing to set.
///
/// ponytail: memory_limit hard-coded at 12GB. Promote to config if a
/// real user hits the ceiling.
fn tune(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "SET preserve_insertion_order = false;
         SET memory_limit = '12GB';",
    )?;
    Ok(())
}
