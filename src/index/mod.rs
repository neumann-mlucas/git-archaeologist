pub mod bucket;
pub mod churn;
pub mod cohort;
pub mod mailmap;
pub mod parse;
pub mod treesitter;
pub mod walker;

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use duckdb::params;

use crate::cache::{queries, Cache};
use crate::index::bucket::BucketSize;
use crate::index::mailmap::AuthorResolver;
use crate::index::walker::CommitInfo;
use crate::repo::Repo;

pub struct IndexOptions {
    pub force_full: bool,
    pub bucket_override: Option<BucketSize>,
}

pub fn run(repo: &Repo, cache: &mut Cache, opts: IndexOptions) -> Result<()> {
    let ignore_revs = parse::load_ignore_revs(&repo.root);

    // Before the walk: a HEAD read later can be a commit the walk never saw
    // (concurrent pull), which would empty the fold.
    let head = repo.git.head_id().ok().map(|id| id.to_string());
    let all_commits = walker::walk(repo, &ignore_revs)?;
    if all_commits.is_empty() {
        return Ok(());
    }
    let head = head.context("resolving HEAD")?;

    let mut size = opts
        .bucket_override
        .unwrap_or_else(|| bucket::auto(all_commits.len()));

    // Tag-bucket collection is done ahead of assign so we can inject dates
    // even when the override didn't ask for `Tag`. If the user asked for
    // `Tag` but no reachable tags exist, degrade to auto so downstream
    // metrics still work.
    // One tag walk feeds both bucketing and the `tags` table. A broken ref
    // store shouldn't block indexing, but it must not pass silently.
    let tag_infos = walker::tags(repo).unwrap_or_else(|e| {
        eprintln!("warning: reading tags failed, indexing without tags: {e:#}");
        Vec::new()
    });
    let mut tag_dates: Vec<i64> = tag_infos.iter().map(|t| t.tagged_at).collect();
    tag_dates.sort_unstable();
    if size == BucketSize::Tag && tag_dates.is_empty() {
        eprintln!("bucket=tag requested but repo has no tags; falling back to auto");
        size = bucket::auto(all_commits.len());
    }

    if opts.force_full {
        wipe_data(cache)?;
    }

    let stored = stored_plans(cache)?;
    let already: HashSet<String> = stored.keys().cloned().collect();

    queries::set_meta(&cache.conn, "bucket_size", &format!("{size:?}"))?;

    let assignments = assign_buckets(&all_commits, size, &tag_dates);

    // Incremental runs: a new commit can take over an indexed commit's
    // "last in bucket" slot, and `--bucket` can change between runs. Old
    // rows whose stored plan differs get re-keyed; demoted samples lose
    // their snapshot, promoted ones are parsed below.
    let mut stale: Vec<(&str, i64, bool)> = Vec::new();
    let mut promote: HashSet<String> = HashSet::new();
    if !opts.force_full {
        for (c, plan) in all_commits.iter().zip(&assignments) {
            if let Some(&(key, sampled)) = stored.get(&c.sha) {
                if (key, sampled) != (plan.bucket_key, plan.is_sampled) {
                    stale.push((&c.sha, plan.bucket_key, plan.is_sampled));
                    if plan.is_sampled && !sampled {
                        promote.insert(c.sha.clone());
                    }
                }
            }
        }
    }

    // Rewritten history: stored commits no ref reaches anymore.
    let live: HashSet<&str> = all_commits.iter().map(|c| c.sha.as_str()).collect();
    let orphans: Vec<&String> = already
        .iter()
        .filter(|s| !live.contains(s.as_str()))
        .collect();

    let total = all_commits.len();
    let sampled_count = assignments.iter().filter(|a| a.is_sampled).count();
    // Direct stderr line every 100ms + at start/end — only visible signal
    // that `git-arch index` is doing work.
    eprintln!("indexing {total} commits ({sampled_count} sampled)…");
    let start_at = std::time::Instant::now();

    let cfg = crate::config::load().context("loading user config for aliases")?;
    let mut resolver = AuthorResolver::new(repo, &cfg.aliases);

    // Chunked churn + treesitter: keeps peak RSS bounded by one chunk's
    // worth of diffs + snapshots rather than the entire repo's. The
    // chunk size trades peak RSS for tree-sitter dedup rate — the
    // unique-blob cache is per-chunk. For ratatui the two are already
    // in tension; the value below is a middle-ground default.
    const INDEX_CHUNK: usize = 1000;

    // Appender flush happens once per chunk boundary (below), not per
    // commit. Each flush syncs 7 tables to disk; batching them at the
    // chunk edge amortizes the round-trips against INDEX_CHUNK rows.
    // Peak Appender buffer = one chunk's worth, already the RSS bound.

    let mut diff_failures: Vec<(String, String)> = Vec::new();
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
    if !orphans.is_empty() {
        cache
            .conn
            .execute_batch("CREATE OR REPLACE TEMP TABLE orphan (sha TEXT)")?;
        let mut app = cache.conn.appender("orphan")?;
        for sha in &orphans {
            app.append_row(params![sha])?;
        }
        drop(app);
        cache.conn.execute_batch(
            "DELETE FROM funcs           WHERE sha IN (SELECT sha FROM orphan);
             DELETE FROM file_stats      WHERE sha IN (SELECT sha FROM orphan);
             DELETE FROM file_churn      WHERE sha IN (SELECT sha FROM orphan);
             DELETE FROM hunks           WHERE sha IN (SELECT sha FROM orphan);
             DELETE FROM tags            WHERE sha IN (SELECT sha FROM orphan);
             DELETE FROM commit_trailers WHERE sha IN (SELECT sha FROM orphan);
             DELETE FROM commit_parents  WHERE sha IN (SELECT sha FROM orphan);
             DELETE FROM commits         WHERE sha IN (SELECT sha FROM orphan);
             DROP TABLE orphan;",
        )?;
    }
    if !stale.is_empty() {
        cache.conn.execute_batch(
            "CREATE OR REPLACE TEMP TABLE plan_fix (sha TEXT, bucket_key BIGINT, is_sampled BOOLEAN)",
        )?;
        let mut app = cache.conn.appender("plan_fix")?;
        for (sha, key, sampled) in &stale {
            app.append_row(params![sha, key, sampled])?;
        }
        drop(app);
        cache.conn.execute_batch(
            "DELETE FROM file_stats WHERE sha IN (SELECT sha FROM plan_fix WHERE NOT is_sampled);
             DELETE FROM funcs      WHERE sha IN (SELECT sha FROM plan_fix WHERE NOT is_sampled);
             UPDATE commits SET bucket_key = p.bucket_key, is_sampled = p.is_sampled
             FROM plan_fix p WHERE commits.sha = p.sha;
             DROP TABLE plan_fix;",
        )?;
    }

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

    // Outer chunk: pre-compute churn + treesitter snapshots for a
    // window of commits, then run the insert loop over that window,
    // then drop the window's data before moving on. Peak RSS is now
    // ~one window instead of the whole repo.
    let commit_pairs: Vec<_> = all_commits
        .iter()
        .zip(assignments.iter())
        .enumerate()
        .collect();
    for window in commit_pairs.chunks(INDEX_CHUNK) {
        let pending = |c: &CommitInfo| opts.force_full || !already.contains(&c.sha);
        if !window
            .iter()
            .any(|(_, (c, _))| pending(c) || promote.contains(&c.sha))
        {
            continue;
        }

        // Diff every pending non-merge commit against its first parent
        // (SPEC §Indexing pipeline step 5) — same commit set as `commits`.
        let chunk_jobs: Vec<_> = window
            .iter()
            .filter(|(_, (c, _))| !c.is_merge && pending(c))
            .map(|(_, (c, _))| {
                (
                    c.sha.clone(),
                    c.parents.first().and_then(|p| p.parse().ok()),
                )
            })
            .collect();
        let (mut churn_map, failed) = churn::process_jobs(repo, &chunk_jobs);
        diff_failures.extend(failed);
        let sampled_shas: Vec<String> = window
            .iter()
            .filter_map(|(_, (commit, plan))| {
                if plan.is_sampled && (pending(commit) || promote.contains(&commit.sha)) {
                    Some(commit.sha.clone())
                } else {
                    None
                }
            })
            .collect();
        let mut snapshot_map = treesitter::batch_sampled(repo, &sampled_shas);

        for (i, (commit, plan)) in window {
            if !pending(commit) {
                // Already indexed; only a promoted sample needs rows.
                if let Some(files) = snapshot_map.remove(&commit.sha) {
                    append_snapshot(&mut file_stats_app, &mut funcs_app, &commit.sha, files)?;
                }
                continue;
            }

            let author_id =
                resolver.resolve(&cache.conn, &commit.author_name, &commit.author_email)?;
            let committer_id =
                resolver.resolve(&cache.conn, &commit.committer_name, &commit.committer_email)?;

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
                commit.message,
            ])?;

            for (idx, parent) in commit.parents.iter().enumerate() {
                parents_app.append_row(params![commit.sha, parent, idx as i32])?;
            }

            for tr in &commit.trailers {
                let tid = resolver.resolve(&cache.conn, &tr.ident.name, &tr.ident.email)?;
                trailers_app.append_row(params![commit.sha, tid, tr.role.as_str()])?;
            }

            if let Some(diff) = churn_map.remove(&commit.sha) {
                for c in diff.churn {
                    churn_app
                        .append_row(params![commit.sha, c.path, c.added, c.deleted, c.language])?;
                }
                for h in diff.hunks {
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
                if let Some(files) = snapshot_map.remove(&commit.sha) {
                    append_snapshot(&mut file_stats_app, &mut funcs_app, &commit.sha, files)?;
                }
            }

            let done = *i + 1;
            let now = std::time::Instant::now();
            if done == total || now.duration_since(last_progress_at) >= progress_interval {
                let elapsed = now.duration_since(start_at).as_secs_f64();
                let rate = if elapsed > 0.0 {
                    done as f64 / elapsed
                } else {
                    0.0
                };
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
            }
        }
        // Chunk-boundary flush — one sync per chunk instead of per commit.
        commits_app.flush()?;
        parents_app.flush()?;
        trailers_app.flush()?;
        hunks_app.flush()?;
        churn_app.flush()?;
        file_stats_app.flush()?;
        funcs_app.flush()?;
        // churn_map + snapshot_map drop here → peak RSS resets between
        // windows.
    }
    eprintln!();
    if let Some((sha, err)) = diff_failures.first() {
        eprintln!(
            "warning: {} commit(s) failed to diff and have no churn/hunks (first: {sha}: {err})",
            diff_failures.len()
        );
    }

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

    queries::set_indexed_head(&cache.conn, &head)?;

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
    let chain = first_parent_chain(&all_commits, &head);
    cohort::fold_and_materialize(repo, cache, &chain).context("cohort fold")?;

    // Recreate line_births indexes after the fold's bulk insert.
    cache.conn.execute_batch(
        "CREATE INDEX IF NOT EXISTS idx_line_births_bucket ON line_births(birth_bucket);
         CREATE INDEX IF NOT EXISTS idx_line_births_path   ON line_births(path);",
    )?;

    Ok(())
}

