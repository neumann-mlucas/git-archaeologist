//! Per-commit diff producer. Yields per-file numstat (`file_churn`) AND
//! per-hunk line ranges (`hunks`) in a single tree-walk. Rename detection
//! is on — renamed files carry `prev_path` and their added/deleted lines
//! are computed from the actual content diff (not treated as add + delete).
//!
//! Binary blobs, symlinks, and submodules are skipped in both outputs.

use std::collections::HashMap;
use std::ops::Range;
use std::path::PathBuf;

use anyhow::{Context, Result};
use gix::bstr::ByteSlice;
use gix::diff::Rewrites;
use imara_diff::intern::InternedInput;
use imara_diff::{Algorithm, Sink};
use rayon::prelude::*;

use crate::index::treesitter::LangRegistry;
use crate::repo::Repo;

#[derive(Debug, Clone)]
pub struct FileChurn {
    pub path: String,
    pub added: u32,
    pub deleted: u32,
    /// Registry label by extension (SPEC: extension-only detection);
    /// `None` for files no grammar claims.
    pub language: Option<&'static str>,
}

#[derive(Debug, Clone)]
pub struct Hunk {
    pub path: String,
    /// Non-`None` only for rewrites (renames + rename-with-content-change).
    pub prev_path: Option<String>,
    /// 1-based line in the old file. Addition rows use 0.
    pub old_start: u32,
    pub old_len: u32,
    /// 1-based line in the new file. Deletion rows use 0.
    pub new_start: u32,
    pub new_len: u32,
}

#[derive(Debug, Clone, Default)]
pub struct CommitDiff {
    pub churn: Vec<FileChurn>,
    pub hunks: Vec<Hunk>,
}

const MAX_BLOB_BYTES: usize = 2 * 1024 * 1024;

/// Run the diff over a slice of jobs. Each rayon worker opens its own
/// `gix::Repository` via `map_init` — gix::Repository is `!Sync` but
/// re-opening the same `.git/` from N threads is safe.
///
/// Commits whose diff fails come back in the error list (sha, message)
/// instead of silently yielding empty churn.
pub fn process_jobs(
    repo: &Repo,
    jobs: &[(String, Option<gix::ObjectId>)],
) -> (HashMap<String, CommitDiff>, Vec<(String, String)>) {
    let git_dir: PathBuf = repo.git.path().to_path_buf();
    let results: Vec<(String, Result<CommitDiff>)> = jobs
        .par_iter()
        .map_init(
            || {
                (
                    gix::open(&git_dir).expect("open per-worker repo"),
                    LangRegistry::new(),
                )
            },
            |(worker_git, reg), (sha, parent_id)| {
                let diff = diff_one(worker_git, sha, parent_id.as_ref()).map(|mut d| {
                    for c in &mut d.churn {
                        c.language = reg.label_for(&c.path);
                    }
                    d
                });
                (sha.clone(), diff)
            },
        )
        .collect();
    let mut ok = HashMap::with_capacity(results.len());
    let mut failed = Vec::new();
    for (sha, r) in results {
        match r {
            Ok(d) => {
                ok.insert(sha, d);
            }
            Err(e) => failed.push((sha, format!("{e:#}"))),
        }
    }
    (ok, failed)
}

/// Compute the (hunks, churn) delta for a single commit against its
/// first parent. Blob reads reuse the worker-local `gix::Repository`.
fn diff_one(
    git: &gix::Repository,
    sha: &str,
    parent_id: Option<&gix::ObjectId>,
) -> Result<CommitDiff> {
    let mut diff = CommitDiff::default();

    let oid = gix::ObjectId::from_hex(sha.as_bytes()).context("parsing sha")?;
    let tree = git
        .find_commit(oid)
        .context("loading commit")?
        .tree()
        .context("loading tree")?;
    // A parent missing from the odb (shallow boundary) diffs against the
    // empty tree, i.e. the commit is treated as a root — startup already
    // warned about shallow history.
    let empty_tree = git.empty_tree();
    let parent_tree_obj = parent_id
        .and_then(|pid| git.find_commit(*pid).ok())
        .and_then(|pc| pc.tree().ok());
    let parent_tree_ref = parent_tree_obj.as_ref().unwrap_or(&empty_tree);

    let mut platform = parent_tree_ref.changes().context("starting tree diff")?;
    platform
        .track_path()
        .track_rewrites(Some(Rewrites::default()));

    platform
        .for_each_to_obtain_tree(
            &tree,
            |change| -> Result<
                gix::object::tree::diff::Action,
                Box<dyn std::error::Error + Send + Sync>,
            > {
                handle_change(git, change, &mut diff);
                Ok(gix::object::tree::diff::Action::Continue)
            },
        )
        .context("diffing trees")?;

    Ok(diff)
}

