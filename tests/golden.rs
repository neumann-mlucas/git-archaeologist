//! Tier 1 golden repo integration test (SPEC §Test strategy).
//!
//! Builds a small 3-author / 7-language repo with the full feature matrix
//! (rename, merge, revert, `Co-authored-by:`, `feat!:`, tag,
//! `.git-blame-ignore-revs`) and asserts every subcommand runs + row-count
//! invariants hold. Not byte-exact — DuckDB output formatting drifts across
//! versions; the intent is regression detection on shape + counts.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use tempfile::TempDir;

const BIN: &str = env!("CARGO_BIN_EXE_git-archaeologist");

struct Fixture {
    #[allow(dead_code)]
    root: TempDir,
    repo: PathBuf,
    xdg_data: PathBuf,
    xdg_config: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let repo = root.path().join("repo");
        let xdg_data = root.path().join("xdg_data");
        let xdg_config = root.path().join("xdg_config");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&xdg_data).unwrap();
        std::fs::create_dir_all(&xdg_config).unwrap();
        Fixture {
            root,
            repo,
            xdg_data,
            xdg_config,
        }
    }

    fn arch(&self, args: &[&str]) -> Output {
        Command::new(BIN)
            .arg("--repo")
            .arg(&self.repo)
            .args(args)
            .env("XDG_DATA_HOME", &self.xdg_data)
            .env("XDG_CONFIG_HOME", &self.xdg_config)
            // Force TTY-off so --format defaults to tsv.
            .env("TERM", "dumb")
            .output()
            .expect("run git-archaeologist")
    }

    fn arch_stdout(&self, args: &[&str]) -> String {
        let out = self.arch(args);
        assert!(
            out.status.success(),
            "arch {:?} failed: {}\nstderr: {}",
            args,
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).expect("utf-8 stdout")
    }
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
        "git {args:?} failed: status={}\nstdout: {}\nstderr: {}",
        out.status,
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr),
    );
}

/// Commit a set of file edits with a fixed author + date.
fn commit(
    dir: &Path,
    author: &str,
    email: &str,
    date: &str,
    msg: &str,
    files: &[(&str, &str)],
) {
    for (path, body) in files {
        let full = dir.join(path);
        if let Some(parent) = full.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&full, body).unwrap();
    }
    let paths: Vec<&str> = files.iter().map(|(p, _)| *p).collect();
    let mut add_args = vec!["add", "--"];
    add_args.extend(paths.iter().copied());
    git(dir, &add_args, &[]);
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

