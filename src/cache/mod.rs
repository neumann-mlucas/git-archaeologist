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
    schema::migrate(&conn)?;
    Ok(Cache { conn })
}
