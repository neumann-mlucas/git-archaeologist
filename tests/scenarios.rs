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
use common::truth;

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

    fn births(&self) -> String {
        self.arch_stdout(&["sql", truth::BIRTHS_SQL, "--format", "tsv"])
    }

    /// `line_births` equals `git blame` at HEAD, line for line.
    fn assert_matches_git(&self) {
        truth::assert_births(&self.repo, &self.births());
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
    let renames =
        env.sql_i64("SELECT COUNT(DISTINCT prev_path) FROM hunks WHERE prev_path IS NOT NULL");
    assert_eq!(renames, 2, "expected 2 distinct rename prev_path entries");
    env.assert_matches_git();
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
    env.assert_matches_git();
}

/// Raw git in the scenario repo with a fixed identity and date.
fn git_at(env: &Env, date: &str, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(&env.repo)
        .env("GIT_AUTHOR_NAME", "Bob")
        .env("GIT_AUTHOR_EMAIL", "bob@example.com")
        .env("GIT_COMMITTER_NAME", "Bob")
        .env("GIT_COMMITTER_EMAIL", "bob@example.com")
        .env("GIT_AUTHOR_DATE", date)
        .env("GIT_COMMITTER_DATE", date)
        .output()
        .expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8(out.stdout).unwrap().trim().to_string()
}

/// Ref scope: an unmerged branch is indexed (commits + churn) but kept out
/// of HEAD's cohort; stash and notes commits are not history at all.
#[test]
fn scenario_ref_scope() {
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
                "feat: a",
                &[("a.txt", "1\n2\n")],
            ),
            Action::commit(
                "Alice",
                "2025-01-02T00:00:00Z",
                "feat: b",
                &[("b.txt", "1\n")],
            ),
        ],
    );
    let head = git_at(&env, "2025-01-02T00:00:00Z", &["rev-parse", "HEAD"]);
    // Newer commit on an unmerged branch.
    git_at(
        &env,
        "2025-01-03T00:00:00Z",
        &["checkout", "-q", "-b", "side"],
    );
    std::fs::write(env.repo.join("side.txt"), "s1\ns2\ns3\n").unwrap();
    git_at(&env, "2025-01-03T00:00:00Z", &["add", "side.txt"]);
    git_at(
        &env,
        "2025-01-03T00:00:00Z",
        &["commit", "-q", "-m", "feat: side"],
    );
    git_at(&env, "2025-01-03T00:00:00Z", &["checkout", "-q", "main"]);
    // Stash (2 commits) + note (1 commit) — neither may be indexed.
    std::fs::write(env.repo.join("a.txt"), "wip\n").unwrap();
    git_at(&env, "2025-01-04T00:00:00Z", &["stash", "-q"]);
    git_at(
        &env,
        "2025-01-04T00:00:00Z",
        &["notes", "add", "-m", "note", "HEAD"],
    );

    env.arch_stdout(&["index"]);

    assert_eq!(env.sql_i64("SELECT COUNT(*) FROM commits"), 3);
    assert_eq!(
        env.sql_i64("SELECT COUNT(*) FROM file_churn WHERE path = 'side.txt'"),
        1,
        "side-branch commit must get churn like every other commit"
    );
    assert_eq!(
        env.sql_i64("SELECT COUNT(*) FROM line_births WHERE path = 'side.txt'"),
        0,
        "unmerged branch must not leak into HEAD's cohort"
    );
    assert_eq!(env.sql_i64("SELECT COUNT(*) FROM line_births"), 3);
    let indexed = env.arch_stdout(&[
        "sql",
        "SELECT value FROM meta WHERE key = 'indexed_head_sha'",
    ]);
    assert_eq!(indexed.lines().nth(1), Some(head.as_str()));
    env.assert_matches_git();
}

/// f.rs → g.rs, then a brand-new f.rs: the new file must not inherit the
/// renamed file's history (and vice versa).
#[test]
fn scenario_path_reused_after_rename() {
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
                "feat: f",
                &[("f.rs", "fn a() {}\nfn b() {}\nfn c() {}\n")],
            ),
            Action::rename(
                "Alice",
                "2025-01-02T00:00:00Z",
                "refactor: f -> g",
                "f.rs",
                "g.rs",
            ),
            Action::commit(
                "Alice",
                "2025-01-03T00:00:00Z",
                "feat: new f",
                &[("f.rs", "fn z() {}\n")],
            ),
        ],
    );
    env.arch_stdout(&["index"]);
    assert_eq!(
        env.sql_i64("SELECT COUNT(*) FROM line_births WHERE path = 'g.rs'"),
        3
    );
    assert_eq!(
        env.sql_i64("SELECT COUNT(*) FROM line_births WHERE path = 'f.rs'"),
        1
    );
    env.assert_matches_git();
}

