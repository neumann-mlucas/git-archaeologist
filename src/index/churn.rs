use std::collections::HashMap;

use anyhow::{Context, Result};
use gix::bstr::ByteSlice;

use crate::repo::Repo;

#[derive(Debug, Clone)]
pub struct FileChurn {
    pub path: String,
    pub added: u32,
    pub deleted: u32,
}

/// Compute per-file adds/dels for every non-merge commit reachable from HEAD.
///
/// Runs entirely in-process via `gix` — replaces the previous `git log
/// --numstat` subprocess. Benefits:
///   - No dependency on `git` being on `$PATH`.
///   - No fork/exec overhead per invocation.
///   - Diff walks the same reachable set as `index::walker`, so churn rows
///     line up with commit rows deterministically (the subprocess version
///     was fragile around merge/first-parent semantics).
///
/// Rename tracking is deliberately disabled — matching the prior
/// `--no-renames` flag — so a moved file registers as an add + delete.
/// Turning it back on lives in the v1.1 backlog.
pub fn batch_all(repo: &Repo) -> Result<HashMap<String, Vec<FileChurn>>> {
    let mut out: HashMap<String, Vec<FileChurn>> = HashMap::new();

    // Resource cache backs the line-diff engine — reused across every
    // change to avoid re-reading blob data + re-checking attributes.
    let mut cache = repo
        .git
        .diff_resource_cache_for_tree_diff()
        .context("creating blob-diff resource cache")?;

    let head_id = repo.git.head_id().context("resolving HEAD id")?;
    let walk = repo
        .git
        .rev_walk([head_id])
        .sorting(gix::traverse::commit::simple::Sorting::ByCommitTimeNewestFirst)
        .all()
        .context("starting rev walk for churn")?;

    let empty_tree = repo.git.empty_tree();

    for info in walk {
        let info = info.context("walking commit for churn")?;
        let commit = info.object().context("loading commit object")?;

        let parents: Vec<_> = commit.parent_ids().collect();
        // Skip merges to line up with walker::walk(skip_merges=true).
        if parents.len() > 1 {
            continue;
        }

        let sha = commit.id().to_string();
        let tree = commit.tree().context("loading commit tree")?;

        // Root commit: diff against the empty tree so every file counts as
        // Addition.
        let parent_tree_obj = parents
            .first()
            .and_then(|pid| repo.git.find_commit(*pid).ok())
            .and_then(|pc| pc.tree().ok());
        let parent_tree_ref = parent_tree_obj.as_ref().unwrap_or(&empty_tree);

        let mut churn: Vec<FileChurn> = Vec::new();
        let mut platform = parent_tree_ref
            .changes()
            .context("initializing tree-diff platform")?;
        platform.track_path().track_rewrites(None);

        platform
            .for_each_to_obtain_tree(
                &tree,
                |change| -> Result<
                    gix::object::tree::diff::Action,
                    Box<dyn std::error::Error + Send + Sync>,
                > {
                    let path = change.location.to_str_lossy().into_owned();
                    // .diff().line_counts() returns Ok(None) for binary blobs,
                    // which is the honest zero-churn signal for those paths.
                    if let Ok(mut blob_platform) = change.diff(&mut cache) {
                        if let Ok(Some(counts)) = blob_platform.line_counts() {
                            churn.push(FileChurn {
                                path,
                                added: counts.insertions,
                                deleted: counts.removals,
                            });
                        }
                    }
                    Ok(gix::object::tree::diff::Action::Continue)
                },
            )
            .context("iterating tree diff")?;

        // The resource cache is unbounded by design; wipe between commits so
        // long histories don't blow up memory.
        cache.clear_resource_cache();

        out.insert(sha, churn);
    }

    Ok(out)
}
