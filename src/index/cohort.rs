//! Per-file cohort fold. Reconstructs each file's line array from its hunk
//! history and emits one `line_births(path, line_no, birth_sha,
//! birth_bucket)` row per surviving line.
//!
//! Algorithm (SPEC §Indexing pipeline, Phase 2):
//!   1. Load every non-merge hunk joined with its commit's authored_at +
//!      bucket_key.
//!   2. Walk commits in order, tracking file identity per live path: a
//!      rename moves the identity to the new path, a full delete retires
//!      it. A path reused after a rename/delete starts a fresh file.
//!   3. Bucket hunks by file identity; emit under its final path.
//!   4. Per path: replay hunks in commit order. Each commit's hunks are
//!      applied together via the standard `old_file → new_file` merge —
//!      copy pre-hunk region, skip `old_len` deleted lines, emit `new_len`
//!      new lines (marked with this commit's sha).
//!   5. INSERT the final Vec<(sha, bucket)> as `line_births`.
//!
//! v1 caveats (per SPEC "What we do NOT test"):
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
    new_start: i32,
    new_len: i32,
}

impl HunkRow {
    /// Whole-file deletion: the churn producer emits new_start = 0 only
    /// for deleted files (content hunks are 1-based).
    fn is_file_delete(&self) -> bool {
        self.prev_path.is_none() && self.new_start == 0 && self.new_len == 0 && self.old_len > 0
    }

    fn rename_source(&self) -> Option<&str> {
        self.prev_path
            .as_deref()
            .filter(|pp| !pp.is_empty() && *pp != self.path)
    }
}

/// `on_head`: shas reachable from HEAD. `line_births` describes HEAD's
/// files, so hunks from unmerged branches are excluded.
pub fn fold_and_materialize(cache: &mut Cache, on_head: &HashSet<String>) -> Result<()> {
    let mut hunks = load_hunks(cache).context("loading hunks for cohort fold")?;
    hunks.retain(|h| on_head.contains(&h.sha));
    let by_file = group_by_file(&hunks);

    // Rebuild line_births from scratch — Phase 3 always full-rebuild.
    // Incremental is v1.x.
    cache.conn.execute("DELETE FROM line_births", [])?;

    // Fold each file in parallel. Files are independent — the whole point
    // of splitting Phase 2 by canonical path (SPEC §Indexing pipeline).
    let folded: Vec<(String, Vec<(String, i64)>)> = by_file
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
                h.old_start, h.old_len, h.new_start, h.new_len
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
            new_start: r.get(6)?,
            new_len: r.get(7)?,
        })
    })?;
    Ok(iter.collect::<duckdb::Result<Vec<_>>>()?)
}

/// Group hunks (sorted by commit order) into files, tracking identity
/// chronologically so renames follow the file and reused paths don't
/// inherit a predecessor's history. Returns only files live at the end,
/// keyed by final path.
fn group_by_file(hunks: &[HunkRow]) -> Vec<(String, Vec<&HunkRow>)> {
    let mut live: HashMap<&str, usize> = HashMap::new();
    let mut files: Vec<(&str, Vec<&HunkRow>)> = Vec::new();
    fn new_file<'a>(files: &mut Vec<(&'a str, Vec<&'a HunkRow>)>, path: &'a str) -> usize {
        files.push((path, Vec::new()));
        files.len() - 1
    }

    for commit in hunks.chunk_by(|a, b| a.sha == b.sha) {
        // Detach every rename source before attaching any target, so a
        // swap (a→b, b→a) in one commit moves both identities correctly.
        let mut moved: Vec<(&str, Option<usize>)> = Vec::new();
        for h in commit {
            if let Some(src) = h.rename_source() {
                if !moved.iter().any(|(p, _)| *p == h.path) {
                    moved.push((&h.path, live.remove(src)));
                }
            }
        }
        for (path, id) in moved {
            let id = id.unwrap_or_else(|| new_file(&mut files, path));
            files[id].0 = path;
            live.insert(path, id);
        }
        for h in commit {
            let id = match live.get(h.path.as_str()) {
                Some(&id) => id,
                None => {
                    let id = new_file(&mut files, &h.path);
                    live.insert(&h.path, id);
                    id
                }
            };
            files[id].1.push(h);
        }
        for h in commit.iter().filter(|h| h.is_file_delete()) {
            live.remove(h.path.as_str());
        }
    }

    live.into_values()
        .map(|id| {
            let (path, hs) = std::mem::take(&mut files[id]);
            (path.to_string(), hs)
        })
        .collect()
}