fn one_commit(env: &Env, writes: &[(&str, &str)]) {
    dsl::build(
        &env.repo,
        &[Action::commit(
            "Alice",
            "2025-01-01T00:00:00Z",
            "feat: init",
            writes,
        )],
    );
}

/// The same blob under two extensions gets each path's own language.
#[test]
fn scenario_shared_blob_keeps_per_path_language() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    one_commit(&env, &[("pkg/__init__.py", ""), ("src/mod.rs", "")]);
    env.arch_stdout(&["index"]);
    let langs = env.arch_stdout(&[
        "sql",
        "SELECT path, language FROM file_stats ORDER BY path",
        "--format",
        "tsv",
    ]);
    assert_eq!(
        langs.lines().skip(1).collect::<Vec<_>>(),
        ["pkg/__init__.py\tPython", "src/mod.rs\tRust"]
    );
}

/// `_` in `--path` is a literal, not a LIKE wildcard.
#[test]
fn scenario_path_filter_is_literal() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    one_commit(&env, &[("a_b/x.txt", "1\n"), ("axb/x.txt", "1\n2\n")]);
    env.arch_stdout(&["index"]);
    let out = env.arch_stdout(&[
        "churn", "--by", "module", "--path", "a_b", "--format", "tsv",
    ]);
    let modules: Vec<&str> = out
        .lines()
        .skip(1)
        .map(|l| l.split('\t').nth(1).unwrap())
        .collect();
    assert_eq!(modules, ["a_b"]);
}

/// `reindex` re-canonicalizes authors after an aliases.toml change.
#[test]
fn scenario_reindex_applies_new_aliases() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    one_commit(&env, &[("a.txt", "1\n")]);
    env.arch_stdout(&["index"]);
    assert_eq!(
        env.sql_i64("SELECT COUNT(*) FROM authors WHERE canonical_name = 'Alice'"),
        1
    );

    let cfg = env.xdg_config.join("git-archaeologist");
    std::fs::create_dir_all(&cfg).unwrap();
    std::fs::write(
        cfg.join("aliases.toml"),
        "[[alias]]\ncanonical_name = \"Alice A.\"\ncanonical_email = \"a@corp\"\n\
         raw = [{ name = \"Alice\", email = \"alice@example.com\" }]\n",
    )
    .unwrap();
    env.arch_stdout(&["reindex"]);

    assert_eq!(
        env.sql_i64("SELECT COUNT(*) FROM authors"),
        1,
        "stale author row left"
    );
    assert_eq!(
        env.sql_i64(
            "SELECT COUNT(*) FROM author_aliases al JOIN authors a ON a.id = al.author_id \
             WHERE a.canonical_name = 'Alice A.'"
        ),
        1,
        "alias row must point at the new canonical author"
    );
}

