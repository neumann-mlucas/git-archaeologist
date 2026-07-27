use anyhow::Result;
use rusqlite::Connection;

const CURRENT_VERSION: i64 = 2;

const V1: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS authors (
    id              INTEGER PRIMARY KEY,
    canonical_name  TEXT NOT NULL,
    canonical_email TEXT NOT NULL,
    UNIQUE(canonical_name, canonical_email)
);

CREATE TABLE IF NOT EXISTS author_aliases (
    author_id INTEGER NOT NULL REFERENCES authors(id),
    raw_name  TEXT NOT NULL,
    raw_email TEXT NOT NULL,
    PRIMARY KEY (raw_name, raw_email)
);

CREATE TABLE IF NOT EXISTS commits (
    sha          TEXT PRIMARY KEY,
    parent_sha   TEXT,
    author_id    INTEGER NOT NULL REFERENCES authors(id),
    committed_at INTEGER NOT NULL,
    is_merge     INTEGER NOT NULL,
    is_sampled   INTEGER NOT NULL,
    bucket_key   INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_commits_ts     ON commits(committed_at);
CREATE INDEX IF NOT EXISTS idx_commits_bucket ON commits(bucket_key);

CREATE TABLE IF NOT EXISTS file_stats (
    sha      TEXT NOT NULL REFERENCES commits(sha),
    path     TEXT NOT NULL,
    language TEXT NOT NULL,
    code     INTEGER NOT NULL,
    comments INTEGER NOT NULL,
    blanks   INTEGER NOT NULL,
    PRIMARY KEY (sha, path)
);
CREATE INDEX IF NOT EXISTS idx_file_stats_lang ON file_stats(language);
CREATE INDEX IF NOT EXISTS idx_file_stats_path ON file_stats(path);

CREATE TABLE IF NOT EXISTS churn (
    sha     TEXT NOT NULL REFERENCES commits(sha),
    path    TEXT NOT NULL,
    added   INTEGER NOT NULL,
    deleted INTEGER NOT NULL,
    PRIMARY KEY (sha, path)
);
CREATE INDEX IF NOT EXISTS idx_churn_path ON churn(path);
"#;

const V2: &str = r#"
-- Author-filtered queries scan commits.author_id — index it.
CREATE INDEX IF NOT EXISTS idx_commits_author ON commits(author_id);
"#;

pub fn migrate(conn: &Connection) -> Result<()> {
    let version: i64 = conn
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap_or(0);

    if version < 1 {
        conn.execute_batch(V1)?;
    }
    if version < 2 {
        conn.execute_batch(V2)?;
    }

    conn.pragma_update(None, "user_version", CURRENT_VERSION)?;
    Ok(())
}
