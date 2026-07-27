use std::collections::HashMap;

use anyhow::{Context, Result};
use gix::actor::SignatureRef;
use rusqlite::{params, Connection};

use crate::config::Aliases;
use crate::repo::Repo;

/// Resolves raw (name, email) → canonical (name, email) using .mailmap plus
/// user-supplied aliases, and caches DB author ids in-memory.
pub struct AuthorResolver {
    mailmap: gix::mailmap::Snapshot,
    user_override: HashMap<(String, String), (String, String)>,
    id_cache: HashMap<(String, String), i64>,
}

impl AuthorResolver {
    pub fn new(repo: &Repo, aliases: &Aliases) -> Self {
        let mailmap = repo.git.open_mailmap();

        let mut user_override = HashMap::new();
        for entry in &aliases.entries {
            for raw in &entry.raw {
                user_override.insert(
                    (raw.name.clone(), raw.email.clone()),
                    (entry.canonical_name.clone(), entry.canonical_email.clone()),
                );
            }
        }

        Self {
            mailmap,
            user_override,
            id_cache: HashMap::new(),
        }
    }

    /// Get or insert the DB `authors.id` for a raw (name, email) pair.
    pub fn resolve(
        &mut self,
        conn: &Connection,
        raw_name: &str,
        raw_email: &str,
    ) -> Result<i64> {
        let key = (raw_name.to_string(), raw_email.to_string());
        if let Some(id) = self.id_cache.get(&key) {
            return Ok(*id);
        }

        let (canonical_name, canonical_email) = self.canonicalize(raw_name, raw_email);

        // Upsert canonical author.
        conn.execute(
            "INSERT OR IGNORE INTO authors(canonical_name, canonical_email) VALUES(?1, ?2)",
            params![canonical_name, canonical_email],
        )?;
        let id: i64 = conn
            .query_row(
                "SELECT id FROM authors WHERE canonical_name = ?1 AND canonical_email = ?2",
                params![canonical_name, canonical_email],
                |r| r.get(0),
            )
            .context("looking up author id")?;

        // Record alias link (raw → id).
        conn.execute(
            "INSERT OR IGNORE INTO author_aliases(author_id, raw_name, raw_email) VALUES(?1, ?2, ?3)",
            params![id, raw_name, raw_email],
        )?;

        self.id_cache.insert(key, id);
        Ok(id)
    }

    fn canonicalize(&self, raw_name: &str, raw_email: &str) -> (String, String) {
        // User overrides win over .mailmap.
        if let Some((n, e)) = self.user_override.get(&(raw_name.to_string(), raw_email.to_string())) {
            return (n.clone(), e.clone());
        }

        let sig = SignatureRef {
            name: raw_name.into(),
            email: raw_email.into(),
            time: Default::default(),
        };
        let resolved = self.mailmap.resolve(sig);
        (resolved.name.to_string(), resolved.email.to_string())
    }
}