/// A cache locked by another process is reported, never wiped.
#[test]
fn scenario_locked_cache_is_not_wiped() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    one_commit(&env, &[("a.txt", "1\n")]);
    env.arch_stdout(&["index"]);
    let cache = std::fs::read_dir(env.xdg_data.join("git-archaeologist/caches"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("cache.duckdb");

    let holder = duckdb::Connection::open(&cache).unwrap();
    let out = env.arch(&["sql", "SELECT 1"]);
    assert!(!out.status.success(), "must refuse a locked cache");
    assert!(
        String::from_utf8_lossy(&out.stderr).contains("in use"),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    drop(holder);

    assert_eq!(
        env.sql_i64("SELECT COUNT(*) FROM commits"),
        1,
        "cache was wiped"
    );
}

/// Incremental `index` landing a commit in an already-indexed bucket must
/// leave one sampled commit per bucket, else `burndown --by language`
/// sums two snapshots for that bucket.
#[test]
fn scenario_incremental_index_keeps_one_sample_per_bucket() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    let commit = |date: &str, body: &str| {
        std::fs::write(env.repo.join("a.rs"), body).unwrap();
        git_at(&env, date, &["add", "a.rs"]);
        git_at(&env, date, &["commit", "-q", "-m", "feat: x"]);
    };
    git_at(&env, "2025-01-01T00:00:00Z", &["init", "-q", "-b", "main"]);
    commit("2025-01-01T10:00:00Z", "fn a() {}\n");
    env.arch_stdout(&["index", "--bucket", "day"]);
    commit("2025-01-01T12:00:00Z", "fn a() {}\nfn b() {}\n");
    env.arch_stdout(&["index", "--bucket", "day"]);

    assert_eq!(
        env.sql_i64("SELECT COUNT(*) FROM commits WHERE is_sampled"),
        1
    );
    let out = env.arch_stdout(&["burndown", "--bucket", "day", "--format", "tsv"]);
    let loc: Vec<&str> = out
        .lines()
        .skip(1)
        .map(|l| l.split('\t').nth(2).unwrap())
        .collect();
    assert_eq!(loc, ["2"]);

    // Changing `--bucket` incrementally must match a fresh reindex.
    let plan = "SELECT COUNT(*) FROM commits c JOIN file_stats fs USING (sha) \
                WHERE c.is_sampled AND c.bucket_key = 202501";
    env.arch_stdout(&["index", "--bucket", "month"]);
    let incremental = env.sql_i64(plan);
    env.arch_stdout(&["reindex", "--bucket", "month"]);
    assert_eq!(incremental, env.sql_i64(plan));
    assert_eq!(incremental, 1);
}

/// `stats` counts HEAD's tree: per-language code/comment/blank split,
/// grammar-less files as `other`, dir grouping, and a `Total` row.
#[test]
fn scenario_stats_snapshot() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    one_commit(
        &env,
        &[
            ("src/a.rs", "// c\nfn f() {\n    1;\n}\n\nfn g() {}\n"),
            ("notes.txt", "x\ny\n"),
            ("tests/b.rs", "fn h() {}\n"),
        ],
    );
    env.arch_stdout(&["index"]);
    let rows = |args: &[&str]| -> Vec<Vec<String>> {
        let mut a = vec!["stats", "--format", "tsv"];
        a.extend_from_slice(args);
        env.arch_stdout(&a)
            .lines()
            .skip(1)
            .map(|l| l.split('\t').map(str::to_string).collect())
            .collect()
    };
    // group, files, lines, code, comments, blanks, test_code, test_pct, funcs
    let lang = rows(&[]);
    let head: Vec<Vec<&str>> = lang
        .iter()
        .map(|r| r[..9].iter().map(String::as_str).collect())
        .collect();
    assert_eq!(
        head,
        [
            vec!["Rust", "2", "7", "5", "1", "1", "1", "20", "3"],
            vec!["other", "1", "2", "", "", "", "", "", "0"],
            vec!["Total", "3", "9", "5", "1", "1", "1", "20", "3"],
        ]
    );
    let dirs: Vec<String> = rows(&["--by", "dir"])
        .into_iter()
        .map(|r| r[0].clone())
        .collect();
    assert_eq!(dirs, ["src", ".", "tests", "Total"]);
}

/// `stats --rev`: one non-HEAD commit, and `A..B` as per-group delta B − A
/// (unchanged groups dropped, `Total` kept).
#[test]
fn scenario_stats_rev_delta() {
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
                "feat: a",
                &[("src/a.rs", "fn f() {}\n"), ("lib/c.py", "x = 1\n")],
            ),
            Action::commit(
                "Alice",
                "2025-01-02T00:00:00Z",
                "feat: b",
                &[("src/a.rs", "fn f() {}\n\n#[test]\nfn t() {\n    f();\n}\n")],
            ),
        ],
    );
    let rows = |args: &[&str]| -> Vec<String> {
        let mut a = vec!["stats", "--format", "tsv", "--by", "dir"];
        a.extend_from_slice(args);
        env.arch_stdout(&a)
            .lines()
            .skip(1)
            .map(|l| l.split('\t').take(9).collect::<Vec<_>>().join(" "))
            .collect()
    };
    // dir files lines code comments blanks test_code test_pct funcs
    assert_eq!(
        rows(&["--rev", "HEAD~1"]),
        [
            "lib 1 1 1 0 0 0 0 0",
            "src 1 1 1 0 0 0 0 1",
            "Total 2 2 2 0 0 0 0 1"
        ]
    );
    assert_eq!(
        rows(&["--rev", "HEAD~1..HEAD"]),
        ["src 0 5 4 0 1 4 80 1", "Total 0 5 4 0 1 4 66.7 1"]
    );
    assert_eq!(
        rows(&["--rev", "HEAD~1.."]),
        rows(&["--rev", "HEAD~1..HEAD"])
    );
}