/// Build the tier-1 golden fixture. Deterministic authors + dates so
/// counts are stable across runs.
fn build_fixture(dir: &Path) {
    // Init.
    git(dir, &["init", "-q", "-b", "main"], &[]);
    git(dir, &["config", "commit.gpgsign", "false"], &[]);

    // c1..c3 — Rust file grows.
    commit(
        dir,
        "Alice",
        "alice@example.com",
        "2025-01-01T00:00:00Z",
        "feat: add rust module",
        &[(
            "src/lib.rs",
            "fn a() { println!(\"a\"); }\nfn b() { println!(\"b\"); }\n",
        )],
    );
    commit(
        dir,
        "Alice",
        "alice@example.com",
        "2025-01-02T00:00:00Z",
        "feat(core): rust extension",
        &[(
            "src/lib.rs",
            "fn a() { println!(\"a\"); }\nfn b() { println!(\"b\"); }\nfn c() { println!(\"c\"); }\n",
        )],
    );
    commit(
        dir,
        "Alice",
        "alice@example.com",
        "2025-01-03T00:00:00Z",
        "fix: typo",
        &[(
            "src/lib.rs",
            "fn a() { println!(\"aa\"); }\nfn b() { println!(\"b\"); }\nfn c() { println!(\"c\"); }\n",
        )],
    );

    // c4..c6 — Bob adds Python + JS + TS.
    commit(
        dir,
        "Bob",
        "bob@example.com",
        "2025-01-04T00:00:00Z",
        "feat: python helper",
        &[(
            "src/util.py",
            "def helper():\n    return 42\n",
        )],
    );
    commit(
        dir,
        "Bob",
        "bob@example.com",
        "2025-01-05T00:00:00Z",
        "feat: js entry",
        &[(
            "web/app.js",
            "function greet() { return 'hi'; }\n",
        )],
    );
    commit(
        dir,
        "Bob",
        "bob@example.com",
        "2025-01-06T00:00:00Z",
        "feat(ui): typescript config",
        &[(
            "web/config.ts",
            "export const NAME: string = 'demo';\n",
        )],
    );

    // c7 — Carol adds Go, C, C++.
    commit(
        dir,
        "Carol",
        "carol@example.com",
        "2025-01-07T00:00:00Z",
        "feat: go pkg",
        &[(
            "svc/main.go",
            "package main\nfunc main() { println(\"go\") }\n",
        )],
    );
    commit(
        dir,
        "Carol",
        "carol@example.com",
        "2025-01-08T00:00:00Z",
        "feat: c core",
        &[(
            "native/core.c",
            "int add(int a, int b) { return a + b; }\n",
        )],
    );
    commit(
        dir,
        "Carol",
        "carol@example.com",
        "2025-01-09T00:00:00Z",
        "feat: cpp wrapper",
        &[(
            "native/wrap.cpp",
            "int mul(int a, int b) { return a * b; }\n",
        )],
    );

    // c10 — Co-authored-by trailer.
    commit(
        dir,
        "Alice",
        "alice@example.com",
        "2025-01-10T00:00:00Z",
        "feat: paired change\n\nCo-authored-by: Bob <bob@example.com>",
        &[(
            "src/lib.rs",
            "fn a() { println!(\"aa\"); }\nfn b() { println!(\"bb\"); }\nfn c() { println!(\"c\"); }\nfn d() { println!(\"d\"); }\n",
        )],
    );

    // c11 — feat!: breaking.
    commit(
        dir,
        "Alice",
        "alice@example.com",
        "2025-01-11T00:00:00Z",
        "feat!: breaking rename API",
        &[(
            "src/lib.rs",
            "fn alpha() { println!(\"aa\"); }\nfn beta() { println!(\"bb\"); }\n",
        )],
    );

    // c12 — rename src/lib.rs -> src/core.rs. Content unchanged.
    let old = dir.join("src/lib.rs");
    let new = dir.join("src/core.rs");
    std::fs::rename(&old, &new).unwrap();
    git(dir, &["add", "-A", "--"], &[]);
    git(
        dir,
        &["commit", "-q", "-m", "refactor: rename lib -> core"],
        &[
            ("GIT_AUTHOR_NAME", "Alice"),
            ("GIT_AUTHOR_EMAIL", "alice@example.com"),
            ("GIT_COMMITTER_NAME", "Alice"),
            ("GIT_COMMITTER_EMAIL", "alice@example.com"),
            ("GIT_AUTHOR_DATE", "2025-01-12T00:00:00Z"),
            ("GIT_COMMITTER_DATE", "2025-01-12T00:00:00Z"),
        ],
    );

    // c13 — tag.
    git(
        dir,
        &["tag", "-a", "v0.1", "-m", "first release"],
        &[
            ("GIT_COMMITTER_NAME", "Alice"),
            ("GIT_COMMITTER_EMAIL", "alice@example.com"),
            ("GIT_COMMITTER_DATE", "2025-01-12T12:00:00Z"),
        ],
    );

    // c14 — .git-blame-ignore-revs entry (points at c11).
    let head_of_c11 = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD~1"])
            .current_dir(dir)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    commit(
        dir,
        "Alice",
        "alice@example.com",
        "2025-01-13T00:00:00Z",
        "chore: ignore-revs",
        &[(
            ".git-blame-ignore-revs",
            &format!("# reformat commits\n{head_of_c11}\n"),
        )],
    );

    // c15 — chore.
    commit(
        dir,
        "Bob",
        "bob@example.com",
        "2025-01-14T00:00:00Z",
        "chore(js): tidy",
        &[(
            "web/app.js",
            "function greet() { return 'hello'; }\nfunction wave() {}\n",
        )],
    );

    // c16 — merge (short-lived branch).
    git(dir, &["checkout", "-q", "-b", "feature-x"], &[]);
    commit(
        dir,
        "Carol",
        "carol@example.com",
        "2025-01-15T00:00:00Z",
        "feat: branch work",
        &[(
            "native/wrap.cpp",
            "int mul(int a, int b) { return a * b; }\nint neg(int a) { return -a; }\n",
        )],
    );
    git(dir, &["checkout", "-q", "main"], &[]);
    git(
        dir,
        &[
            "merge",
            "--no-ff",
            "-q",
            "feature-x",
            "-m",
            "merge feature-x",
        ],
        &[
            ("GIT_AUTHOR_NAME", "Alice"),
            ("GIT_AUTHOR_EMAIL", "alice@example.com"),
            ("GIT_COMMITTER_NAME", "Alice"),
            ("GIT_COMMITTER_EMAIL", "alice@example.com"),
            ("GIT_AUTHOR_DATE", "2025-01-16T00:00:00Z"),
            ("GIT_COMMITTER_DATE", "2025-01-16T00:00:00Z"),
        ],
    );

    // c17 — revert of c15 ("chore(js): tidy").
    // After the merge, HEAD~1 (first-parent) is the pre-merge main tip = c15.
    let c15_sha = String::from_utf8(
        Command::new("git")
            .args(["rev-parse", "HEAD~1"])
            .current_dir(dir)
            .output()
            .unwrap()
            .stdout,
    )
    .unwrap()
    .trim()
    .to_string();
    git(
        dir,
        &["revert", "--no-edit", &c15_sha],
        &[
            ("GIT_AUTHOR_NAME", "Bob"),
            ("GIT_AUTHOR_EMAIL", "bob@example.com"),
            ("GIT_COMMITTER_NAME", "Bob"),
            ("GIT_COMMITTER_EMAIL", "bob@example.com"),
            ("GIT_AUTHOR_DATE", "2025-01-17T00:00:00Z"),
            ("GIT_COMMITTER_DATE", "2025-01-17T00:00:00Z"),
        ],
    );

    // c18..c30 — filler + a few multi-file commits so coupling has data.
    for i in 0..13 {
        let day = 18 + i;
        let author = if i % 2 == 0 { "Alice" } else { "Carol" };
        let email = if i % 2 == 0 {
            "alice@example.com"
        } else {
            "carol@example.com"
        };
        if i % 4 == 0 {
            // Multi-file commit — bumps two files together.
            commit(
                dir,
                author,
                email,
                &format!("2025-01-{day}T00:00:00Z"),
                &format!("chore: multi-file {i}"),
                &[
                    ("src/util.py", &format!("def helper():\n    return {i}\n")),
                    ("web/app.js", &format!("function greet() {{ return 'hi{i}'; }}\n")),
                ],
            );
        } else {
            commit(
                dir,
                author,
                email,
                &format!("2025-01-{day}T00:00:00Z"),
                &format!("chore: filler {i}"),
                &[(
                    "src/util.py",
                    &format!("def helper():\n    return {i}\n"),
                )],
            );
        }
    }
}

