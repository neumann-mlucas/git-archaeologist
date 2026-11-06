//! Ground truth from git: what `line_births` must equal at HEAD.
//!
//! Both checks take `BIRTHS_SQL` output as TSV. Files git calls blobs are
//! compared, except binary ones (the indexer skips those) and empty ones
//! (no lines, no rows).

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

pub const BIRTHS_SQL: &str = "SELECT path, line_no, birth_sha FROM line_births \
                              ORDER BY path, line_no";

fn git(repo: &Path, args: &[&str]) -> Vec<u8> {
    let out = Command::new("git")
        .args(args)
        .current_dir(repo)
        .output()
        .expect("git executable required on PATH");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    out.stdout
}

/// HEAD's text blobs: path → line count.
fn head_line_counts(repo: &Path) -> BTreeMap<String, usize> {
    let listing = git(repo, &["ls-tree", "-r", "-z", "HEAD"]);
    let mut counts = BTreeMap::new();
    for entry in String::from_utf8(listing).unwrap().split_terminator('\0') {
        let (meta, path) = entry.split_once('\t').unwrap();
        if !meta.starts_with("100") {
            continue; // symlink / submodule
        }
        let body = git(repo, &["show", &format!("HEAD:{path}")]);
        if body.is_empty() || body.contains(&0) {
            continue;
        }
        let breaks = body.iter().filter(|&&b| b == b'\n').count();
        counts.insert(
            path.to_string(),
            breaks + usize::from(body.last() != Some(&b'\n')),
        );
    }
    counts
}

/// `line_births` TSV: path → birth shas in line order.
fn parse_births(tsv: &str) -> BTreeMap<String, Vec<String>> {
    let mut births: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for line in tsv.lines().skip(1) {
        let cols: Vec<&str> = line.split('\t').collect();
        births
            .entry(cols[0].to_string())
            .or_default()
            .push(cols[2].to_string());
    }
    births
}

/// Every HEAD file has exactly as many `line_births` rows as lines, and no
/// other path has any.
pub fn assert_line_counts(repo: &Path, births_tsv: &str) {
    let got: BTreeMap<String, usize> = parse_births(births_tsv)
        .into_iter()
        .map(|(p, shas)| (p, shas.len()))
        .collect();
    assert_eq!(got, head_line_counts(repo), "line_births != HEAD's files");
}

/// `assert_line_counts`, plus every line's birth commit equals `git blame`.
/// Needs histories whose diffs are unambiguous: blame's diff and ours can
/// legitimately pick different lines when a file repeats content.
pub fn assert_births(repo: &Path, births_tsv: &str) {
    assert_line_counts(repo, births_tsv);
    for (path, shas) in parse_births(births_tsv) {
        let blame = git(
            repo,
            &["blame", "--root", "-s", "-l", "HEAD", "--", path.as_str()],
        );
        let blame: Vec<String> = String::from_utf8_lossy(&blame)
            .lines()
            .map(|l| l.split_whitespace().next().unwrap().to_string())
            .collect();
        assert_eq!(shas, blame, "birth commits != git blame for {path}");
    }
}
