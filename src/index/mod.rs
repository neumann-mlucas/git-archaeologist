pub mod blame;
pub mod bucket;
pub mod churn;
pub mod mailmap;
pub mod treesitter;
pub mod walker;

use std::collections::HashSet;

use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use duckdb::params;

use crate::cache::{queries, Cache};
use crate::index::bucket::BucketSize;
use crate::index::mailmap::AuthorResolver;
use crate::index::walker::CommitInfo;
use crate::repo::Repo;

#[derive(Debug, Clone)]
pub enum Progress {
    Started { total_commits: usize, sampled: usize },
    Commit { done: usize, total: usize },
    Blame { done: usize, total: usize },
    Finished,
}

pub struct IndexOptions {
    pub force_full: bool,
    pub bucket_override: Option<BucketSize>,
}

pub fn run(
    repo: &Repo,
    cache: &mut Cache,
    opts: IndexOptions,
    progress: Option<Sender<Progress>>,
) -> Result<()> {
    let all_commits = walker::walk(repo, /* skip_merges */ true)?;
    if all_commits.is_empty() {
        return Ok(());
    }

    let size = opts
        .bucket_override
        .unwrap_or_else(|| bucket::auto(all_commits.len()));

    if opts.force_full {
        wipe_data(cache)?;
    }

    let already: HashSet<String> = existing_shas(cache)?;

    queries::set_meta(&cache.conn, "bucket_size", &format!("{size:?}"))?;

    // Assign bucket_key to every commit; mark last-per-bucket as sampled.
    let assignments = assign_buckets(&all_commits, size);

    let total = all_commits.len();
    let sampled_count = assignments.iter().filter(|a| a.is_sampled).count();
    if let Some(tx) = &progress {
        let _ = tx.send(Progress::Started {
            total_commits: total,
            sampled: sampled_count,
        });
    }

    // Load aliases fresh each run (cheap).
    let cfg = crate::config::load().context("loading user config for aliases")?;
    let mut resolver = AuthorResolver::new(repo, &cfg.aliases);

    // Batch-fetch all churn in one git log invocation (one fork/exec vs N).
    // Filtered down to commits we still need to write.
    let churn_map = churn::batch_all(repo).unwrap_or_default();

    // Blob-parse memoization across sampled commits.
    let mut blob_cache = treesitter::BlobCache::default();
    let mut lang_registry = treesitter::LangRegistry::new();

    // Chunked transactions — one giant tx OOMs DuckDB on large repos
    // (buffered rows never released mid-tx). Re-run is idempotent via
    // ON CONFLICT DO UPDATE, so a partial commit boundary is safe.
    const COMMIT_CHUNK: usize = 500;

    // Time-based progress throttle. Coarse commit-count throttling looked
    // stalled on slow diffs (large trees, single 10s commit → no update).
    let mut last_progress_at = std::time::Instant::now();
    let progress_interval = std::time::Duration::from_millis(100);

    let mut tx = cache.conn.transaction()?;
    let mut in_chunk: usize = 0;

    for (i, (commit, plan)) in all_commits.iter().zip(assignments.iter()).enumerate() {
        if already.contains(&commit.sha) && !opts.force_full {
            continue;
        }

        let author_id =
            resolver.resolve(&tx, &commit.author_name, &commit.author_email)?;

        // DuckDB has no OR REPLACE; use INSERT ... ON CONFLICT DO UPDATE.
        tx.execute(
            "INSERT INTO commits
             (sha, parent_sha, author_id, committed_at, is_merge, is_sampled, bucket_key)
             VALUES(?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (sha) DO UPDATE SET
                parent_sha   = excluded.parent_sha,
                author_id    = excluded.author_id,
                committed_at = excluded.committed_at,
                is_merge     = excluded.is_merge,
                is_sampled   = excluded.is_sampled,
                bucket_key   = excluded.bucket_key",
            params![
                commit.sha,
                commit.parent_sha,
                author_id,
                commit.committed_at.unix_timestamp(),
                commit.is_merge,
                plan.is_sampled,
                plan.bucket_key,
            ],
        )?;

        if let Some(rows) = churn_map.get(&commit.sha) {
            let mut stmt = tx.prepare_cached(
                "INSERT INTO churn(sha, path, added, deleted) VALUES(?, ?, ?, ?)
                 ON CONFLICT (sha, path) DO UPDATE SET
                    added   = excluded.added,
                    deleted = excluded.deleted",
            )?;
            for c in rows {
                stmt.execute(params![commit.sha, c.path, c.added, c.deleted])?;
            }
        }

        // File stats: only on sampled commits.
        if plan.is_sampled {
            if let Ok(files) = treesitter::snapshot(
                repo,
                &commit.sha,
                &mut blob_cache,
                &mut lang_registry,
            ) {
                let mut stmt = tx.prepare_cached(
                    "INSERT INTO file_stats(sha, path, language, code, comments, blanks)
                     VALUES(?, ?, ?, ?, ?, ?)
                     ON CONFLICT (sha, path) DO UPDATE SET
                        language = excluded.language,
                        code     = excluded.code,
                        comments = excluded.comments,
                        blanks   = excluded.blanks",
                )?;
                for f in files {
                    stmt.execute(params![
                        commit.sha,
                        f.path,
                        f.language,
                        f.code,
                        f.comments,
                        f.blanks
                    ])?;
                }
            }
        }

        in_chunk += 1;
        if in_chunk >= COMMIT_CHUNK {
            tx.commit()?;
            in_chunk = 0;
            tx = cache.conn.transaction()?;
        }

        // Throttle: emit at most one progress event per `progress_interval`,
        // plus one on the final commit. Time-based so slow diffs still show
        // motion; fast repos aren't drowned in 35k channel sends.
        if let Some(sender) = &progress {
            let done = i + 1;
            let now = std::time::Instant::now();
            if done == total || now.duration_since(last_progress_at) >= progress_interval {
                let _ = sender.send(Progress::Commit { done, total });
                last_progress_at = now;
            }
        }
    }

    if let Some(head) = all_commits.last() {
        queries::set_indexed_head(&tx, &head.sha)?;
    }
    tx.commit()?;

    // Blame pass — attribute lines in every sampled snapshot. Skip shas
    // that already have blame rows (idempotent re-run). Cost scales as
    // O(files × sampled buckets × blame_runtime); the sampling in the
    // walker already keeps `sampled buckets` small (~52 for a year of
    // weekly data), so the walk stays tractable.
    let already_blamed: HashSet<String> = {
        let mut stmt = cache.conn.prepare("SELECT DISTINCT sha FROM blame")?;
        let iter = stmt.query_map([], |r| r.get::<_, String>(0))?;
        iter.collect::<duckdb::Result<_>>()?
    };

    let sampled_shas: Vec<String> = all_commits
        .iter()
        .zip(assignments.iter())
        .filter(|(_, plan)| plan.is_sampled)
        .map(|(c, _)| c.sha.clone())
        .filter(|s| opts.force_full || !already_blamed.contains(s))
        .collect();

    let total_shas = sampled_shas.len();
    let mut last_blame_at = std::time::Instant::now();

    for (idx, sha) in sampled_shas.iter().enumerate() {
        let counts =
            match blame::snapshot(repo, sha, &mut resolver, &cache.conn, &lang_registry) {
                Ok(c) => c,
                Err(_) => continue, // per-sha failure (missing commit, etc.) → skip
            };

        // Wipe any existing blame for this sha before repopulating (idempotent).
        cache
            .conn
            .execute("DELETE FROM blame WHERE sha = ?", params![sha])?;

        let tx = cache.conn.transaction()?;
        {
            let mut stmt = tx.prepare(
                "INSERT INTO blame(sha, path, author_id, line_count) VALUES(?, ?, ?, ?)
                 ON CONFLICT (sha, path, author_id) DO UPDATE SET
                    line_count = excluded.line_count",
            )?;
            for entry in &counts {
                for (author_id, lines) in &entry.by_author {
                    stmt.execute(params![sha, entry.path, author_id, *lines as i64])?;
                }
            }
        }
        tx.commit()?;

        if let Some(sender) = &progress {
            let done = idx + 1;
            let now = std::time::Instant::now();
            if done == total_shas || now.duration_since(last_blame_at) >= progress_interval
            {
                let _ = sender.send(Progress::Blame { done, total: total_shas });
                last_blame_at = now;
            }
        }
    }

    if let Some(sender) = &progress {
        let _ = sender.send(Progress::Finished);
    }

    Ok(())
}

fn wipe_data(cache: &Cache) -> Result<()> {
    cache.conn.execute_batch(
        "DELETE FROM blame; DELETE FROM file_stats; DELETE FROM churn; DELETE FROM commits;",
    )?;
    Ok(())
}

fn existing_shas(cache: &Cache) -> Result<HashSet<String>> {
    let mut stmt = cache.conn.prepare("SELECT sha FROM commits")?;
    let iter = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(iter.collect::<duckdb::Result<_>>()?)
}

struct BucketAssignment {
    bucket_key: i64,
    is_sampled: bool,
}

/// Assign a bucket key to each commit (in-order, oldest first) and mark the
/// last commit of each bucket as sampled.
fn assign_buckets(commits: &[CommitInfo], size: BucketSize) -> Vec<BucketAssignment> {
    let mut keys: Vec<i64> = commits
        .iter()
        .map(|c| bucket::bucket_key(c.committed_at, size))
        .collect();

    let mut is_sampled = vec![false; commits.len()];
    for i in 0..commits.len() {
        let is_last_in_bucket = i + 1 == commits.len() || keys[i + 1] != keys[i];
        if is_last_in_bucket {
            is_sampled[i] = true;
        }
    }

    keys.drain(..)
        .zip(is_sampled)
        .map(|(bucket_key, is_sampled)| BucketAssignment {
            bucket_key,
            is_sampled,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::process::Command;

    use tempfile::TempDir;

    use crate::cache;
    use crate::index::{self, bucket::BucketSize};
    use crate::repo;

    /// Init a git repo in `dir` and add `n` commits touching one Rust file
    /// each. Each commit adds ~2 lines. Skips if `git` isn't on PATH.
    fn seed_repo(dir: &Path, n: usize) {
        let run_git = |args: &[&str], date: &str| {
            Command::new("git")
                .args(args)
                .current_dir(dir)
                .env("GIT_AUTHOR_NAME", "Alice")
                .env("GIT_AUTHOR_EMAIL", "alice@example.com")
                .env("GIT_COMMITTER_NAME", "Alice")
                .env("GIT_COMMITTER_EMAIL", "alice@example.com")
                .env("GIT_AUTHOR_DATE", date)
                .env("GIT_COMMITTER_DATE", date)
                .status()
                .expect("git failed to execute")
        };
        let noop = "2025-01-01T00:00:00Z";
        assert!(run_git(&["init", "-q", "-b", "main"], noop).success());
        assert!(run_git(&["config", "commit.gpgsign", "false"], noop).success());
        for i in 0..n {
            let path = dir.join("main.rs");
            let mut body = String::new();
            for k in 0..=i {
                body.push_str(&format!("fn f{k}() {{ println!(\"{k}\"); }}\n"));
            }
            std::fs::write(&path, body).unwrap();
            assert!(run_git(&["add", "main.rs"], noop).success());
            // Distinct timestamp per commit so bucket_key(Commit) differs
            // and each commit is its own sampled bucket.
            let date = format!("2025-01-0{}T00:00:00Z", i + 1);
            assert!(
                run_git(&["commit", "-q", "-m", &format!("commit {i}")], &date).success(),
                "commit {i} failed"
            );
        }
    }

    #[test]
    fn smoke_index_tiny_repo() {
        // Skip when `git` isn't available.
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("skip: git not on PATH");
            return;
        }

        let td = TempDir::new().unwrap();
        seed_repo(td.path(), 3);

        let repo = repo::open(td.path()).expect("open repo");

        // Redirect the DuckDB cache to a temp file so we don't touch
        // the user's XDG data dir.
        let cache_file = td.path().join("cache.duckdb");
        let mut cache = cache::open(&cache_file).expect("open cache");

        index::run(
            &repo,
            &mut cache,
            index::IndexOptions {
                force_full: true,
                bucket_override: Some(BucketSize::Commit),
            },
            None,
        )
        .expect("index run");

        let stats = crate::query::cache_stats(&cache.conn).unwrap();
        assert_eq!(stats.commits, 3, "commits row count");
        assert!(stats.churn >= 3, "churn rows >= commits");
        assert!(stats.file_stats >= 3, "file_stats rows >= sampled commits");
        assert!(stats.authors >= 1, "authors row present");
    }
}
