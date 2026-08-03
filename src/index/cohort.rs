//! Per-file cohort fold. Reconstructs each file's line array from its hunk
//! history and emits one `line_births(path, line_no, birth_sha,
//! birth_bucket)` row per surviving line.
//!
//! Algorithm (SPEC §Indexing pipeline, Phase 2):
//!   1. Load every non-merge hunk joined with its commit's authored_at +
//!      bucket_key.
//!   2. Build a rename alias map (`prev_path` → `path`) so a file's
//!      history spans its rename chain.
//!   3. Bucket hunks by canonical (post-rename) path.
//!   4. Per path: replay hunks in commit order. Each commit's hunks are
//!      applied together via the standard `old_file → new_file` merge —
//!      copy pre-hunk region, skip `old_len` deleted lines, emit `new_len`
//!      new lines (marked with this commit's sha).
//!   5. INSERT the final Vec<(sha, bucket)> as `line_births`.
//!
//! v1 caveats (per SPEC "What we do NOT test"):
//! - Rename chain is a single hop per rewrite event; deeper chains drift.
//! - Merges are skipped, matching the rest of the indexer's non-merge
//!   sampling — cross-branch content only shows up when it lands via a
//!   non-merge commit.
//! - Serial. Rayon parallelism across files is a v1.x perf follow-up.

use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result};
use duckdb::params;
use rayon::prelude::*;

use crate::cache::Cache;

#[derive(Clone)]
struct HunkRow {
    sha: String,
    bucket_key: i64,
    path: String,
    prev_path: Option<String>,
    old_start: i32,
    old_len: i32,
    new_len: i32,
}

pub fn fold_and_materialize(cache: &mut Cache) -> Result<()> {
    let hunks = load_hunks(cache).context("loading hunks for cohort fold")?;
    if hunks.is_empty() {
        return Ok(());
    }

    // Alias map — later renames win. `resolve` chases until stable.
    let mut alias: HashMap<String, String> = HashMap::new();
    for h in &hunks {
        if let Some(pp) = &h.prev_path {
            if !pp.is_empty() && pp != &h.path {
                alias.insert(pp.clone(), h.path.clone());
            }
        }
    }

    // Group hunks by canonical (post-rename) path.
    let mut by_path: HashMap<String, Vec<HunkRow>> = HashMap::new();
    for h in hunks {
        let key = resolve(&alias, &h.path);
        by_path.entry(key).or_default().push(h);
    }

    // Rebuild line_births from scratch — Phase 3 always full-rebuild.
    // Incremental is v1.x.
    cache.conn.execute("DELETE FROM line_births", [])?;

    // Fold each file in parallel. Files are independent — the whole point
    // of splitting Phase 2 by canonical path (SPEC §Indexing pipeline).
    let folded: Vec<(String, Vec<(String, i64)>)> = by_path
        .into_par_iter()
        .map(|(path, hunks)| (path, fold_file(&hunks)))
        .collect();

    // Bulk-load via Appender — 800k rows on a ~2.5k-commit repo. Prepared
    // execute() per row was measurably the slowest step in the fold.
    {
        let mut app = cache
            .conn
            .appender("line_births")
            .context("line_births appender")?;
        for (path, state) in &folded {
            for (i, (sha, bucket)) in state.iter().enumerate() {
                app.append_row(params![path, (i as i32) + 1, sha, bucket])?;
            }
        }
        app.flush()?;
    }
    Ok(())
}

fn load_hunks(cache: &Cache) -> Result<Vec<HunkRow>> {
    let mut stmt = cache.conn.prepare(
        "SELECT h.sha, c.bucket_key,
                h.path, h.prev_path,
                h.old_start, h.old_len, h.new_len
         FROM   hunks h
         JOIN   commits c ON c.sha = h.sha
         WHERE  c.is_merge = FALSE
         ORDER  BY c.authored_at ASC, h.sha ASC, h.old_start ASC",
    )?;
    let iter = stmt.query_map([], |r| {
        Ok(HunkRow {
            sha: r.get(0)?,
            bucket_key: r.get(1)?,
            path: r.get(2)?,
            prev_path: r.get::<_, Option<String>>(3)?,
            old_start: r.get(4)?,
            old_len: r.get(5)?,
            new_len: r.get(6)?,
        })
    })?;
    Ok(iter.collect::<duckdb::Result<Vec<_>>>()?)
}

fn resolve(alias: &HashMap<String, String>, p: &str) -> String {
    let mut cur = p.to_string();
    let mut seen: HashSet<String> = HashSet::new();
    while let Some(nxt) = alias.get(&cur) {
        if !seen.insert(cur.clone()) {
            break; // cycle guard
        }
        cur = nxt.clone();
    }
    cur
}

/// Fold every hunk touching this canonical path into a Vec<(sha, bucket)>.
/// Groups by commit sha (preserving global commit order), then applies each
/// commit's hunks via `apply_commit`.
fn fold_file(hunks: &[HunkRow]) -> Vec<(String, i64)> {
    // Group consecutive same-sha hunks together while preserving the outer
    // commit order (already sorted by authored_at, sha).
    let mut groups: Vec<(String, i64, Vec<&HunkRow>)> = Vec::new();
    for h in hunks {
        if let Some((last_sha, _, group)) = groups.last_mut() {
            if last_sha == &h.sha {
                group.push(h);
                continue;
            }
        }
        groups.push((h.sha.clone(), h.bucket_key, vec![h]));
    }

    let mut state: Vec<(String, i64)> = Vec::new();
    for (sha, bucket, group) in groups {
        state = apply_commit(state, &sha, bucket, &group);
    }
    state
}

