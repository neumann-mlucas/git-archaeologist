pub mod bucket;
pub mod churn;
pub mod cohort;
pub mod mailmap;
pub mod parse;
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
    let ignore_revs = parse::load_ignore_revs(&repo.root);

    let all_commits = walker::walk(repo, &ignore_revs)?;
    if all_commits.is_empty() {
        return Ok(());
    }

    let mut size = opts
        .bucket_override
        .unwrap_or_else(|| bucket::auto(all_commits.len()));

    // Tag-bucket collection is done ahead of assign so we can inject dates
    // even when the override didn't ask for `Tag`. If the user asked for
    // `Tag` but no reachable tags exist, degrade to auto so downstream
    // metrics still work.
    let tag_dates: Vec<i64> = {
        let mut v: Vec<i64> = walker::tags(repo)
            .unwrap_or_default()
            .into_iter()
            .map(|t| t.tagged_at)
            .collect();
        v.sort_unstable();
        v
    };
    if size == BucketSize::Tag && tag_dates.is_empty() {
        eprintln!("bucket=tag requested but repo has no tags; falling back to auto");
        size = bucket::auto(all_commits.len());
    }

    if opts.force_full {
        wipe_data(cache)?;
    }

    let already: HashSet<String> = existing_shas(cache)?;

    queries::set_meta(&cache.conn, "bucket_size", &format!("{size:?}"))?;

    let assignments = assign_buckets(&all_commits, size, &tag_dates);

    let total = all_commits.len();
    let sampled_count = assignments.iter().filter(|a| a.is_sampled).count();
    if let Some(tx) = &progress {
        let _ = tx.send(Progress::Started {
            total_commits: total,
            sampled: sampled_count,
        });
    }
    // Direct stderr line every 100ms + at start/end. Independent of the
    // channel — CLI mode has no channel consumer, and this is the only
    // visible signal that `git-arch index` is doing something.
    eprintln!("indexing {total} commits ({sampled_count} sampled)…");
    let start_at = std::time::Instant::now();

    let cfg = crate::config::load().context("loading user config for aliases")?;
    let mut resolver = AuthorResolver::new(repo, &cfg.aliases);

    let churn_map = churn::batch_all(repo).unwrap_or_default();

    let mut blob_cache = treesitter::BlobCache::default();
    let mut lang_registry = treesitter::LangRegistry::new();

    // Flush cadence: how many commits between BEGIN/COMMIT chunk
    // boundaries + Appender flushes. Trade-off:
    // - large chunks minimize per-commit sync cost (auto-commit = disk
    //   round-trip per row), but hold every row in RAM until commit.
    // - small chunks bound RAM but pay more sync cost.
    // On ratatui-scale (2.5k commits, ~150 hunks each), 50 keeps peak
    // RSS < 1 GB while running >5 commits/s. Not a knob.
    const FLUSH_EVERY: usize = 50;

    let mut last_progress_at = std::time::Instant::now();
    let progress_interval = std::time::Duration::from_millis(100);

    // Drop secondary indexes on high-write tables during bulk insert;
    // rebuild at the end. DuckDB pays index maintenance in memory per
    // insert; on a mid-size repo the hunks index alone is the memory
    // hotspot. Recreate identically to schema.rs.
    cache.conn.execute_batch(
        "DROP INDEX IF EXISTS idx_hunks_sha_path;
         DROP INDEX IF EXISTS idx_hunks_path;
         DROP INDEX IF EXISTS idx_file_churn_path;
         DROP INDEX IF EXISTS idx_file_stats_lang;
         DROP INDEX IF EXISTS idx_file_stats_path;
         DROP INDEX IF EXISTS idx_funcs_path;
         DROP INDEX IF EXISTS idx_trailers_sha;
         DROP INDEX IF EXISTS idx_commits_ts;
         DROP INDEX IF EXISTS idx_commits_bucket;
         DROP INDEX IF EXISTS idx_commits_author;
         DROP INDEX IF EXISTS idx_line_births_bucket;
         DROP INDEX IF EXISTS idx_line_births_path;",
    )?;

    // Tags — inserted after commits so the FK target is present. Reuse
    // the list we already walked for tag-bucket assignment.
    let tag_infos = walker::tags(repo).unwrap_or_default();

    // Everything write-heavy goes through Appender. Under `--force-full`
    // we wiped the tables above; on incremental runs, `already` filters
    // SHAs whose rows already exist. Either way, no PK/UNIQUE conflicts.
    // Dropping ON CONFLICT DO UPDATE means we lose the "re-run same SHA
    // and refresh in place" behavior — but v1 doesn't rely on it: only
    // path is force_full (wipe) or incremental-append (skip).
    let mut commits_app = cache.conn.appender("commits").context("commits appender")?;
    let mut parents_app = cache
        .conn
        .appender("commit_parents")
        .context("commit_parents appender")?;
    let mut trailers_app = cache
        .conn
        .appender("commit_trailers")
        .context("commit_trailers appender")?;
    let mut hunks_app = cache.conn.appender("hunks").context("hunks appender")?;
    let mut churn_app = cache
        .conn
        .appender("file_churn")
        .context("file_churn appender")?;
    let mut file_stats_app = cache
        .conn
        .appender("file_stats")
        .context("file_stats appender")?;
    let mut funcs_app = cache.conn.appender("funcs").context("funcs appender")?;

    let mut since_flush: usize = 0;

    for (i, (commit, plan)) in all_commits.iter().zip(assignments.iter()).enumerate() {
        if already.contains(&commit.sha) && !opts.force_full {
            continue;
        }

        let author_id =
            resolver.resolve(&cache.conn, &commit.author_name, &commit.author_email)?;
        let committer_id = resolver.resolve(
            &cache.conn,
            &commit.committer_name,
            &commit.committer_email,
        )?;

        commits_app.append_row(params![
            commit.sha,
            author_id,
            committer_id,
            commit.committed_at.unix_timestamp(),
            commit.is_merge,
            plan.is_sampled,
            plan.bucket_key,
            commit.conv.msg_type,
            commit.conv.is_breaking,
            commit.conv.is_revert,
            commit.ignored_blame,
        ])?;

        for (idx, parent) in commit.parents.iter().enumerate() {
            parents_app.append_row(params![commit.sha, parent, idx as i32])?;
        }

        for tr in &commit.trailers {
            let tid = resolver.resolve(&cache.conn, &tr.ident.name, &tr.ident.email)?;
            trailers_app.append_row(params![commit.sha, tid, tr.role.as_str()])?;
        }

        if let Some(diff) = churn_map.get(&commit.sha) {
            for c in &diff.churn {
                churn_app.append_row(params![commit.sha, c.path, c.added, c.deleted])?;
            }
            for h in &diff.hunks {
                hunks_app.append_row(params![
                    commit.sha,
                    h.path,
                    h.prev_path,
                    h.old_start,
                    h.old_len,
                    h.new_start,
                    h.new_len,
                ])?;
            }
        }

        if plan.is_sampled {
            if let Ok(files) = treesitter::snapshot(
                repo,
                &commit.sha,
                &mut blob_cache,
                &mut lang_registry,
            ) {
                for f in &files {
                    file_stats_app.append_row(params![
                        commit.sha,
                        f.stat.path,
                        f.stat.language,
                        f.stat.code,
                        f.stat.comments,
                        f.stat.blanks
                    ])?;
                    for def in &f.funcs {
                        funcs_app.append_row(params![
                            commit.sha,
                            f.stat.path,
                            def.name,
                            def.kind,
                            def.start_line,
                            def.end_line,
                        ])?;
                    }
                }
            }
        }

        since_flush += 1;
        if since_flush >= FLUSH_EVERY {
            commits_app.flush()?;
            parents_app.flush()?;
            trailers_app.flush()?;
            hunks_app.flush()?;
            churn_app.flush()?;
            file_stats_app.flush()?;
            funcs_app.flush()?;
            since_flush = 0;
        }

        let done = i + 1;
        let now = std::time::Instant::now();
        if done == total || now.duration_since(last_progress_at) >= progress_interval {
            let elapsed = now.duration_since(start_at).as_secs_f64();
            let rate = if elapsed > 0.0 { done as f64 / elapsed } else { 0.0 };
            let eta_s = if rate > 0.0 {
                (total - done) as f64 / rate
            } else {
                0.0
            };
            eprint!(
                "\rindex: {done}/{total} ({:.0}%) {rate:.0}/s eta {eta_s:.0}s   ",
                (done as f64 / total as f64) * 100.0
            );
            last_progress_at = now;
            if let Some(sender) = &progress {
                let _ = sender.send(Progress::Commit { done, total });
            }
        }
    }
    eprintln!();

    // Final flush + drop appenders before any query/DDL touches these
    // tables. Appender::drop calls flush(), but calling explicitly keeps
    // error handling straightforward and makes the ordering obvious.
    commits_app.flush()?;
    parents_app.flush()?;
    trailers_app.flush()?;
    hunks_app.flush()?;
    churn_app.flush()?;
    file_stats_app.flush()?;
    funcs_app.flush()?;
    drop(commits_app);
    drop(parents_app);
    drop(trailers_app);
    drop(hunks_app);
    drop(churn_app);
    drop(file_stats_app);
    drop(funcs_app);

    // Tags — inserted after all commits so the FK target is present.
    // Ignore tags whose target didn't land in `commits` (foreign tips
    // outside the walked set).
    for t in &tag_infos {
        let commit_exists: bool = cache
            .conn
            .query_row(
                "SELECT COUNT(*) > 0 FROM commits WHERE sha = ?",
                params![t.sha],
                |r| r.get(0),
            )
            .unwrap_or(false);
        if !commit_exists {
            continue;
        }
        cache.conn.execute(
            "INSERT INTO tags(name, sha, tagged_at) VALUES(?, ?, ?)
             ON CONFLICT (name) DO UPDATE SET
                sha       = excluded.sha,
                tagged_at = excluded.tagged_at",
            params![t.name, t.sha, t.tagged_at],
        )?;
    }

    if let Some(head) = all_commits.last() {
        queries::set_indexed_head(&cache.conn, &head.sha)?;
    }

    // Rebuild secondary indexes after bulk write completes. Query paths
    // downstream (cohort fold, subcommand queries) depend on them.
    cache.conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_commits_ts     ON commits(authored_at);
         CREATE INDEX IF NOT EXISTS idx_commits_bucket ON commits(bucket_key);
         CREATE INDEX IF NOT EXISTS idx_commits_author ON commits(author_id);
         CREATE INDEX IF NOT EXISTS idx_trailers_sha   ON commit_trailers(sha);
         CREATE INDEX IF NOT EXISTS idx_hunks_sha_path ON hunks(sha, path);
         CREATE INDEX IF NOT EXISTS idx_hunks_path     ON hunks(path);
         CREATE INDEX IF NOT EXISTS idx_file_churn_path ON file_churn(path);
         CREATE INDEX IF NOT EXISTS idx_file_stats_lang ON file_stats(language);
         CREATE INDEX IF NOT EXISTS idx_file_stats_path ON file_stats(path);
         CREATE INDEX IF NOT EXISTS idx_funcs_path      ON funcs(path);",
    )?;

    // Phase 3 — cohort fold materializes `line_births` from hunks.
    eprintln!("cohort fold…");
    cohort::fold_and_materialize(cache).context("cohort fold")?;

    // Recreate line_births indexes after the fold's bulk insert.
    cache.conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_line_births_bucket ON line_births(birth_bucket);
         CREATE INDEX IF NOT EXISTS idx_line_births_path   ON line_births(path);",
    )?;

    if let Some(sender) = &progress {
        let _ = sender.send(Progress::Finished);
    }

    Ok(())
}

