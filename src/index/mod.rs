pub mod bucket;
pub mod churn;
pub mod mailmap;
pub mod tokei_run;
pub mod walker;

use std::collections::HashSet;

use anyhow::{Context, Result};
use crossbeam_channel::Sender;
use rusqlite::params;

use crate::cache::{queries, Cache};
use crate::index::bucket::BucketSize;
use crate::index::mailmap::AuthorResolver;
use crate::index::walker::CommitInfo;
use crate::repo::Repo;

#[derive(Debug, Clone)]
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
    if opts.force_full {
        wipe_data(cache)?;
    }

    let already: HashSet<String> = existing_shas(cache)?;

    let all_commits = walker::walk(repo, /* skip_merges */ true)?;
    if all_commits.is_empty() {
        return Ok(());
    }

    let size = opts
        .bucket_override
        .unwrap_or_else(|| bucket::auto(all_commits.len()));

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

    // One big transaction — SQLite is much faster this way.
    let tx = cache.conn.transaction()?;

    for (i, (commit, plan)) in all_commits.iter().zip(assignments.iter()).enumerate() {
        if already.contains(&commit.sha) && !opts.force_full {
            continue;
        }

        let author_id =
            resolver.resolve(&tx, &commit.author_name, &commit.author_email)?;

        tx.execute(
            "INSERT OR REPLACE INTO commits
             (sha, parent_sha, author_id, committed_at, is_merge, is_sampled, bucket_key)
             VALUES(?1,?2,?3,?4,?5,?6,?7)",
            params![
                commit.sha,
                commit.parent_sha,
                author_id,
                commit.committed_at.unix_timestamp(),
                commit.is_merge as i64,
                plan.is_sampled as i64,
                plan.bucket_key,
            ],
        )?;

        // Churn: always per-commit, full-resolution.
        if let Ok(churn_rows) = churn::for_commit(repo, &commit.sha) {
            let mut stmt = tx.prepare_cached(
                "INSERT OR REPLACE INTO churn(sha, path, added, deleted) VALUES(?1,?2,?3,?4)",
            )?;
            for c in churn_rows {
                stmt.execute(params![commit.sha, c.path, c.added, c.deleted])?;
            }
        }

        // File stats: only on sampled commits.
        if plan.is_sampled {
            if let Ok(files) = tokei_run::snapshot(repo, &commit.sha) {
                let mut stmt = tx.prepare_cached(
                    "INSERT OR REPLACE INTO file_stats(sha, path, language, code, comments, blanks)
                     VALUES(?1,?2,?3,?4,?5,?6)",
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

        if let Some(sender) = &progress {
            let _ = sender.send(Progress::Commit {
                done: i + 1,
                total,
            });
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
    Ok(iter.collect::<rusqlite::Result<_>>()?)
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
