//! Ground truth from git: what `line_births` must equal at HEAD.
//!
//! All checks take `BIRTHS_SQL` output as TSV. Files git calls blobs are
//! compared, except binary or oversize ones (the indexer skips those) and
//! empty ones (no lines, no rows).
//!
//! `assert_tokei_close` is a differential sanity check, not truth: tokei
//! has its own counting rules (see `TOKEI_LANGS`). Takes `stats` TSV.

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

/// `index::churn::MAX_BLOB_BYTES`.
const MAX_BLOB_BYTES: usize = 2 * 1024 * 1024;

/// Languages named alike in both tools with matching file sets. Markdown is
/// out (tokei: prose is comments).
const TOKEI_LANGS: [&str; 3] = ["Rust", "Python", "JSON"];
const TOKEI_TOLERANCE: f64 = 0.005;

const BLAME_SAMPLE: usize = 100;
const BLAME_FLOOR: f64 = 0.95;

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
        let binary = body[..body.len().min(8192)].contains(&0);
        if body.is_empty() || body.len() > MAX_BLOB_BYTES || binary {
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
        let blame = blame(repo, &[], &path);
        assert_eq!(shas, blame, "birth commits != git blame for {path}");
    }
}

/// `git blame` of HEAD's `path`: commit per line.
fn blame(repo: &Path, flags: &[&str], path: &str) -> Vec<String> {
    let mut args = vec!["blame", "--root", "-s", "-l"];
    args.extend(flags);
    args.extend(["HEAD", "--", path]);
    String::from_utf8_lossy(&git(repo, &args))
        .lines()
        .map(|l| l.split_whitespace().next().unwrap().to_string())
        .collect()
}

/// Share of lines agreeing with `git blame --first-parent`, the fold's own
/// semantics, is at least `BLAME_FLOOR`, over a sample of up to
/// `BLAME_SAMPLE` files. A floor, not equality: blame's diff and ours can
/// pick different lines when a file repeats content.
pub fn assert_blame_close(repo: &Path, births_tsv: &str) {
    let births = parse_births(births_tsv);
    let step = (births.len() / BLAME_SAMPLE).max(1);
    let (mut same, mut total) = (0, 0);
    for (path, shas) in births.iter().step_by(step) {
        let blame = blame(repo, &["--first-parent"], path);
        same += shas.iter().zip(&blame).filter(|(a, b)| a == b).count();
        total += shas.len();
    }
    let share = same as f64 / total as f64;
    eprintln!("blame --first-parent: {same}/{total} lines, {share:.4}");
    assert!(share >= BLAME_FLOOR, "births match blame on {share:.4}");
}

/// `code + blanks` per `TOKEI_LANGS` language, from `stats --by language`
/// TSV, within `TOKEI_TOLERANCE` of tokei on the same tree. Summed because
/// a blank line inside a string is code to tokei and blank to us. Comments
/// stay out: tokei splits Rust doc comments into embedded Markdown. Skips
/// when tokei isn't installed.
pub fn assert_tokei_close(repo: &Path, stats_tsv: &str) {
    if Command::new("tokei").arg("--version").output().is_err() {
        eprintln!("skip tokei check: tokei not on PATH");
        return;
    }
    // Archive, not the checkout: same files as HEAD, nothing untracked.
    let dir = tempfile::TempDir::new().unwrap();
    let tar = dir.path().join("head.tar");
    git(repo, &["archive", "-o", tar.to_str().unwrap(), "HEAD"]);
    let tree = dir.path().join("tree");
    std::fs::create_dir(&tree).unwrap();
    let status = Command::new("tar")
        .arg("-xf")
        .arg(&tar)
        .arg("-C")
        .arg(&tree)
        .status()
        .unwrap();
    assert!(status.success(), "tar: {status}");
    let out = Command::new("tokei")
        // SCsub: SCons build files, Python to tokei, unknown to us (godot).
        .args([
            "--hidden",
            "--no-ignore",
            "--exclude",
            "SCsub",
            "-o",
            "json",
        ])
        .arg(&tree)
        .output()
        .unwrap();
    assert!(out.status.success(), "tokei: {}", out.status);
    let tokei: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();

    let mut lines = stats_tsv.lines();
    let headers: Vec<&str> = lines.next().unwrap().split('\t').collect();
    let col = |name| headers.iter().position(|h| *h == name).unwrap();
    let (lang, code, blanks) = (col("lang"), col("code"), col("blanks"));
    for row in lines {
        let cols: Vec<&str> = row.split('\t').collect();
        if !TOKEI_LANGS.contains(&cols[lang]) {
            continue;
        }
        let num = |i: usize| cols[i].parse::<f64>().unwrap();
        let ours = num(code) + num(blanks);
        let them = &tokei[cols[lang]];
        let theirs = them["code"].as_f64().unwrap() + them["blanks"].as_f64().unwrap();
        eprintln!("tokei {}: ours {ours}, tokei {theirs}", cols[lang]);
        assert!(
            (ours - theirs).abs() <= theirs * TOKEI_TOLERANCE,
            "{}: code+blanks {ours} vs tokei {theirs}",
            cols[lang]
        );
    }
}
