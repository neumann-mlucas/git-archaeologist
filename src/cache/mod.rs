use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::Connection;

pub mod queries;
pub mod schema;

pub struct Cache {
    pub conn: Connection,
}

pub fn open(path: impl AsRef<Path>) -> Result<Cache> {
    let conn = Connection::open(path.as_ref())
        .with_context(|| format!("opening cache at {}", path.as_ref().display()))?;
    conn.pragma_update(None, "journal_mode", "WAL")?;
    conn.pragma_update(None, "synchronous", "NORMAL")?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    schema::migrate(&conn)?;
    Ok(Cache { conn })
}
