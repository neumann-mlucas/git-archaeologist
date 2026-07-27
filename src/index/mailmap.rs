use anyhow::Result;
use rusqlite::{params, Connection};

use crate::config::Aliases;
use crate::repo::Repo;

#[derive(Debug, Clone)]
pub struct CanonicalAuthor {
    pub id: i64,
    pub name: String,
    pub email: String,
}

/// Populate `authors` + `author_aliases` from repo `.mailmap` and user aliases.
///
/// User aliases win over mailmap on conflict.
pub fn sync(_repo: &Repo, _aliases: &Aliases, _conn: &Connection) -> Result<()> {
    // 1. Collect all (raw_name, raw_email) pairs seen in commits table (or on the fly)
    // 2. For each, apply mailmap → (canonical_name, canonical_email)
    // 3. Apply user aliases override → final canonical identity
    // 4. Upsert into authors, link via author_aliases
    todo!("populate authors + author_aliases")
}

pub fn canonicalize(
    conn: &Connection,
    raw_name: &str,
    raw_email: &str,
) -> Result<Option<i64>> {
    let id = conn
        .query_row(
            "SELECT author_id FROM author_aliases WHERE raw_name=?1 AND raw_email=?2",
            params![raw_name, raw_email],
            |r| r.get::<_, i64>(0),
        )
        .ok();
    Ok(id)
}