fn handle_change<'a>(
    git: &gix::Repository,
    change: gix::object::tree::diff::Change<'a, '_, '_>,
    out: &mut CommitDiff,
) {
    use gix::object::tree::diff::change::Event;

    let path = change.location.to_str_lossy().into_owned();

    match change.event {
        Event::Addition { entry_mode, id } => {
            if !entry_mode.is_blob() {
                return;
            }
            let bytes = match load_text_blob(git, id.detach()) {
                Some(b) => b,
                None => return,
            };
            let lines = count_lines(&bytes);
            if lines > 0 {
                out.hunks.push(Hunk {
                    path: path.clone(),
                    prev_path: None,
                    old_start: 0,
                    old_len: 0,
                    new_start: 1,
                    new_len: lines,
                });
            }
            out.churn.push(FileChurn {
                path,
                added: lines,
                deleted: 0,
                language: None,
            });
        }
        Event::Deletion { entry_mode, id } => {
            if !entry_mode.is_blob() {
                return;
            }
            let bytes = match load_text_blob(git, id.detach()) {
                Some(b) => b,
                None => return,
            };
            let lines = count_lines(&bytes);
            if lines > 0 {
                out.hunks.push(Hunk {
                    path: path.clone(),
                    prev_path: None,
                    old_start: 1,
                    old_len: lines,
                    new_start: 0,
                    new_len: 0,
                });
            }
            out.churn.push(FileChurn {
                path,
                added: 0,
                deleted: lines,
                language: None,
            });
        }
        Event::Modification {
            entry_mode,
            previous_entry_mode,
            id,
            previous_id,
        } => {
            let Some((old, new)) = text_pair(
                git,
                (previous_entry_mode, previous_id.detach()),
                (entry_mode, id.detach()),
            ) else {
                return;
            };
            let (hunks, added, deleted) = diff_bytes(&old, &new);
            for h in hunks {
                out.hunks.push(Hunk {
                    path: path.clone(),
                    prev_path: None,
                    old_start: h.old_start,
                    old_len: h.old_len,
                    new_start: h.new_start,
                    new_len: h.new_len,
                });
            }
            out.churn.push(FileChurn {
                path,
                added,
                deleted,
                language: None,
            });
        }
        Event::Rewrite {
            source_location,
            source_entry_mode,
            source_id,
            entry_mode,
            id,
            ..
        } => {
            let prev_path = source_location.to_str_lossy().into_owned();
            let Some((old, new)) = text_pair(
                git,
                (source_entry_mode, source_id.detach()),
                (entry_mode, id.detach()),
            ) else {
                return;
            };
            let (hunks, added, deleted) = diff_bytes(&old, &new);
            if hunks.is_empty() {
                // Pure rename, no content delta — emit a zero-len hunk to
                // capture the rename edge for cohort's rename-follow chain.
                out.hunks.push(Hunk {
                    path: path.clone(),
                    prev_path: Some(prev_path.clone()),
                    old_start: 0,
                    old_len: 0,
                    new_start: 0,
                    new_len: 0,
                });
            } else {
                for h in hunks {
                    out.hunks.push(Hunk {
                        path: path.clone(),
                        prev_path: Some(prev_path.clone()),
                        old_start: h.old_start,
                        old_len: h.old_len,
                        new_start: h.new_start,
                        new_len: h.new_len,
                    });
                }
            }
            out.churn.push(FileChurn {
                path,
                added,
                deleted,
                language: None,
            });
        }
    }
}