/// Commit `body` to `path` on the current branch through raw git.
fn commit_file(env: &Env, date: &str, path: &str, body: &str, msg: &str) {
    std::fs::write(env.repo.join(path), body).unwrap();
    git_at(env, date, &["add", path]);
    git_at(env, date, &["commit", "-q", "-m", msg]);
}

fn numbered(n: usize) -> String {
    (1..=n).map(|i| format!("line {i}\n")).collect()
}

/// A corrupt cache is rebuilt: DuckDB's checksum error says "in block at",
/// which must not read as the "Could not set lock" of a cache in use.
#[test]
fn scenario_corrupt_cache_is_rebuilt() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    one_commit(&env, &[("a.txt", "1\n")]);
    env.arch_stdout(&["index"]);
    let cache = std::fs::read_dir(env.xdg_data.join("git-archaeologist/caches"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path()
        .join("cache.duckdb");
    let mut bytes = std::fs::read(&cache).unwrap();
    bytes[8192..8192 + 16384].fill(0xAB);
    std::fs::write(&cache, bytes).unwrap();

    assert_eq!(env.sql_i64("SELECT COUNT(*) FROM commits"), 1);
}

/// Known drift: main edits `a.txt` after a branch renamed it, so the fold
/// sees a new file at the old path and HEAD gains a ghost `a.txt`.
#[test]
#[ignore = "known drift: linear fold can't follow a rename across branches"]
fn known_drift_rename_on_branch() {
    let env = Env::new();
    let d = |n: u32| format!("2025-01-0{n}T00:00:00Z");
    git_at(&env, &d(1), &["init", "-q", "-b", "main"]);
    commit_file(&env, &d(1), "a.txt", &numbered(30), "feat: base");
    git_at(&env, &d(2), &["checkout", "-q", "-b", "side"]);
    git_at(&env, &d(2), &["mv", "a.txt", "b.txt"]);
    git_at(&env, &d(2), &["commit", "-q", "-m", "refactor: mv"]);
    git_at(&env, &d(3), &["checkout", "-q", "main"]);
    commit_file(
        &env,
        &d(3),
        "a.txt",
        &format!("top\n{}", numbered(30)),
        "feat: edit",
    );
    git_at(
        &env,
        &d(4),
        &["merge", "-q", "--no-ff", "side", "-m", "merge"],
    );
    env.arch_stdout(&["index"]);
    truth::assert_line_counts(&env.repo, &env.births());
}

/// Known drift: a merge's own content has no hunks, so lines added while
/// resolving it are missing from `line_births`.
#[test]
#[ignore = "known drift: merge commits are not diffed"]
fn known_drift_evil_merge() {
    let env = Env::new();
    let d = |n: u32| format!("2025-01-0{n}T00:00:00Z");
    git_at(&env, &d(1), &["init", "-q", "-b", "main"]);
    commit_file(&env, &d(1), "f.txt", &numbered(5), "feat: base");
    git_at(&env, &d(2), &["checkout", "-q", "-b", "side"]);
    commit_file(&env, &d(2), "g.txt", "g\n", "feat: side");
    git_at(&env, &d(3), &["checkout", "-q", "main"]);
    commit_file(&env, &d(3), "h.txt", "h\n", "feat: main");
    git_at(
        &env,
        &d(4),
        &["merge", "-q", "--no-ff", "--no-commit", "side"],
    );
    std::fs::write(env.repo.join("f.txt"), numbered(9)).unwrap();
    git_at(&env, &d(4), &["add", "f.txt"]);
    git_at(&env, &d(4), &["commit", "-q", "-m", "merge side"]);
    env.arch_stdout(&["index"]);
    truth::assert_line_counts(&env.repo, &env.births());
}
