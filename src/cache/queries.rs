use anyhow::Result;
use duckdb::{params, Connection};

pub fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut stmt = conn.prepare("SELECT value FROM meta WHERE key = ?")?;
    let val = stmt
        .query_row(params![key], |r| r.get::<_, String>(0))
        .ok();
    Ok(val)
}

pub fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES(?, ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

pub fn set_indexed_head(conn: &Connection, sha: &str) -> Result<()> {
    set_meta(conn, "indexed_head_sha", sha)
}