/// Both sides of a change as text. A side that isn't text (binary, too
/// big, symlink, submodule) has no tracked lines, so it reads as empty:
/// going text → binary deletes the file's lines, binary → text adds them.
/// `None` when neither side is text.
fn text_pair(
    git: &gix::Repository,
    old: (gix::object::tree::EntryMode, gix::ObjectId),
    new: (gix::object::tree::EntryMode, gix::ObjectId),
) -> Option<(Vec<u8>, Vec<u8>)> {
    let text = |(mode, id): (gix::object::tree::EntryMode, gix::ObjectId)| {
        mode.is_blob().then(|| load_text_blob(git, id)).flatten()
    };
    match (text(old), text(new)) {
        (None, None) => None,
        (o, n) => Some((o.unwrap_or_default(), n.unwrap_or_default())),
    }
}

fn load_text_blob(git: &gix::Repository, oid: gix::ObjectId) -> Option<Vec<u8>> {
    let blob = git.find_blob(oid).ok()?;
    if blob.data.len() > MAX_BLOB_BYTES {
        return None;
    }
    // Fast binary reject: NUL in the first 8KiB → skip.
    let head = &blob.data[..blob.data.len().min(8192)];
    if head.contains(&0u8) {
        return None;
    }
    Some(blob.data.to_vec())
}

fn count_lines(bytes: &[u8]) -> u32 {
    if bytes.is_empty() {
        return 0;
    }
    let mut n: u32 = 0;
    for &b in bytes {
        if b == b'\n' {
            n += 1;
        }
    }
    // Trailing partial line without a final newline.
    if bytes.last() != Some(&b'\n') {
        n += 1;
    }
    n
}

struct HunkRange {
    old_start: u32,
    old_len: u32,
    new_start: u32,
    new_len: u32,
}

struct HunkSink(Vec<HunkRange>);
impl Sink for HunkSink {
    type Out = Vec<HunkRange>;
    fn process_change(&mut self, before: Range<u32>, after: Range<u32>) {
        self.0.push(HunkRange {
            // imara-diff returns 0-based ranges. Convert to 1-based; a
            // pure-insertion hunk has before.start == before.end and
            // old_len == 0, so old_start stays a valid anchor line.
            old_start: before.start + 1,
            old_len: before.end - before.start,
            new_start: after.start + 1,
            new_len: after.end - after.start,
        });
    }
    fn finish(self) -> Self::Out {
        self.0
    }
}

fn diff_bytes(old: &[u8], new: &[u8]) -> (Vec<HunkRange>, u32, u32) {
    let old_str = String::from_utf8_lossy(old);
    let new_str = String::from_utf8_lossy(new);
    let input = InternedInput::new(old_str.as_ref(), new_str.as_ref());
    let hunks = imara_diff::diff(Algorithm::Histogram, &input, HunkSink(Vec::new()));
    let mut added = 0u32;
    let mut deleted = 0u32;
    for h in &hunks {
        added += h.new_len;
        deleted += h.old_len;
    }
    (hunks, added, deleted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn count_lines_basic() {
        assert_eq!(count_lines(b""), 0);
        assert_eq!(count_lines(b"a\n"), 1);
        assert_eq!(count_lines(b"a\nb\nc\n"), 3);
        assert_eq!(count_lines(b"a\nb\nc"), 3); // no trailing newline
    }

    #[test]
    fn diff_addition_only() {
        let (hunks, added, deleted) = diff_bytes(b"", b"a\nb\n");
        assert_eq!(added, 2);
        assert_eq!(deleted, 0);
        assert_eq!(hunks.len(), 1);
        assert_eq!(hunks[0].old_len, 0);
        assert_eq!(hunks[0].new_len, 2);
    }

    #[test]
    fn diff_modification() {
        let (hunks, added, deleted) = diff_bytes(b"a\nb\nc\n", b"a\nX\nc\n");
        assert_eq!(added, 1);
        assert_eq!(deleted, 1);
        assert_eq!(hunks.len(), 1);
        // 1-based line 2 replaced.
        assert_eq!(hunks[0].old_start, 2);
        assert_eq!(hunks[0].new_start, 2);
    }

    #[test]
    fn diff_pure_delete() {
        let (hunks, added, deleted) = diff_bytes(b"a\nb\nc\n", b"a\nc\n");
        assert_eq!(added, 0);
        assert_eq!(deleted, 1);
        assert_eq!(hunks.len(), 1);
    }
}
