use std::collections::HashMap;
use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};

use anyhow::{Context, Result};

use crate::repo::Repo;

#[derive(Debug, Clone)]
pub struct FileChurn {
    pub path: String,
    pub added: u32,
    pub deleted: u32,
}

/// Streamed per-commit adds/dels for the entire history in one subprocess.
///
/// Replaces 1 `git show` per commit (fork+exec dominated the wall time on
/// large repos like nvim). Format: `--format='C %H'` puts a marker line
/// before each commit's numstat block; we accumulate rows into the current
/// sha and flush on the next marker.
pub fn batch_all(repo: &Repo) -> Result<HashMap<String, Vec<FileChurn>>> {
    // --no-merges matches walker.rs (skip_merges=true) so every commit the
    // indexer writes has churn rows here. --first-parent was previously set
    // but the walker uses full rev traversal, so any commit reachable only
    // through a non-first-parent path got empty churn silently.
    let mut child = Command::new("git")
        .arg("-C")
        .arg(&repo.root)
        .args([
            "log",
            "--no-merges",
            "--no-renames",
            "--format=C %H",
            "--numstat",
            "HEAD",
        ])
        .stdout(Stdio::piped())
        // Piped-but-unread stderr will deadlock if git logs enough warnings
        // to fill its pipe. We don't consume it, so send to /dev/null.
        .stderr(Stdio::null())
        .spawn()
        .context("spawning git log --numstat")?;

    let stdout = child.stdout.take().expect("piped");
    let reader = BufReader::new(stdout);

    let mut out: HashMap<String, Vec<FileChurn>> = HashMap::new();
    let mut current: Option<String> = None;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if let Some(sha) = line.strip_prefix("C ") {
            current = Some(sha.trim().to_string());
            continue;
        }
        if line.is_empty() {
            continue;
        }
        let sha = match &current {
            Some(s) => s,
            None => continue,
        };
        let mut parts = line.splitn(3, '\t');
        let added_s = parts.next().unwrap_or("");
        let deleted_s = parts.next().unwrap_or("");
        let path = match parts.next() {
            Some(p) => p.to_string(),
            None => continue,
        };
        // Binary files show up as "-\t-\tpath"; skip.
        if added_s == "-" && deleted_s == "-" {
            continue;
        }
        let added = added_s.parse::<u32>().unwrap_or(0);
        let deleted = deleted_s.parse::<u32>().unwrap_or(0);
        out.entry(sha.clone()).or_default().push(FileChurn {
            path,
            added,
            deleted,
        });
    }

    let status = child.wait().context("waiting for git log")?;
    if !status.success() {
        anyhow::bail!("git log exited {status}");
    }
    Ok(out)
}