/// HEAD's first-parent chain, oldest first. Stops at a parent outside
/// `commits` (shallow boundary).
fn first_parent_chain<'a>(commits: &'a [CommitInfo], head: &str) -> Vec<&'a CommitInfo> {
    let by_sha: HashMap<&str, &CommitInfo> = commits.iter().map(|c| (c.sha.as_str(), c)).collect();
    let mut chain = Vec::new();
    let mut cur = by_sha.get(head).copied();
    while let Some(c) = cur {
        chain.push(c);
        cur = c
            .parents
            .first()
            .and_then(|p| by_sha.get(p.as_str()).copied());
    }
    chain.reverse();
    chain
}

fn wipe_data(cache: &Cache) -> Result<()> {
    cache.conn.execute_batch(
        "DELETE FROM line_births;
         DELETE FROM chain_births;
         DELETE FROM funcs;
         DELETE FROM file_stats;
         DELETE FROM file_churn;
         DELETE FROM hunks;
         DELETE FROM tags;
         DELETE FROM commit_trailers;
         DELETE FROM commit_parents;
         DELETE FROM commits;
         DELETE FROM author_aliases;
         DELETE FROM authors;",
    )?;
    Ok(())
}

/// Parse HEAD's tree live into temp tables `head_stats(path, language,
/// code, comments, blanks)` and `head_funcs(path, start_line, end_line)`
/// for `stats`. Sampled snapshots can trail HEAD, so they aren't reused.
/// Warns when `line_births` (age/owner) was folded at a different HEAD.
pub fn snapshot_head(repo: &Repo, cache: &Cache) -> Result<()> {
    let head = repo.git.head_id().context("resolving HEAD")?.to_string();
    let indexed: Option<String> = cache
        .conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'indexed_head_sha'",
            [],
            |r| r.get(0),
        )
        .ok();
    if indexed.as_deref() != Some(head.as_str()) {
        eprintln!(
            "warning: cache indexed at a different HEAD; age/owner/churn are stale — run `index`"
        );
    }
    snapshot(repo, cache, Some(&head), "head")
}

