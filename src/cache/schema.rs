use anyhow::Result;
use duckdb::Connection;

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
    Ok(())
}