/// Apply one commit's hunks to the previous line array.
///
/// Standard merge algorithm: walk hunks sorted by `old_start` ascending,
/// copy pre-hunk regions verbatim, skip deleted spans, emit new_len lines
/// marked with the current sha.
fn apply_commit(
    prev: Vec<(String, i64)>,
    sha: &str,
    bucket: i64,
    hunks_group: &[&HunkRow],
) -> Vec<(String, i64)> {
    let mut sorted: Vec<&HunkRow> = hunks_group.to_vec();
    sorted.sort_by_key(|h| h.old_start);

    let mut new_state: Vec<(String, i64)> = Vec::with_capacity(prev.len());
    let mut old_idx: usize = 0;

    for h in sorted {
        // 1-based → 0-based; hunks with old_start=0 (pure addition) treat
        // as insertion at index 0.
        let hstart = if h.old_start <= 0 {
            0usize
        } else {
            (h.old_start as usize) - 1
        };

        // Copy unchanged region.
        while old_idx < hstart && old_idx < prev.len() {
            new_state.push(prev[old_idx].clone());
            old_idx += 1;
        }
        // Skip deleted lines.
        let del_end = (hstart + h.old_len.max(0) as usize).min(prev.len());
        if del_end > old_idx {
            old_idx = del_end;
        }
        // Emit new lines.
        for _ in 0..h.new_len.max(0) as usize {
            new_state.push((sha.to_string(), bucket));
        }
    }
    // Copy the tail.
    while old_idx < prev.len() {
        new_state.push(prev[old_idx].clone());
        old_idx += 1;
    }
    new_state
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mk(sha: &str, bucket: i64, path: &str, os: i32, ol: i32, _ns: i32, nl: i32) -> HunkRow {
        HunkRow {
            sha: sha.to_string(),
            bucket_key: bucket,
            path: path.to_string(),
            prev_path: None,
            old_start: os,
            old_len: ol,
            new_len: nl,
        }
    }

    #[test]
    fn addition_only_births_all_from_first_sha() {
        // Commit A: add 3 lines.
        let h = vec![mk("A", 100, "f", 0, 0, 1, 3)];
        let state = fold_file(&h);
        assert_eq!(state.len(), 3);
        for (sha, bucket) in &state {
            assert_eq!(sha, "A");
            assert_eq!(*bucket, 100);
        }
    }

    #[test]
    fn subsequent_modification_updates_birth_of_new_lines_only() {
        // A: add 4 lines. B: replace line 2 with 2 new lines.
        let hunks = vec![mk("A", 100, "f", 0, 0, 1, 4), mk("B", 200, "f", 2, 1, 2, 2)];
        let state = fold_file(&hunks);
        // Result: [A, B, B, A, A]  (line1=A, lines2-3=B, lines4-5=A originals 3+4)
        assert_eq!(state.len(), 5);
        assert_eq!(state[0].0, "A");
        assert_eq!(state[1].0, "B");
        assert_eq!(state[2].0, "B");
        assert_eq!(state[3].0, "A");
        assert_eq!(state[4].0, "A");
    }

    #[test]
    fn full_delete_leaves_empty_state() {
        let hunks = vec![mk("A", 100, "f", 0, 0, 1, 5), mk("B", 200, "f", 1, 5, 0, 0)];
        let state = fold_file(&hunks);
        assert!(state.is_empty());
    }

    #[test]
    fn rename_alias_collapses_history() {
        let alias: HashMap<String, String> = [("old.rs".to_string(), "new.rs".to_string())].into();
        assert_eq!(resolve(&alias, "old.rs"), "new.rs");
        assert_eq!(resolve(&alias, "new.rs"), "new.rs");
    }

    #[test]
    fn rename_chain_two_hops() {
        let alias: HashMap<String, String> = [
            ("a".to_string(), "b".to_string()),
            ("b".to_string(), "c".to_string()),
        ]
        .into();
        assert_eq!(resolve(&alias, "a"), "c");
    }

    #[test]
    fn rename_cycle_bails() {
        let alias: HashMap<String, String> = [
            ("a".to_string(), "b".to_string()),
            ("b".to_string(), "a".to_string()),
        ]
        .into();
        // Should not infinite-loop.
        let _ = resolve(&alias, "a");
    }

    #[test]
    fn multiple_hunks_same_commit_applied_left_to_right() {
        // Old file has 6 lines from A.
        // B modifies lines 2 and 5.
        let hunks = vec![
            mk("A", 100, "f", 0, 0, 1, 6),
            mk("B", 200, "f", 2, 1, 2, 1),
            mk("B", 200, "f", 5, 1, 5, 1),
        ];
        let state = fold_file(&hunks);
        // Result: [A, B, A, A, B, A]  (lines 2 and 5 replaced)
        assert_eq!(state.len(), 6);
        assert_eq!(state[0].0, "A");
        assert_eq!(state[1].0, "B");
        assert_eq!(state[2].0, "A");
        assert_eq!(state[3].0, "A");
        assert_eq!(state[4].0, "B");
        assert_eq!(state[5].0, "A");
    }
}
