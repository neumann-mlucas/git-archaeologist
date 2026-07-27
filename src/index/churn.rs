use std::process::Command;

use anyhow::{Context, Result};

use crate::repo::Repo;

#[derive(Debug, Clone)]
pub struct FileChurn {
    pub path: String,
    pub added: u32,
    pub deleted: u32,
}

/// Per-path adds/dels for one commit vs its first parent.
///
/// Shells out to `git show --numstat` — simpler and much faster than
/// wrestling gix's diff API for v1. Handles the root commit (git diffs
/// against empty tree).
pub fn for_commit(repo: &Repo, sha: &str) -> Result<Vec<FileChurn>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(&repo.root)
        .args([
            "show",
            "--no-renames",
            "--first-parent",
            "--format=",
            "--numstat",
            sha,
        ])
        .output()
        .context("running git show --numstat")?;

    if !output.status.success() {
        anyhow::bail!(
            "git show failed for {sha}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }

    let mut out = Vec::new();
    for line in output.stdout.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        let line = String::from_utf8_lossy(line);
        let mut parts = line.splitn(3, '\t');
        let added_s = parts.next().unwrap_or("");
        let deleted_s = parts.next().unwrap_or("");
        let path = match parts.next() {
            Some(p) => p.to_string(),
            None => continue,
        };
        // Binary files show up as "-\t-\tpath"; skip.
        let added = added_s.parse::<u32>().unwrap_or(0);
        let deleted = deleted_s.parse::<u32>().unwrap_or(0);
        if added_s == "-" && deleted_s == "-" {
            continue;
        }
        out.push(FileChurn {
            path,
            added,
            deleted,
        });
    }

    Ok(out)
}