fn wipe_data(cache: &Cache) -> Result<()> {
    cache.conn.execute_batch(
        "DELETE FROM line_births;
         DELETE FROM funcs;
         DELETE FROM file_stats;
         DELETE FROM file_churn;
         DELETE FROM hunks;
         DELETE FROM tags;
         DELETE FROM commit_trailers;
         DELETE FROM commit_parents;
         DELETE FROM commits;",
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

fn assign_buckets(
    commits: &[CommitInfo],
    size: BucketSize,
    tag_dates: &[i64],
) -> Vec<BucketAssignment> {
    let mut keys: Vec<i64> = commits
        .iter()
        .map(|c| match size {
            BucketSize::Tag => bucket::tag_bucket_key(c.committed_at, tag_dates),
            _ => bucket::bucket_key(c.committed_at, size),
        })
        .collect();

    let mut is_sampled = vec![false; commits.len()];
    for i in 0..commits.len() {
        if commits[i].is_merge {
            continue;
        }
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
            let date = format!("2025-01-0{}T00:00:00Z", i + 1);
            assert!(
                run_git(&["commit", "-q", "-m", &format!("commit {i}")], &date).success(),
                "commit {i} failed"
            );
        }
    }

    #[test]
    fn smoke_index_tiny_repo() {
        if Command::new("git").arg("--version").output().is_err() {
            eprintln!("skip: git not on PATH");
            return;
        }

        let td = TempDir::new().unwrap();
        seed_repo(td.path(), 3);

        let repo = repo::open(td.path()).expect("open repo");
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

        let commits: i64 = cache
            .conn
            .query_row("SELECT COUNT(*) FROM commits", [], |r| r.get(0))
            .unwrap();
        assert_eq!(commits, 3);

        // Parents populated for the two non-root commits.
        let pcount: i64 = cache
            .conn
            .query_row("SELECT COUNT(*) FROM commit_parents", [], |r| r.get(0))
            .unwrap();
        assert_eq!(pcount, 2);
    }
}
