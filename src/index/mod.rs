pub mod bucket;
pub mod churn;
pub mod mailmap;
pub mod tokei_run;
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
#[allow(dead_code)] // consumed by M7 progress modal
pub enum Progress {
    Started { total_commits: usize, sampled: usize },
    Commit { done: usize, total: usize },
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
    let mut blob_cache = tokei_run::BlobCache::default();

    // One big transaction — SQLite is much faster this way.
    let tx = cache.conn.transaction()?;

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
            if let Ok(files) = tokei_run::snapshot(repo, &commit.sha, &mut blob_cache) {
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

        // Throttle: 35k channel sends dominate over the actual index work on
        // fast repos. One update per ~64 commits (and one for the final one).
        if let Some(sender) = &progress {
            let done = i + 1;
            if done == total || done & 0x3F == 0 {
                let _ = sender.send(Progress::Commit { done, total });
            }
        }
    }

    if let Some(head) = all_commits.last() {
        queries::set_indexed_head(&tx, &head.sha)?;
    }

    tx.commit()?;

    if let Some(sender) = &progress {
        let _ = sender.send(Progress::Finished);
    }

    Ok(())
}

fn wipe_data(cache: &Cache) -> Result<()> {
    cache.conn.execute_batch(
        "DELETE FROM file_stats; DELETE FROM churn; DELETE FROM commits;",
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
