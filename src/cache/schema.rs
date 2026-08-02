use anyhow::Result;
use duckdb::Connection;

pub const SCHEMA_VERSION: &str = "2";

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
    sha           TEXT PRIMARY KEY,
    author_id     BIGINT NOT NULL,
    committer_id  BIGINT NOT NULL,
    authored_at   BIGINT NOT NULL,
    is_merge      BOOLEAN NOT NULL,
    is_sampled    BOOLEAN NOT NULL,
    bucket_key    BIGINT NOT NULL,
    msg_type      TEXT,
    is_breaking   BOOLEAN NOT NULL,
    is_revert     BOOLEAN NOT NULL,
    ignored_blame BOOLEAN NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_commits_ts     ON commits(authored_at);
CREATE INDEX IF NOT EXISTS idx_commits_bucket ON commits(bucket_key);
CREATE INDEX IF NOT EXISTS idx_commits_author ON commits(author_id);

CREATE TABLE IF NOT EXISTS commit_parents (
    sha        TEXT    NOT NULL,
    parent_sha TEXT    NOT NULL,
    parent_idx INTEGER NOT NULL,
    PRIMARY KEY (sha, parent_idx)
);

CREATE TABLE IF NOT EXISTS commit_trailers (
    sha       TEXT    NOT NULL,
    author_id BIGINT  NOT NULL,
    role      TEXT    NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_trailers_sha ON commit_trailers(sha);

CREATE TABLE IF NOT EXISTS tags (
    name      TEXT PRIMARY KEY,
    sha       TEXT NOT NULL,
    tagged_at BIGINT NOT NULL
);

CREATE TABLE IF NOT EXISTS hunks (
    sha        TEXT    NOT NULL,
    path       TEXT    NOT NULL,
    prev_path  TEXT,
    old_start  INTEGER NOT NULL,
    old_len    INTEGER NOT NULL,
    new_start  INTEGER NOT NULL,
    new_len    INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_hunks_sha_path ON hunks(sha, path);
CREATE INDEX IF NOT EXISTS idx_hunks_path     ON hunks(path);

CREATE TABLE IF NOT EXISTS file_churn (
    sha     TEXT NOT NULL,
    path    TEXT NOT NULL,
    added   INTEGER NOT NULL,
    deleted INTEGER NOT NULL,
    PRIMARY KEY (sha, path)
);
CREATE INDEX IF NOT EXISTS idx_file_churn_path ON file_churn(path);

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

CREATE TABLE IF NOT EXISTS funcs (
    sha        TEXT    NOT NULL,
    path       TEXT    NOT NULL,
    name       TEXT    NOT NULL,
    kind       TEXT    NOT NULL,
    start_line INTEGER NOT NULL,
    end_line   INTEGER NOT NULL,
    PRIMARY KEY (sha, path, name, start_line)
);
CREATE INDEX IF NOT EXISTS idx_funcs_path ON funcs(path);

CREATE TABLE IF NOT EXISTS line_births (
    path         TEXT    NOT NULL,
    line_no      INTEGER NOT NULL,
    birth_sha    TEXT    NOT NULL,
    birth_bucket BIGINT  NOT NULL
);
CREATE INDEX IF NOT EXISTS idx_line_births_bucket ON line_births(birth_bucket);
CREATE INDEX IF NOT EXISTS idx_line_births_path   ON line_births(path);
"#;

pub fn migrate(conn: &Connection) -> Result<()> {
    conn.execute_batch(SCHEMA)?;
    // Stamp schema_version on first run; drift detection lives in cache::open.
    conn.execute(
        "INSERT INTO meta(key, value) VALUES('schema_version', ?)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        duckdb::params![SCHEMA_VERSION],
    )?;
    Ok(())
}