/// Resolve a revspec (`HEAD~3`, branch, tag, sha) to a commit sha.
pub fn resolve_rev(repo: &Repo, spec: &str) -> Result<String> {
    Ok(repo
        .git
        .rev_parse_single(spec)
        .with_context(|| format!("resolving {spec:?}"))?
        .object()?
        .peel_to_kind(gix::object::Kind::Commit)
        .with_context(|| format!("{spec:?} is not a commit"))?
        .id
        .to_string())
}

/// Parse `sha`'s tree into temp tables `{name}_stats` / `{name}_funcs`.
/// `None` leaves them empty. Grammar-backed files only.
pub fn snapshot(repo: &Repo, cache: &Cache, sha: Option<&str>, name: &str) -> Result<()> {
    cache.conn.execute_batch(&format!(
        "CREATE OR REPLACE TEMP TABLE {name}_stats (path TEXT, language TEXT, code INTEGER, comments INTEGER, blanks INTEGER, test_code INTEGER);
         CREATE OR REPLACE TEMP TABLE {name}_funcs (path TEXT, start_line INTEGER, end_line INTEGER);",
    ))?;
    let Some(sha) = sha else { return Ok(()) };
    let files = treesitter::batch_sampled(repo, &[sha.to_string()])
        .remove(sha)
        .unwrap_or_default();
    let mut stats = cache.conn.appender(&format!("{name}_stats"))?;
    let mut funcs = cache.conn.appender(&format!("{name}_funcs"))?;
    for f in files {
        let s = &f.stat;
        stats.append_row(params![
            s.path,
            s.language,
            s.code,
            s.comments,
            s.blanks,
            s.test_code
        ])?;
        for d in f.funcs {
            funcs.append_row(params![s.path, d.start_line, d.end_line])?;
        }
    }
    stats.flush()?;
    funcs.flush()?;
    Ok(())
}

/// sha → (bucket_key, is_sampled) for every indexed commit.
fn stored_plans(cache: &Cache) -> Result<HashMap<String, (i64, bool)>> {
    let mut stmt = cache
        .conn
        .prepare("SELECT sha, bucket_key, is_sampled FROM commits")?;
    let iter = stmt.query_map([], |r| Ok((r.get(0)?, (r.get(1)?, r.get(2)?))))?;
    Ok(iter.collect::<duckdb::Result<_>>()?)
}

fn append_snapshot(
    file_stats_app: &mut duckdb::Appender,
    funcs_app: &mut duckdb::Appender,
    sha: &str,
    files: Vec<treesitter::FileSnapshot>,
) -> Result<()> {
    for f in files {
        file_stats_app.append_row(params![
            sha,
            f.stat.path,
            f.stat.language,
            f.stat.code,
            f.stat.comments,
            f.stat.blanks,
            f.stat.test_code
        ])?;
        for def in f.funcs {
            funcs_app.append_row(params![
                sha,
                f.stat.path,
                def.name,
                def.kind,
                def.start_line,
                def.end_line,
                def.body_hash as i64,
            ])?;
        }
    }
    Ok(())
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
