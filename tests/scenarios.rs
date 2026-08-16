//! Scenario-focused synthetic-repo tests. Complements the Tier 1 catch-all
//! (`golden.rs`) with narrow, invariant-per-test fixtures built via the
//! shared DSL (`common::dsl`). Each test targets one indexer / fold
//! invariant with exact assertions.
//!
//! Add a new file/section here (not another `#[test]` in `golden.rs`)
//! when a real bug is easier to reproduce via a small hand-crafted commit
//! sequence than by extending the tier 1 fixture.

mod common;

use std::path::PathBuf;
use std::process::{Command, Output};

use tempfile::TempDir;

use common::dsl::{self, Action};

const BIN: &str = env!("CARGO_BIN_EXE_git-archaeologist");

struct Env {
    #[allow(dead_code)]
    root: TempDir,
    repo: PathBuf,
    xdg_data: PathBuf,
    xdg_config: PathBuf,
}

impl Env {
    fn new() -> Self {
        let root = TempDir::new().unwrap();
        let repo = root.path().join("repo");
        let xdg_data = root.path().join("xdg_data");
        let xdg_config = root.path().join("xdg_config");
        std::fs::create_dir_all(&repo).unwrap();
        std::fs::create_dir_all(&xdg_data).unwrap();
        std::fs::create_dir_all(&xdg_config).unwrap();
        Env {
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
            .env("TERM", "dumb")
            .output()
            .expect("run git-archaeologist")
    }

    fn arch_stdout(&self, args: &[&str]) -> String {
        let out = self.arch(args);
        assert!(
            out.status.success(),
            "arch {args:?} failed: {}\nstderr: {}",
            out.status,
            String::from_utf8_lossy(&out.stderr)
        );
        String::from_utf8(out.stdout).expect("utf-8 stdout")
    }

    /// Run a `sql` query, parse the single scalar in the first data row.
    fn sql_i64(&self, query: &str) -> i64 {
        let out = self.arch_stdout(&["sql", query, "--format", "tsv"]);
        out.lines()
            .nth(1)
            .and_then(|s| s.trim().parse().ok())
            .unwrap_or_else(|| panic!("could not parse scalar from:\n{out}"))
    }
}

fn have_git() -> bool {
    Command::new("git").arg("--version").output().is_ok()
}

/// Rename chain a.rs → b.rs → c.rs. Asserts the cohort fold resolves the
/// rename chain to the final canonical path (all birthed lines survive
/// under `c.rs`, not scattered across historical names).
#[test]
fn scenario_rename_chain() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    dsl::build(
        &env.repo,
        &[
            Action::commit(
                "Alice",
                "2025-01-01T00:00:00Z",
                "feat: init",
                &[("a.rs", "fn one() {}\nfn two() {}\nfn three() {}\n")],
            ),
            Action::rename(
                "Alice",
                "2025-01-02T00:00:00Z",
                "refactor: a -> b",
                "a.rs",
                "b.rs",
            ),
            Action::rename(
                "Alice",
                "2025-01-03T00:00:00Z",
                "refactor: b -> c",
                "b.rs",
                "c.rs",
            ),
            Action::commit(
                "Alice",
                "2025-01-04T00:00:00Z",
                "feat: add four",
                &[(
                    "c.rs",
                    "fn one() {}\nfn two() {}\nfn three() {}\nfn four() {}\n",
                )],
            ),
        ],
    );

    let out = env.arch(&["index"]);
    assert!(
        out.status.success(),
        "index failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // All surviving line_births live under the final canonical path.
    let under_c = env.sql_i64("SELECT COUNT(*) FROM line_births WHERE path = 'c.rs'");
    let under_a = env.sql_i64("SELECT COUNT(*) FROM line_births WHERE path = 'a.rs'");
    let under_b = env.sql_i64("SELECT COUNT(*) FROM line_births WHERE path = 'b.rs'");
    assert_eq!(under_a, 0, "no line_births should remain under old 'a.rs'");
    assert_eq!(under_b, 0, "no line_births should remain under old 'b.rs'");
    assert_eq!(
        under_c, 4,
        "expected 4 surviving lines under final canonical 'c.rs'"
    );

    // Two rename links recorded in hunks (a->b, b->c).
    let renames = env
        .sql_i64("SELECT COUNT(DISTINCT prev_path) FROM hunks WHERE prev_path IS NOT NULL");
    assert_eq!(renames, 2, "expected 2 distinct rename prev_path entries");
}

/// Controlled cohort birth/death: three commits, three distinct cohorts,
/// final state has exactly one cohort surviving with a known line count.
/// Asserts the fold correctly attributes surviving lines to the commit
/// that birthed them (not the commit that last edited surrounding lines).
#[test]
fn scenario_cohort_survival() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    // c1: 10 lines from cohort A
    let cohort_a: String = (0..10).map(|i| format!("A{i}\n")).collect();
    // c2: keep first 5 A lines, add 5 B lines
    let mixed: String = (0..5)
        .map(|i| format!("A{i}\n"))
        .chain((0..5).map(|i| format!("B{i}\n")))
        .collect();
    // c3: delete first 5 A lines, keep the 5 B lines → only cohort B alive
    let only_b: String = (0..5).map(|i| format!("B{i}\n")).collect();

    dsl::build(
        &env.repo,
        &[
            Action::commit(
                "Alice",
                "2025-01-01T00:00:00Z",
                "feat: cohort A",
                &[("f.txt", &cohort_a)],
            ),
            Action::commit(
                "Alice",
                "2025-01-02T00:00:00Z",
                "feat: cohort B, drop half of A",
                &[("f.txt", &mixed)],
            ),
            Action::commit(
                "Alice",
                "2025-01-03T00:00:00Z",
                "chore: drop remaining A",
                &[("f.txt", &only_b)],
            ),
        ],
    );

    let out = env.arch(&["index"]);
    assert!(
        out.status.success(),
        "index failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    // Exactly one surviving cohort (B), 5 lines. Small repo (3 commits) → auto
    // bucket = Commit, so cohort A and cohort B live in distinct buckets.
    let cohorts = env.arch_stdout(&[
        "sql",
        "SELECT birth_bucket, COUNT(*) AS n FROM line_births \
         WHERE path='f.txt' GROUP BY birth_bucket ORDER BY birth_bucket",
        "--format",
        "tsv",
    ]);
    let data: Vec<&str> = cohorts.lines().skip(1).collect();
    assert_eq!(
        data.len(),
        1,
        "expected exactly 1 surviving cohort, got:\n{cohorts}"
    );
    let count: i64 = data[0]
        .split('\t')
        .nth(1)
        .and_then(|s| s.trim().parse().ok())
        .unwrap_or_else(|| panic!("could not parse count from:\n{cohorts}"));
    assert_eq!(count, 5, "expected 5 lines from cohort B, got {count}");

    // Total line_births rows = 5 (only surviving-line rows are materialized).
    let total = env.sql_i64("SELECT COUNT(*) FROM line_births WHERE path='f.txt'");
    assert_eq!(total, 5, "expected 5 total line_births for f.txt");
}
