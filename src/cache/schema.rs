use anyhow::Result;
use duckdb::Connection;

const CURRENT_VERSION: i64 = 1;

// DuckDB dialect notes vs the prior sqlite schema:
//  - No INTEGER PRIMARY KEY autoinc — use IDENTITY.
//  - FK enforcement is off by default; declarations are informational.
//  - No PRAGMA journal_mode/synchronous — DuckDB has its own WAL.
//  - We keep a `schema_version` row in `meta` since DuckDB has no
//    `user_version` pragma.
const SCHEMA: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE SEQUENCE IF NOT EXISTS seq_authors_id START 1;

CREATE TABLE IF NOT EXISTS authors (
    id              BIGINT PRIMARY KEY DEFAULT nextval('seq_authors_id'),
    canonical_name  TEXT NOT NULL,
    canonical_email TEXT NOT NULL,
    UNIQUE(canonical_name, canonical_email)
);

CREATE TABLE IF NOT EXISTS author_aliases (
    author_id BIGINT NOT NULL,
    raw_name  TEXT NOT NULL,
    raw_email TEXT NOT NULL,
    PRIMARY KEY (raw_name, raw_email)
);

CREATE TABLE IF NOT EXISTS commits (
    sha          TEXT PRIMARY KEY,
    parent_sha   TEXT,
    author_id    BIGINT NOT NULL,
    committed_at BIGINT NOT NULL,
    is_merge     BOOLEAN NOT NULL,
    is_sampled   BOOLEAN NOT NULL,
    bucket_key   BIGINT NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_commits_ts     ON commits(committed_at);
CREATE INDEX IF NOT EXISTS idx_commits_bucket ON commits(bucket_key);
CREATE INDEX IF NOT EXISTS idx_commits_author ON commits(author_id);

CREATE TABLE IF NOT EXISTS file_stats (
    sha      TEXT NOT NULL,
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
    sha     TEXT NOT NULL,
    path    TEXT NOT NULL,
    added   INTEGER NOT NULL,
    deleted INTEGER NOT NULL,
    PRIMARY KEY (sha, path)
);
CREATE INDEX IF NOT EXISTS idx_churn_path ON churn(path);
"#;

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    // Track schema version in meta rather than user_version.
    conn.execute(
        "INSERT INTO meta(key, value) VALUES('schema_version', ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        [CURRENT_VERSION.to_string()],
    )?;
    Ok(())
}