fn line_count(s: &str) -> usize {
    s.lines().count()
}

#[test]
fn tier1_golden_integration() {
    if Command::new("git").arg("--version").output().is_err() {
        eprintln!("skip: git not on PATH");
        return;
    }

    let fx = Fixture::new();
    build_fixture(&fx.repo);

    // --- index ---
    let out = fx.arch(&["index"]);
    assert!(
        out.status.success(),
        "index failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // --- row invariants via sql ---
    let commits =
        fx.arch_stdout(&["sql", "SELECT COUNT(*) AS n FROM commits", "--format", "tsv"]);
    // 30 fixture commits + 1 merge = 31 in the DAG (all reachable from HEAD).
    let n: i64 = commits
        .lines()
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("commits count parses");
    assert!(n >= 30, "expected >= 30 commits, got {n}");

    let tags = fx.arch_stdout(&["sql", "SELECT COUNT(*) FROM tags", "--format", "tsv"]);
    assert!(tags.contains('1'), "expected 1 tag row, got {tags:?}");

    let breaking = fx.arch_stdout(&[
        "sql",
        "SELECT COUNT(*) FROM commits WHERE is_breaking = TRUE",
        "--format",
        "tsv",
    ]);
    assert!(breaking.contains('1'), "expected 1 breaking commit");

    let trailers = fx.arch_stdout(&[
        "sql",
        "SELECT COUNT(*) FROM commit_trailers",
        "--format",
        "tsv",
    ]);
    let tcount: i64 = trailers.lines().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    assert!(tcount >= 1, "expected >= 1 trailer row, got {tcount}");

    let ignored = fx.arch_stdout(&[
        "sql",
        "SELECT COUNT(*) FROM commits WHERE ignored_blame = TRUE",
        "--format",
        "tsv",
    ]);
    let icount: i64 = ignored.lines().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    assert!(icount >= 1, "expected >= 1 ignored_blame commit, got {icount}");

    let renames = fx.arch_stdout(&[
        "sql",
        "SELECT COUNT(*) FROM hunks WHERE prev_path IS NOT NULL",
        "--format",
        "tsv",
    ]);
    let rcount: i64 = renames.lines().nth(1).and_then(|s| s.parse().ok()).unwrap_or(0);
    assert!(rcount >= 1, "expected >= 1 rename hunk row, got {rcount}");

    // --- every subcommand exits 0 with non-empty stdout ---
    for (label, args) in [
        ("burndown-lang", vec!["burndown", "--by", "language", "--format", "tsv"]),
        ("burndown-author", vec!["burndown", "--by", "author", "--format", "tsv"]),
        ("classify", vec!["classify", "--format", "tsv"]),
        ("churn-module", vec!["churn", "--by", "module", "--format", "tsv"]),
        ("churn-author", vec!["churn", "--by", "author", "--format", "tsv"]),
        ("age", vec!["age", "--format", "tsv"]),
        ("coupling", vec!["coupling", "--top", "5", "--format", "tsv"]),
        ("hotspot", vec!["hotspot", "--lang", "rust", "--top", "5", "--format", "tsv"]),
        ("cohort", vec!["cohort", "--format", "tsv"]),
        ("survival", vec!["survival", "--format", "tsv"]),
    ] {
        let stdout = fx.arch_stdout(&args);
        assert!(
            line_count(&stdout) >= 2,
            "{label} produced too-few lines:\n{stdout}"
        );
    }

    // --- export parquet round-trip ---
    let out_dir = fx.root.path().join("export");
    fx.arch_stdout(&["export", "parquet", out_dir.to_str().unwrap()]);
    for table in [
        "meta",
        "authors",
        "author_aliases",
        "commits",
        "commit_parents",
        "commit_trailers",
        "tags",
        "hunks",
        "file_churn",
        "file_stats",
        "funcs",
        "line_births",
    ] {
        let p = out_dir.join(format!("{table}.parquet"));
        assert!(p.exists(), "missing parquet: {}", p.display());
        assert!(std::fs::metadata(&p).unwrap().len() > 0, "empty: {}", p.display());
    }
}
