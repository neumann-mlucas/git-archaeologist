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
    let conn = Connection::open(path)
        .with_context(|| format!("opening cache at {}", path.display()))?;
    tune(&conn)?;
    schema::migrate(&conn)?;
    Ok(Cache { conn })
}

/// Cap DuckDB memory + parallelism so indexing large repos (tens of
/// thousands of commits) doesn't OOM the host. Insertion order is
/// irrelevant here — every table has an explicit PK.
fn tune(conn: &Connection) -> Result<()> {
    conn.execute_batch(
        "SET preserve_insertion_order = false;
         SET memory_limit = '4GB';
         SET threads = 2;",
    )?;
    Ok(())
}