/// Fold every hunk touching this canonical path into a Vec<(sha, bucket)>.
/// Groups by commit sha (preserving global commit order), then applies each
/// commit's hunks via `apply_commit`.
fn fold_file(hunks: &[&HunkRow]) -> Vec<(String, i64)> {
    // Group consecutive same-sha hunks together while preserving the outer
    // commit order (already sorted by authored_at, sha).
    let mut groups: Vec<(String, i64, Vec<&HunkRow>)> = Vec::new();
    for &h in hunks {
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

    fn mk(sha: &str, bucket: i64, path: &str, os: i32, ol: i32, ns: i32, nl: i32) -> HunkRow {
        HunkRow {
            sha: sha.to_string(),
            bucket_key: bucket,
            path: path.to_string(),
            prev_path: None,
            old_start: os,
            old_len: ol,
            new_start: ns,
            new_len: nl,
        }
    }

    fn renamed(mut h: HunkRow, from: &str) -> HunkRow {
        h.prev_path = Some(from.to_string());
        h
    }

    fn fold(hunks: &[HunkRow]) -> Vec<(String, i64)> {
        fold_file(&hunks.iter().collect::<Vec<_>>())
    }

    /// Final path → birth shas, sorted by path.
    fn files(hunks: &[HunkRow]) -> Vec<(String, Vec<String>)> {
        let mut out: Vec<_> = group_by_file(hunks)
            .into_iter()
            .map(|(p, hs)| (p, fold_file(&hs).into_iter().map(|(s, _)| s).collect()))
            .collect();
        out.sort();
        out
    }

    #[test]
    fn addition_only_births_all_from_first_sha() {
        // Commit A: add 3 lines.
        let h = vec![mk("A", 100, "f", 0, 0, 1, 3)];
        let state = fold(&h);
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
        let state = fold(&hunks);
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
        let state = fold(&hunks);
        assert!(state.is_empty());
    }

    #[test]
    fn rename_follows_history() {
        let h = vec![
            mk("A", 1, "old", 0, 0, 1, 2),
            renamed(mk("B", 2, "new", 0, 0, 0, 0), "old"),
            mk("C", 3, "new", 3, 0, 3, 1),
        ];
        assert_eq!(
            files(&h),
            vec![("new".into(), vec!["A".into(), "A".into(), "C".into()])]
        );
    }

    #[test]
    fn rename_chain_two_hops() {
        let h = vec![
            mk("A", 1, "a", 0, 0, 1, 1),
            renamed(mk("B", 2, "b", 0, 0, 0, 0), "a"),
            renamed(mk("C", 3, "c", 0, 0, 0, 0), "b"),
        ];
        assert_eq!(files(&h), vec![("c".into(), vec!["A".into()])]);
    }

    #[test]
    fn path_reused_after_rename_is_a_new_file() {
        // A: create f (2 lines). B: f → g. C: new f (1 line).
        let h = vec![
            mk("A", 1, "f", 0, 0, 1, 2),
            renamed(mk("B", 2, "g", 0, 0, 0, 0), "f"),
            mk("C", 3, "f", 0, 0, 1, 1),
        ];
        assert_eq!(
            files(&h),
            vec![
                ("f".into(), vec!["C".into()]),
                ("g".into(), vec!["A".into(), "A".into()]),
            ]
        );
    }

    #[test]
    fn rename_and_recreate_in_same_commit() {
        // B moves f → g and adds a fresh f in one commit.
        let h = vec![
            mk("A", 1, "f", 0, 0, 1, 1),
            mk("B", 2, "f", 0, 0, 1, 3),
            renamed(mk("B", 2, "g", 0, 0, 0, 0), "f"),
        ];
        assert_eq!(
            files(&h),
            vec![
                ("f".into(), vec!["B".into(), "B".into(), "B".into()]),
                ("g".into(), vec!["A".into()]),
            ]
        );
    }

    #[test]
    fn swap_in_one_commit() {
        let h = vec![
            mk("A", 1, "a", 0, 0, 1, 1),
            mk("A", 1, "b", 0, 0, 1, 2),
            renamed(mk("B", 2, "a", 0, 0, 0, 0), "b"),
            renamed(mk("B", 2, "b", 0, 0, 0, 0), "a"),
        ];
        assert_eq!(
            files(&h),
            vec![
                ("a".into(), vec!["A".into(), "A".into()]),
                ("b".into(), vec!["A".into()]),
            ]
        );
    }

    #[test]
    fn deleted_file_is_dropped() {
        let h = vec![mk("A", 1, "f", 0, 0, 1, 2), mk("B", 2, "f", 1, 2, 0, 0)];
        assert!(files(&h).is_empty());
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
        let state = fold(&hunks);
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
