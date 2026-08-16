//! Scenario DSL for synthetic git repos. Tier 1 `golden.rs` predates this
//! and keeps its inline builder (rewriting would churn pinned parquet
//! goldens); new scenario tests use this DSL to keep setup under ~30 lines.
//!
//! Extend the `Action` enum only when a new scenario needs a new op —
//! Merge, Branch, Checkout, Tag are trivial one-liners to add. Kept out
//! today because no current test needs them.

#![allow(dead_code)]

use std::path::Path;
use std::process::Command;

/// One replayable step against a git repo.
pub enum Action {
    /// Write files (create/overwrite) then commit.
    Commit {
        author: String,
        email: String,
        date: String,
        msg: String,
        writes: Vec<(String, String)>,
    },
    /// Rename a path then commit. Content preserved.
    Rename {
        author: String,
        email: String,
        date: String,
        msg: String,
        from: String,
        to: String,
    },
    /// `git rm` paths then commit.
    Delete {
        author: String,
        email: String,
        date: String,
        msg: String,
        paths: Vec<String>,
    },
}

impl Action {
    /// Convenience: derives email as `{author}@example.com` (lowercased),
    /// matching the tier 1 fixture convention.
    pub fn commit(author: &str, date: &str, msg: &str, writes: &[(&str, &str)]) -> Self {
        Action::Commit {
            author: author.into(),
            email: email_of(author),
            date: date.into(),
            msg: msg.into(),
            writes: writes
                .iter()
                .map(|(p, b)| (p.to_string(), b.to_string()))
                .collect(),
        }
    }

    pub fn rename(author: &str, date: &str, msg: &str, from: &str, to: &str) -> Self {
        Action::Rename {
            author: author.into(),
            email: email_of(author),
            date: date.into(),
            msg: msg.into(),
            from: from.into(),
            to: to.into(),
        }
    }

    pub fn delete(author: &str, date: &str, msg: &str, paths: &[&str]) -> Self {
        Action::Delete {
            author: author.into(),
            email: email_of(author),
            date: date.into(),
            msg: msg.into(),
            paths: paths.iter().map(|p| p.to_string()).collect(),
        }
    }
}

fn email_of(author: &str) -> String {
    format!("{}@example.com", author.to_lowercase())
}

/// Initialize a fresh repo at `dir` and replay `actions` in order.
/// Deterministic: fixed author/committer identities and dates → stable SHAs.
pub fn build(dir: &Path, actions: &[Action]) {
    git(dir, &["init", "-q", "-b", "main"], &[]);
    git(dir, &["config", "commit.gpgsign", "false"], &[]);
    for a in actions {
        run(dir, a);
    }
}

fn run(dir: &Path, a: &Action) {
    match a {
        Action::Commit {
            author,
            email,
            date,
            msg,
            writes,
        } => {
            write_files(dir, writes);
            let paths: Vec<&str> = writes.iter().map(|(p, _)| p.as_str()).collect();
            let mut add = vec!["add", "--"];
            add.extend(paths.iter().copied());
            git(dir, &add, &[]);
            commit_with(dir, author, email, date, msg);
        }
        Action::Rename {
            author,
            email,
            date,
            msg,
            from,
            to,
        } => {
            let src = dir.join(from);
            let dst = dir.join(to);
            if let Some(p) = dst.parent() {
                std::fs::create_dir_all(p).unwrap();
            }
            std::fs::rename(&src, &dst).unwrap();
            git(dir, &["add", "-A", "--"], &[]);
            commit_with(dir, author, email, date, msg);
        }
        Action::Delete {
            author,
            email,
            date,
            msg,
            paths,
        } => {
            let mut rm = vec!["rm", "-q", "--"];
            rm.extend(paths.iter().map(|s| s.as_str()));
            git(dir, &rm, &[]);
            commit_with(dir, author, email, date, msg);
        }
    }
}

fn write_files(dir: &Path, writes: &[(String, String)]) {
    for (path, body) in writes {
        let full = dir.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, body).unwrap();
    }
}

fn commit_with(dir: &Path, author: &str, email: &str, date: &str, msg: &str) {
    git(
        dir,
        &["commit", "-q", "-m", msg],
        &[
            ("GIT_AUTHOR_NAME", author),
            ("GIT_AUTHOR_EMAIL", email),
            ("GIT_COMMITTER_NAME", author),
            ("GIT_COMMITTER_EMAIL", email),
            ("GIT_AUTHOR_DATE", date),
            ("GIT_COMMITTER_DATE", date),
        ],
    );
}

fn git(dir: &Path, args: &[&str], envs: &[(&str, &str)]) {
    let mut cmd = Command::new("git");
    cmd.args(args).current_dir(dir);
    for (k, v) in envs {
        cmd.env(k, v);
    }
    let out = cmd.output().expect("git executable required on PATH");
    assert!(
        out.status.success(),
        "git {args:?} failed: status={}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stderr),
    );
}
