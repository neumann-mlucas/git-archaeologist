use std::path::Path;

use anyhow::{Context, Result};
use tokei::{Config, LanguageType};

use crate::repo::Repo;

#[derive(Debug, Clone)]
pub struct FileStat {
    pub path: String,
    pub language: String,
    pub code: u32,
    pub comments: u32,
    pub blanks: u32,
}

/// Snapshot LOC per file at `sha` — walks the tree in git object DB, feeds
/// blobs to tokei parsers. No worktree checkout.
pub fn snapshot(repo: &Repo, sha: &str) -> Result<Vec<FileStat>> {
    let oid: gix::ObjectId = sha.parse().context("parsing commit sha")?;
    let commit = repo
        .git
        .find_object(oid)
        .context("finding commit object")?
        .try_into_commit()
        .context("expected commit object")?;

    let tree = commit.tree().context("resolving commit tree")?;
    let entries = tree
        .traverse()
        .breadthfirst
        .files()
        .context("walking tree entries")?;

    let cfg = Config::default();
    let mut out = Vec::with_capacity(entries.len());

    for entry in entries {
        if !entry.mode.is_blob() {
            continue;
        }
        let path_str = entry.filepath.to_string();
        let lang = match LanguageType::from_path(Path::new(&path_str), &cfg) {
            Some(l) => l,
            None => continue,
        };

        let blob = match repo.git.find_blob(entry.oid) {
            Ok(b) => b,
            Err(_) => continue, // missing / unreachable; skip rather than fail
        };

        let stats = lang.parse_from_slice(&blob.data, &cfg);

        out.push(FileStat {
            path: path_str,
            language: format!("{lang:?}"),
            code: stats.code as u32,
            comments: stats.comments as u32,
            blanks: stats.blanks as u32,
        });
    }

    Ok(out)
}
