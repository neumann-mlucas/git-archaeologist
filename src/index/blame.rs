use std::collections::HashMap;
use std::path::Path;
use std::process::{Command, Stdio};

use anyhow::{bail, Context, Result};
use duckdb::Connection;

use crate::index::mailmap::AuthorResolver;
use crate::index::treesitter::LangRegistry;
use crate::repo::Repo;

/// Aggregate blame result for a single (sha, path) pair.
pub struct BlameCounts {
    pub path: String,
    /// author_id → line count.
    pub by_author: HashMap<i64, u64>,
}

/// Blame every file at `sha` that has a recognized language extension, using
/// `git blame --incremental` (much smaller output than `--porcelain`).
///
/// Returns one entry per path; empty vec if `sha` is unknown / tree walk fails.
pub fn snapshot(
    repo: &Repo,
    sha: &str,
    resolver: &mut AuthorResolver,
    conn: &Connection,
    registry: &LangRegistry,
) -> Result<Vec<BlameCounts>> {
    let oid: gix::ObjectId = sha.parse().context("parsing sha")?;
    let commit = repo
        .git
        .find_object(oid)?
        .try_into_commit()?;
    let tree = commit.tree()?;
    let entries = tree.traverse().breadthfirst.files()?;

    let mut out = Vec::new();
    for entry in entries {
        if !entry.mode.is_blob() {
            continue;
        }
        let path = entry.filepath.to_string();
        if !registry.is_known(&path) {
            continue;
        }
        let by_raw = match blame_file(&repo.root, sha, &path) {
            Ok(m) => m,
            Err(_) => continue, // symlink / oversized / gone — skip silently
        };
        let mut by_author: HashMap<i64, u64> = HashMap::new();
        for ((name, email), lines) in by_raw {
            let id = resolver.resolve(conn, &name, &email)?;
            *by_author.entry(id).or_insert(0) += lines;
        }
        if !by_author.is_empty() {
            out.push(BlameCounts { path, by_author });
        }
    }
    Ok(out)
}

/// Run `git blame --incremental <sha> -- <path>` and parse the streamed
/// output into per-raw-author line counts.
fn blame_file(
    repo_root: &Path,
    sha: &str,
    path: &str,
) -> Result<HashMap<(String, String), u64>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root)
        .arg("blame")
        .arg("--incremental")
        .arg("-w")
        .arg(sha)
        .arg("--")
        .arg(path)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .context("spawning git blame")?;
    if !output.status.success() {
        bail!("git blame failed: {}", output.status);
    }
    Ok(parse_incremental(&String::from_utf8_lossy(&output.stdout)))
}

/// Parse `git blame --incremental` output into (raw name, raw email) → lines.
///
/// Format per hunk:
///
/// ```text
/// <40-hex-sha> <orig-lineno> <final-lineno> <num-lines>
/// author <name>
/// author-mail <<email>>
/// … (author-time, committer-*, summary, previous, boundary — ignored)
/// filename <path>
/// ```
///
/// A commit's metadata lines are only sent once; later hunks referencing the
/// same sha get header + filename only.
fn parse_incremental(text: &str) -> HashMap<(String, String), u64> {
    let mut info: HashMap<String, (String, String)> = HashMap::new();
    let mut pending: HashMap<String, u64> = HashMap::new();
    let mut counts: HashMap<(String, String), u64> = HashMap::new();
    let mut current_sha = String::new();

    for line in text.lines() {
        if let Some((sha, num)) = parse_header(line) {
            current_sha = sha.to_string();
            if let Some((n, e)) = info.get(&current_sha) {
                *counts.entry((n.clone(), e.clone())).or_insert(0) += num;
            } else {
                *pending.entry(current_sha.clone()).or_insert(0) += num;
            }
        } else if let Some(name) = line.strip_prefix("author ") {
            info.entry(current_sha.clone())
                .or_default()
                .0 = name.to_string();
        } else if let Some(mail) = line.strip_prefix("author-mail ") {
            let email = mail.trim_matches(|c| c == '<' || c == '>').to_string();
            let entry = info.entry(current_sha.clone()).or_default();
            entry.1 = email;
            // Both fields set — flush any lines pending on this sha.
            if let Some(pending_n) = pending.remove(&current_sha) {
                *counts.entry((entry.0.clone(), entry.1.clone())).or_insert(0) += pending_n;
            }
        }
        // All other keys (author-time, committer-*, summary, filename,
        // previous, boundary) are ignored.
    }

    counts
}

/// Detect a hunk header: `<40 lowercase hex> <int> <int> <int>`.
fn parse_header(line: &str) -> Option<(&str, u64)> {
    let mut parts = line.split_ascii_whitespace();
    let sha = parts.next()?;
    if sha.len() != 40 || !sha.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let _orig: u64 = parts.next()?.parse().ok()?;
    let _final: u64 = parts.next()?.parse().ok()?;
    let num: u64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((sha, num))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_two_commits_same_author() {
        let text = "\
0000000000000000000000000000000000000001 1 1 2
author Alice
author-mail <alice@example.com>
author-time 1735689600
author-tz +0000
committer Alice
committer-mail <alice@example.com>
summary c1
filename main.rs
0000000000000000000000000000000000000002 3 3 3
author Bob
author-mail <bob@example.com>
author-time 1735689700
author-tz +0000
committer Bob
committer-mail <bob@example.com>
summary c2
filename main.rs
0000000000000000000000000000000000000001 6 6 1
filename main.rs
";
        let counts = parse_incremental(text);
        assert_eq!(
            counts.get(&("Alice".to_string(), "alice@example.com".to_string())),
            Some(&3)
        );
        assert_eq!(
            counts.get(&("Bob".to_string(), "bob@example.com".to_string())),
            Some(&3)
        );
    }

    #[test]
    fn header_detection() {
        assert!(parse_header("abc").is_none());
        assert!(parse_header("author Alice").is_none());
        assert!(
            parse_header("0000000000000000000000000000000000000001 1 1 2")
                == Some(("0000000000000000000000000000000000000001", 2))
        );
        // 40 chars but not hex
        assert!(parse_header("zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz 1 1 1").is_none());
    }
}
