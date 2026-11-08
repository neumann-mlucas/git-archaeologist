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

    /// Data rows of a TSV report, header dropped.
    fn rows(&self, args: &[&str]) -> Vec<String> {
        let mut a = args.to_vec();
        a.extend(["--format", "tsv"]);
        self.arch_stdout(&a)
            .lines()
            .skip(1)
            .map(str::to_string)
            .collect()
    }

    /// stderr of a run that must succeed.
    fn stderr_of(&self, args: &[&str]) -> String {
        let out = self.arch(args);
        assert!(out.status.success(), "arch {args:?} failed");
        String::from_utf8_lossy(&out.stderr).into_owned()
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
    git_as(env, ("Bob", "bob@example.com"), date, args)
}

/// Raw git as `(name, email)` at `date`.
fn git_as(env: &Env, who: (&str, &str), date: &str, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(&env.repo)
        .env("GIT_AUTHOR_NAME", who.0)
        .env("GIT_AUTHOR_EMAIL", who.1)
        .env("GIT_COMMITTER_NAME", who.0)
        .env("GIT_COMMITTER_EMAIL", who.1)
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

/// `stats --rev` across adds, deletes, a rename and a grammar-less file:
/// one-sided groups in the delta, negative deltas, empty-left `..B`,
/// `--by lang|file`, filters, the `--rev HEAD` fast path, bad revs.
#[test]
fn scenario_stats_rev_add_delete_rename() {
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
                "feat: base",
                &[
                    ("src/a.rs", "fn f() {}\n"),
                    ("lib/c.py", "x = 1\n"),
                    ("notes.txt", "hi\nthere\n"),
                ],
            ),
            Action::commit(
                "Alice",
                "2025-01-02T00:00:00Z",
                "feat: add b",
                &[("src/b.rs", "fn g() {}\nfn h() {}\n")],
            ),
            Action::delete(
                "Alice",
                "2025-01-03T00:00:00Z",
                "chore: drop c",
                &["lib/c.py"],
            ),
            Action::rename(
                "Alice",
                "2025-01-04T00:00:00Z",
                "refactor: move a",
                "src/a.rs",
                "core/a.rs",
            ),
        ],
    );
    let rows = |args: &[&str]| -> Vec<String> {
        let mut a = vec!["stats", "--format", "tsv"];
        a.extend_from_slice(args);
        env.arch_stdout(&a)
            .lines()
            .skip(1)
            .map(|l| l.split('\t').take(9).collect::<Vec<_>>().join(" "))
            .collect()
    };
    // name files lines code comments blanks test_code test_pct funcs
    // One-sided groups: `core` only at HEAD, `lib` only at HEAD~3.
    assert_eq!(
        rows(&["--by", "dir", "--rev", "HEAD~3..HEAD"]),
        [
            "core 1 1 1 0 0 0 0 1",
            "lib -1 -1 -1 0 0 0 0 0",
            "src 0 1 1 0 0 0 0 1",
            "Total 0 1 1 0 0 0 0 2"
        ]
    );
    // Backwards range negates; empty left side means HEAD.
    let back = [
        "core -1 -1 -1 0 0 0 0 -1",
        "lib 1 1 1 0 0 0 0 0",
        "src 0 -1 -1 0 0 0 0 -1",
        "Total 0 -1 -1 0 0 0 0 -2",
    ];
    assert_eq!(rows(&["--by", "dir", "--rev", "HEAD..HEAD~3"]), back);
    assert_eq!(rows(&["--by", "dir", "--rev", "..HEAD~3"]), back);
    assert_eq!(
        rows(&["--by", "lang", "--rev", "HEAD~3..HEAD"]),
        [
            "Rust 1 2 2 0 0 0 0 2",
            "Python -1 -1 -1 0 0 0 0 0",
            "Total 0 1 1 0 0 0 0 2"
        ]
    );
    // A rename is a delete + add between snapshots.
    assert_eq!(
        rows(&["--by", "file", "--rev", "HEAD~3..HEAD"]),
        [
            "src/b.rs 1 2 2 0 0 0 0 2",
            "core/a.rs 1 1 1 0 0 0 0 1",
            "lib/c.py -1 -1 -1 0 0 0 0 0",
            "src/a.rs -1 -1 -1 0 0 0 0 -1",
            "Total 0 1 1 0 0 0 0 2"
        ]
    );
    assert_eq!(
        rows(&["--by", "lang", "--rev", "HEAD~3..HEAD", "--lang", "python"]),
        ["Python -1 -1 -1 0 0 0 0 0", "Total -1 -1 -1 0 0 0 0 0"]
    );
    assert_eq!(
        rows(&["--by", "dir", "--rev", "HEAD~3..HEAD", "--path", "src/"]),
        ["src 0 1 1 0 0 0 0 1", "Total 0 1 1 0 0 0 0 1"]
    );
    assert_eq!(
        rows(&["--by", "dir", "--rev", "HEAD..HEAD"]),
        ["Total 0 0 0 0 0 0 0 0"]
    );
    // Grammar-less `notes.txt` is absent from a `--rev` snapshot but shows
    // in HEAD stats (dir `.`), so Totals differ for the same tree.
    assert_eq!(
        rows(&["--by", "dir", "--rev", "HEAD~3"]),
        [
            "lib 1 1 1 0 0 0 0 0",
            "src 1 1 1 0 0 0 0 1",
            "Total 2 2 2 0 0 0 0 1"
        ]
    );
    let head = [
        ". 1 2      0",
        "src 1 2 2 0 0 0 0 2",
        "core 1 1 1 0 0 0 0 1",
        "Total 3 5 3 0 0 0 0 3",
    ];
    assert_eq!(rows(&["--by", "dir"]), head);
    // `--rev HEAD` takes the full-stats path, not the parse-only one.
    assert_eq!(rows(&["--by", "dir", "--rev", "HEAD"]), head);

    for (bad, msg) in [
        ("HEAD~1...HEAD", "--rev takes A..B, not A...B"),
        ("nope", "resolving \"nope\""),
        ("nope..HEAD", "resolving \"nope\""),
    ] {
        let out = env.arch(&["stats", "--rev", bad]);
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(!out.status.success(), "{bad} must fail");
        assert!(err.contains(msg), "{bad}: {err}");
    }
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

/// Commits sharing a timestamp (rebase, scripted imports) fold parent-first.
/// Sorting by (time, sha) deletes `g.txt` before creating it, leaving a
/// ghost file alive at HEAD.
#[test]
fn scenario_same_second_commits_fold_in_parent_order() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    let at = "2025-01-01T00:00:00Z";
    let bodies: Vec<String> = (1..=6).map(|i| numbered(i * 3)).collect();
    let mut actions: Vec<Action> = bodies
        .iter()
        .map(|b| Action::commit("Alice", at, "feat: grow", &[("f.txt", b)]))
        .collect();
    actions.push(Action::commit("Alice", at, "feat: g", &[("g.txt", "g\n")]));
    actions.push(Action::delete("Alice", at, "chore: drop g", &["g.txt"]));
    dsl::build(&env.repo, &actions);
    let delete = git_at(&env, at, &["rev-parse", "HEAD"]);
    let add = git_at(&env, at, &["rev-parse", "HEAD~1"]);
    assert!(delete < add, "fixture no longer sorts the delete first");
    env.arch_stdout(&["index"]);
    env.assert_matches_git();
}

/// Clock skew: a child dated before its parent still folds after it.
#[test]
fn scenario_child_older_than_parent_folds_after_it() {
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
                "2025-01-03T00:00:00Z",
                "feat: a",
                &[("f.txt", "a\n")],
            ),
            Action::commit(
                "Alice",
                "2025-01-02T00:00:00Z",
                "feat: b",
                &[("f.txt", "a\nb\n")],
            ),
            Action::commit(
                "Alice",
                "2025-01-01T00:00:00Z",
                "feat: c",
                &[("f.txt", "a\nb\nc\n")],
            ),
        ],
    );
    env.arch_stdout(&["index"]);
    env.assert_matches_git();
}

/// Rewritten history: the amended-away commit leaves every table, its
/// bucket keeps one sample, and incremental `index` agrees with `reindex`.
#[test]
fn scenario_amended_commit_leaves_no_orphans() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    git_at(&env, "2025-01-01T00:00:00Z", &["init", "-q", "-b", "main"]);
    commit_file(
        &env,
        "2025-01-01T10:00:00Z",
        "a.rs",
        "fn a() {}\n",
        "feat: one",
    );
    commit_file(
        &env,
        "2025-01-01T11:00:00Z",
        "a.rs",
        "fn a() {}\nfn b() {}\n",
        "feat: two",
    );
    let gone = git_at(&env, "2025-01-01T11:00:00Z", &["rev-parse", "HEAD"]);
    env.arch_stdout(&["index", "--bucket", "day"]);

    std::fs::write(env.repo.join("a.rs"), "fn a() {}\nfn c() {}\nfn d() {}\n").unwrap();
    git_at(&env, "2025-01-01T12:00:00Z", &["add", "a.rs"]);
    git_at(
        &env,
        "2025-01-01T12:00:00Z",
        &["commit", "-q", "--amend", "-m", "feat: two amended"],
    );
    env.arch_stdout(&["index", "--bucket", "day"]);

    for table in [
        "commits",
        "commit_parents",
        "commit_trailers",
        "tags",
        "hunks",
        "file_churn",
        "file_stats",
        "funcs",
    ] {
        assert_eq!(
            env.sql_i64(&format!(
                "SELECT COUNT(*) FROM {table} WHERE sha = '{gone}'"
            )),
            0,
            "orphan rows left in {table}"
        );
    }
    assert_eq!(
        env.sql_i64("SELECT COUNT(*) FROM commits WHERE is_sampled"),
        1
    );
    let state = || {
        env.arch_stdout(&[
            "sql",
            "SELECT c.sha, c.bucket_key, c.is_sampled, \
                    (SELECT SUM(added) FROM file_churn f WHERE f.sha = c.sha) AS added, \
                    (SELECT COUNT(*) FROM file_stats s WHERE s.sha = c.sha) AS stats \
             FROM commits c ORDER BY c.sha",
            "--format",
            "tsv",
        ])
    };
    let incremental = state();
    env.arch_stdout(&["reindex", "--bucket", "day"]);
    assert_eq!(incremental, state());
    env.assert_matches_git();
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

/// `age` bins floor: 4 days old is week 0, 8 days old is week 1.
#[test]
fn scenario_age_buckets_floor() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs();
    let days_ago = |d: u64| format!("{} +0000", now - d * 86400);
    dsl::build(
        &env.repo,
        &[
            Action::commit("Alice", &days_ago(8), "feat: old", &[("old.txt", "1\n")]),
            Action::commit("Alice", &days_ago(4), "feat: new", &[("new.txt", "1\n")]),
        ],
    );
    env.arch_stdout(&["index"]);
    let out = env.arch_stdout(&["age", "--format", "tsv"]);
    assert_eq!(
        out.lines().skip(1).collect::<Vec<_>>(),
        ["0\t1", "7\t1"],
        "got:\n{out}"
    );
}

/// A move of 40 files, each also edited, keeps the unedited lines' births.
/// Past gix's default limit of 1000 source x destination pairs these fall
/// back to add + delete, and every line looks born at the move.
#[test]
fn scenario_mass_rename_with_edits_keeps_births() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    let body = |i: usize, last: &str| {
        let lines: Vec<String> = (0..9).map(|j| format!("file {i} line {j}")).collect();
        format!("{}\n{last}\n", lines.join("\n"))
    };
    let files: Vec<(String, String)> = (0..40)
        .map(|i| (format!("old/f{i}.txt"), body(i, "before")))
        .collect();
    let writes: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, b)| (p.as_str(), b.as_str()))
        .collect();
    one_commit(&env, &writes);
    for i in 0..40 {
        std::fs::write(env.repo.join(format!("old/f{i}.txt")), body(i, "after")).unwrap();
    }
    dsl::build(
        &env.repo,
        &[Action::rename(
            "Alice",
            "2025-01-02T00:00:00Z",
            "refactor: move",
            "old",
            "new",
        )],
    );
    env.arch_stdout(&["index"]);
    env.assert_matches_git();
}

/// A commit after `index` makes every report warn on stderr until the next
/// `index`, while the output itself stays the cached answer.
#[test]
fn scenario_moved_head_warns_until_reindexed() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    one_commit(&env, &[("a.txt", "1\n")]);
    let warned = |args: &[&str]| {
        let out = env.arch(args);
        assert!(out.status.success(), "arch {args:?} failed");
        String::from_utf8_lossy(&out.stderr).contains("different HEAD")
    };
    env.arch_stdout(&["index"]);
    assert!(!warned(&["age"]), "fresh cache warned");

    dsl::build(
        &env.repo,
        &[Action::commit(
            "Alice",
            "1700000000 +0000",
            "feat: more",
            &[("b.txt", "1\n")],
        )],
    );
    for args in [&["age"][..], &["stats"], &["sql", "SELECT 1"]] {
        assert!(warned(args), "{args:?} didn't warn");
    }

    env.arch_stdout(&["index"]);
    assert!(!warned(&["age"]), "reindexed cache warned");
}

/// Minified JS repeats a name on one line, which `funcs` can't key apart:
/// `index` must keep one row instead of aborting on the primary key.
#[test]
fn scenario_repeated_name_on_one_line_indexes() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    one_commit(&env, &[("m.js", "function t(){}function t(){}\n")]);
    env.arch_stdout(&["index"]);
    assert_eq!(
        env.sql_i64("SELECT COUNT(*) FROM funcs WHERE name = 't'"),
        1
    );
}

/// A file leaving text (binary, over the size cap, symlink) loses its
/// lines, and coming back adds them, instead of the fold keeping stale ones.
#[test]
fn scenario_text_to_non_text_transitions_match_git() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    let d = |n: u32| format!("2025-01-0{n}T00:00:00Z");
    git_at(&env, &d(1), &["init", "-q", "-b", "main"]);
    for f in ["binary.txt", "big.txt", "link.txt", "keep.txt"] {
        std::fs::write(env.repo.join(f), format!("{f} 1\n{f} 2\n")).unwrap();
    }
    git_at(&env, &d(1), &["add", "-A"]);
    git_at(&env, &d(1), &["commit", "-q", "-m", "feat: text"]);

    std::fs::write(env.repo.join("binary.txt"), "a\0b\n").unwrap();
    std::fs::write(env.repo.join("big.txt"), "big\n".repeat(600_000)).unwrap();
    std::fs::remove_file(env.repo.join("link.txt")).unwrap();
    std::os::unix::fs::symlink("keep.txt", env.repo.join("link.txt")).unwrap();
    git_at(&env, &d(2), &["add", "-A"]);
    git_at(&env, &d(2), &["commit", "-q", "-m", "feat: not text"]);

    std::fs::remove_file(env.repo.join("binary.txt")).unwrap();
    std::fs::write(env.repo.join("big.txt"), "small\n").unwrap();
    git_at(&env, &d(3), &["add", "-A"]);
    git_at(&env, &d(3), &["commit", "-q", "-m", "feat: back"]);

    env.arch_stdout(&["index"]);
    env.assert_matches_git();
}

/// Main edits `a.txt` after a branch renamed it: the merge's diff against
/// main carries the rename, so no ghost `a.txt` survives.
#[test]
fn scenario_rename_on_branch_matches_git() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
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

/// Lines written while resolving a merge exist only in the merge's diff.
#[test]
fn scenario_evil_merge_matches_git() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
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

/// Branch lines are born at the merge, and `survival` measures them there:
/// no cohort outlives its births, and the branch commit's bucket is gone.
#[test]
fn scenario_branch_lines_are_born_at_merge() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    let d = |n: u32| format!("2025-01-0{n}T00:00:00Z");
    git_at(&env, &d(1), &["init", "-q", "-b", "main"]);
    commit_file(&env, &d(1), "f.txt", &numbered(2), "feat: base");
    git_at(&env, &d(2), &["checkout", "-q", "-b", "side"]);
    commit_file(&env, &d(2), "g.txt", "g1\ng2\n", "feat: side");
    let branch = git_at(&env, &d(2), &["rev-parse", "HEAD"]);
    git_at(&env, &d(3), &["checkout", "-q", "main"]);
    commit_file(&env, &d(3), "h.txt", "h\n", "feat: main");
    git_at(
        &env,
        &d(4),
        &["merge", "-q", "--no-ff", "side", "-m", "merge"],
    );
    let merge = git_at(&env, &d(4), &["rev-parse", "HEAD"]);
    env.arch_stdout(&["index", "--bucket", "commit"]);

    truth::assert_line_counts(&env.repo, &env.births());
    assert_eq!(
        env.sql_i64(&format!(
            "SELECT COUNT(*) FROM line_births WHERE path = 'g.txt' AND birth_sha = '{merge}'"
        )),
        2
    );
    assert_eq!(
        env.sql_i64(&format!(
            "SELECT COUNT(*) FROM chain_births WHERE sha = '{branch}'"
        )),
        0
    );

    let out = env.arch_stdout(&["survival", "--format", "tsv"]);
    let rows: Vec<Vec<&str>> = out
        .lines()
        .skip(1)
        .map(|l| l.split('\t').collect())
        .collect();
    assert_eq!(rows.len(), 3, "base, main, merge cohorts:\n{out}");
    for r in rows {
        assert_eq!(r[1], r[2], "all lines alive, so born == alive:\n{out}");
    }
}

// ---- query outputs on controlled histories ----

const T0: i64 = 1_735_689_600; // 2025-01-01T00:00:00Z
const DAY: i64 = 86_400;

/// Unix-seconds date for `git_at` and friends.
fn at(ts: i64) -> String {
    format!("{ts} +0000")
}

/// Stage everything and commit at `ts`.
fn commit_all(env: &Env, ts: i64, msg: &str) {
    git_at(env, &at(ts), &["add", "-A"]);
    git_at(env, &at(ts), &["commit", "-q", "--allow-empty", "-m", msg]);
}

fn write(env: &Env, files: &[(&str, &str)]) {
    for (p, body) in files {
        let full = env.repo.join(p);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, body).unwrap();
    }
}

/// Fresh repo on `main`, no commits.
fn init(env: &Env) {
    git_at(env, &at(T0), &["init", "-q", "-b", "main"]);
}

fn sorted(mut v: Vec<String>) -> Vec<String> {
    v.sort();
    v
}

/// Kaplan-Meier rows: born / alive / dead per birth cohort, and the fit
/// refusing to run on too few deaths.
#[test]
fn scenario_survival_counts_deaths_per_cohort() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    write(&env, &[("f.txt", "a\nb\nc\nd\n")]);
    commit_all(&env, T0, "feat: f");
    write(&env, &[("f.txt", "a\nc\nnew\n")]);
    commit_all(&env, T0 + DAY, "fix: f");
    write(&env, &[("g.txt", "g\n")]);
    commit_all(&env, T0 + 2 * DAY, "feat: g");
    env.arch_stdout(&["index", "--bucket", "commit"]);

    assert_eq!(
        env.rows(&["survival"]),
        [
            format!("{T0}\t4\t2\t2\t0.5"),
            format!("{}\t1\t1\t0\t1", T0 + DAY),
            format!("{}\t1\t1\t0\t1", T0 + 2 * DAY),
        ]
    );
    assert_eq!(
        env.rows(&["survival", "--fit", "exp"]),
        ["half_life\t\tinsufficient data: 2 deletion events (need >= 100)"]
    );
}

/// With enough deaths the fit returns a positive half-life.
#[test]
fn scenario_survival_fit_returns_half_life() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    // Two partly-dead cohorts: the older one keeps less.
    let tail = |n: usize, skip: usize| -> String {
        numbered(n)
            .lines()
            .skip(skip)
            .map(|l| format!("{l}\n"))
            .collect()
    };
    write(&env, &[("f.txt", &numbered(300))]);
    commit_all(&env, T0, "feat: f");
    write(&env, &[("g.txt", &numbered(200))]);
    commit_all(&env, T0 + 10 * DAY, "feat: g");
    write(
        &env,
        &[("f.txt", &tail(300, 150)), ("g.txt", &tail(200, 50))],
    );
    commit_all(&env, T0 + 20 * DAY, "fix: trim");
    env.arch_stdout(&["index", "--bucket", "commit"]);

    let rows = env.rows(&["survival", "--fit", "exp"]);
    let cols: Vec<&str> = rows[0].split('\t').collect();
    assert_eq!(cols[0], "half_life", "{rows:?}");
    let days: f64 = cols[1]
        .parse()
        .unwrap_or_else(|_| panic!("no value: {rows:?}"));
    // s = 0.5 at 20 days, 0.75 at 10: ln 2 / (ln 1.5 / 10) = 17.1 days.
    assert!((days - 17.1).abs() < 0.1, "half-life not in days: {rows:?}");
}

/// Co-commit counts, and `--max-files-per-commit` dropping wide commits.
#[test]
fn scenario_coupling_counts_and_caps_wide_commits() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    write(&env, &[("a.txt", "1\n"), ("b.txt", "1\n")]);
    commit_all(&env, T0, "feat: ab");
    write(
        &env,
        &[("a.txt", "2\n"), ("b.txt", "2\n"), ("c.txt", "1\n")],
    );
    commit_all(&env, T0 + DAY, "feat: abc");
    let wide: Vec<(String, String)> = ["a", "b", "c", "d", "e"]
        .iter()
        .map(|f| (format!("{f}.txt"), "3\n".to_string()))
        .collect();
    let wide: Vec<(&str, &str)> = wide.iter().map(|(p, b)| (p.as_str(), b.as_str())).collect();
    write(&env, &wide);
    commit_all(&env, T0 + 2 * DAY, "chore: wide");
    env.arch_stdout(&["index"]);

    let top = env.rows(&["coupling", "--top", "1"]);
    assert_eq!(top, ["a.txt\tb.txt\t3"]);
    let capped = env.rows(&["coupling", "--top", "10", "--max-files-per-commit", "4"]);
    assert_eq!(
        sorted(capped),
        ["a.txt\tb.txt\t2", "a.txt\tc.txt\t1", "b.txt\tc.txt\t1"]
    );
}

/// A hunk is credited to the function whose span holds its first line.
#[test]
fn scenario_hotspot_credits_the_enclosing_fn() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    write(
        &env,
        &[("lib.rs", "fn a() {\n    1;\n}\n\nfn b() {\n    2;\n}\n")],
    );
    commit_all(&env, T0, "feat: lib");
    write(
        &env,
        &[("lib.rs", "fn a() {\n    1;\n}\n\nfn b() {\n    3;\n}\n")],
    );
    commit_all(&env, T0 + DAY, "fix: b");
    env.arch_stdout(&["index", "--bucket", "commit"]);

    let rows = env.rows(&["hotspot", "--lang", "rust"]);
    let t1 = T0 + DAY;
    assert!(
        rows.contains(&format!("{t1}\tlib.rs\tb\tfn\tRust\t1\t1\t2")),
        "{rows:?}"
    );
    assert!(
        !rows
            .iter()
            .any(|r| r.starts_with(&format!("{t1}\tlib.rs\ta\t"))),
        "edit inside b credited to a: {rows:?}"
    );
}

/// `churn --depth N` groups by the first N segments; shorter paths keep
/// their full path.
#[test]
fn scenario_churn_depth_groups_paths() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    write(
        &env,
        &[
            ("src/x/a.rs", "1\n"),
            ("src/y/b.rs", "1\n2\n"),
            ("src/c.rs", "1\n"),
            ("top.txt", "1\n"),
        ],
    );
    commit_all(&env, T0, "feat: init");
    env.arch_stdout(&["index", "--bucket", "commit"]);

    let module = |depth: &str| sorted(env.rows(&["churn", "--by", "module", "--depth", depth]));
    assert_eq!(
        module("2"),
        [
            format!("{T0}\tsrc/c.rs\t1\t0"),
            format!("{T0}\tsrc/x\t1\t0"),
            format!("{T0}\tsrc/y\t2\t0"),
            format!("{T0}\ttop.txt\t1\t0"),
        ]
    );
    assert_eq!(
        module("1"),
        [format!("{T0}\tsrc\t4\t0"), format!("{T0}\ttop.txt\t1\t0")]
    );
}

/// `burndown --by kind` splits test code (test path or test region) from
/// the rest.
#[test]
fn scenario_burndown_by_kind_splits_tests() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    write(
        &env,
        &[
            (
                "src/lib.rs",
                "fn f() {}\n#[cfg(test)]\nmod t {\n    #[test]\n    fn x() {}\n}\n",
            ),
            ("tests/it.rs", "fn y() {}\n"),
            ("config.yaml", "a: 1\n"),
        ],
    );
    commit_all(&env, T0, "feat: init");
    env.arch_stdout(&["index", "--bucket", "commit"]);
    assert_eq!(
        sorted(env.rows(&["burndown", "--by", "kind"])),
        [format!("{T0}\tcode\t2"), format!("{T0}\ttest\t6")]
    );
}

/// Conventional Commit parsing on the subjects real repos produce.
#[test]
fn scenario_classify_edge() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    let msgs = [
        "feat(scope)!: x",
        "fix: y\n\nBREAKING CHANGE: z",
        "Revert \"feat(scope)!: x\"\n\nThis reverts commit abc.",
        "revert: y",
        "fixup! fix: y",
        "Feat: Caps",
        "docs: d",
        "random words",
        "feat:tight",
    ];
    for (i, m) in msgs.iter().enumerate() {
        write(&env, &[("f.txt", &numbered(i + 1))]);
        commit_all(&env, T0 + i as i64 * 60, m);
    }
    env.arch_stdout(&["index", "--bucket", "month"]);
    let got = env.rows(&[
        "sql",
        "SELECT split_part(message, chr(10), 1), coalesce(msg_type, '-'), \
                is_breaking, is_revert \
         FROM commits ORDER BY authored_at",
    ]);
    assert_eq!(
        got,
        [
            "feat(scope)!: x\tfeat\ttrue\tfalse",
            "fix: y\tfix\ttrue\tfalse",
            "Revert \"feat(scope)!: x\"\t-\tfalse\ttrue",
            "revert: y\trevert\tfalse\ttrue",
            "fixup! fix: y\t-\tfalse\tfalse",
            "Feat: Caps\tfeat\tfalse\tfalse",
            "docs: d\tdocs\tfalse\tfalse",
            "random words\t-\tfalse\tfalse",
            "feat:tight\tfeat\tfalse\tfalse",
        ]
    );
    // commits fixes feats reverts breaking untyped
    assert_eq!(env.rows(&["classify"]), ["202501\t9\t1\t3\t2\t2\t3"]);
}

// ---- bucketing and dates ----

/// Every bucket size keys the same commits as documented, and one commit
/// per bucket is sampled. 2024-12-30 sits in ISO week 2025-W01.
#[test]
fn scenario_bucket_keys_per_size() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    let (dec30, jan2, feb10) = (T0 - 2 * DAY + 43_200, T0 + DAY, T0 + 40 * DAY);
    for (i, ts) in [dec30, jan2, feb10].into_iter().enumerate() {
        write(&env, &[("f.txt", &numbered(i + 1))]);
        commit_all(&env, ts, "feat: f");
        if i == 1 {
            git_at(&env, &at(ts), &["tag", "v1"]);
        }
    }
    let keys = |bucket: &str| {
        env.arch_stdout(&["reindex", "--bucket", bucket]);
        let k = env.rows(&["sql", "SELECT DISTINCT bucket_key FROM commits ORDER BY 1"]);
        let sampled = env.sql_i64("SELECT COUNT(*) FROM commits WHERE is_sampled");
        assert_eq!(sampled as usize, k.len(), "{bucket}: one sample per bucket");
        k
    };
    assert_eq!(keys("day"), ["20241230", "20250102", "20250210"]);
    assert_eq!(keys("week"), ["202501", "202507"]);
    assert_eq!(keys("month"), ["202412", "202501", "202502"]);
    assert_eq!(
        keys("commit"),
        [dec30.to_string(), jan2.to_string(), feb10.to_string()]
    );
    assert_eq!(keys("tag").len(), 2, "one bucket up to v1, one after");

    let warn = env.stderr_of(&["classify", "--bucket", "month"]);
    assert!(
        warn.contains("bucketed by tag, not month"),
        "no bucket mismatch warning: {warn}"
    );
    assert!(!env
        .stderr_of(&["classify", "--bucket", "tag"])
        .contains("bucketed by"));
}

/// `--from` / `--to` are inclusive UTC days; a +05:00 commit on Feb 1
/// local time is Jan 31 in UTC.
#[test]
fn scenario_date_filters_are_inclusive_utc_days() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    let dates = [
        "2025-01-01T00:00:00Z",
        "2025-01-31T23:59:59Z",
        "2025-02-01T01:00:00+05:00",
        "2025-02-01T00:00:00Z",
    ];
    for (i, d) in dates.iter().enumerate() {
        write(&env, &[("f.txt", &numbered(i + 1))]);
        git_at(&env, d, &["add", "-A"]);
        git_at(&env, d, &["commit", "-q", "-m", "feat: f"]);
    }
    env.arch_stdout(&["index", "--bucket", "month"]);
    let commits = |args: &[&str]| -> i64 {
        let mut a = vec!["classify"];
        a.extend(args);
        env.rows(&a)
            .iter()
            .map(|r| r.split('\t').nth(1).unwrap().parse::<i64>().unwrap())
            .sum()
    };
    assert_eq!(commits(&["--from", "2025-01-01", "--to", "2025-01-31"]), 3);
    assert_eq!(commits(&["--from", "2025-02-01"]), 1);
    assert_eq!(commits(&["--to", "2025-01-01"]), 1);
}

// ---- authors ----

/// `.mailmap` folds two emails into one author; trailers with unicode,
/// quoted commas and a sign-off chain land in `commit_trailers` under
/// canonical identities.
#[test]
fn scenario_authors_mailmap_and_trailers() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    write(
        &env,
        &[(".mailmap", "Alice <alice@new.org> <alice@old.org>\n")],
    );
    git_as(&env, ("alice", "alice@old.org"), &at(T0), &["add", "-A"]);
    git_as(
        &env,
        ("alice", "alice@old.org"),
        &at(T0),
        &["commit", "-q", "-m", "feat: map"],
    );
    write(&env, &[("f.txt", "1\n")]);
    let msg = "feat: pair\n\n\
               Co-authored-by: \"Doe, John\" <j@x.org>\n\
               Co-authored-by: Åsa Ölund <asa@ø.no>\n\
               Signed-off-by: Bob <bob@x.org>\n\
               Signed-off-by: alice <alice@old.org>\n";
    let who = ("Alice", "alice@new.org");
    git_as(&env, who, &at(T0 + DAY), &["add", "-A"]);
    git_as(&env, who, &at(T0 + DAY), &["commit", "-q", "-m", msg]);
    env.arch_stdout(&["index"]);

    assert_eq!(
        env.rows(&[
            "sql",
            "SELECT canonical_name, canonical_email FROM authors ORDER BY 1"
        ]),
        [
            "Alice\talice@new.org",
            "Bob\tbob@x.org",
            "Doe, John\tj@x.org",
            "Åsa Ölund\tasa@ø.no",
        ]
    );
    assert_eq!(
        env.rows(&[
            "sql",
            "SELECT a.canonical_name, t.role FROM commit_trailers t \
             JOIN authors a ON a.id = t.author_id ORDER BY 1, 2",
        ]),
        [
            "Alice\tsignoff",
            "Bob\tsignoff",
            "Doe, John\tcoauthor",
            "Åsa Ölund\tcoauthor",
        ]
    );
    assert_eq!(
        env.rows(&["churn", "--by", "author"])
            .iter()
            .map(|r| r.split('\t').nth(1).unwrap().to_string())
            .collect::<std::collections::BTreeSet<_>>(),
        ["Alice".to_string()].into()
    );
}

/// `.git-blame-ignore-revs` (comments, blanks, uppercase sha) flags exactly
/// the listed commit.
#[test]
fn scenario_blame_ignore_revs_flags_listed_commit() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    write(&env, &[("f.txt", "1\n")]);
    commit_all(&env, T0, "feat: f");
    write(&env, &[("f.txt", "  1\n")]);
    commit_all(&env, T0 + DAY, "style: reformat");
    let fmt = git_at(&env, &at(T0), &["rev-parse", "HEAD"]);
    write(
        &env,
        &[(
            ".git-blame-ignore-revs",
            &format!("# formatting\n\n{}\n", fmt.to_uppercase()),
        )],
    );
    commit_all(&env, T0 + 2 * DAY, "chore: ignore fmt");
    env.arch_stdout(&["index"]);
    assert_eq!(
        env.rows(&["sql", "SELECT sha FROM commits WHERE ignored_blame"]),
        [fmt]
    );
}

// ---- unusual files ----

/// CRLF, no trailing newline, empty file, unicode and spaces in paths,
/// a case-only rename and a gitlink all fold the way git sees them.
#[test]
fn scenario_odd_files_match_git() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    git_at(&env, &at(T0), &["config", "core.autocrlf", "false"]);
    write(
        &env,
        &[
            ("crlf.txt", "a\r\nb\r\n\r\nc\r\n"),
            ("nonl.txt", "a\nb"),
            ("empty.txt", ""),
            ("dir with space/naïve ü.txt", "x\ny\n"),
            ("Case.txt", "1\n2\n"),
        ],
    );
    commit_all(&env, T0, "feat: odd");
    git_at(&env, &at(T0 + DAY), &["mv", "Case.txt", "case.txt"]);
    write(&env, &[("nonl.txt", "a\nb\nc")]);
    let blob = git_at(&env, &at(T0), &["rev-parse", "HEAD"]);
    git_at(
        &env,
        &at(T0 + DAY),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{blob},sub"),
        ],
    );
    commit_all(&env, T0 + DAY, "chore: rename, gitlink");
    env.arch_stdout(&["index"]);

    env.assert_matches_git();
    truth::assert_line_counts(&env.repo, &env.births());
    assert_eq!(
        env.sql_i64("SELECT COUNT(*) FROM line_births WHERE path = 'sub'"),
        0,
        "gitlink has no lines"
    );
    assert_eq!(
        env.rows(&[
            "sql",
            "SELECT code, blanks FROM file_stats fs JOIN commits c USING (sha) \
             WHERE path = 'crlf.txt' AND c.authored_at = (SELECT MAX(authored_at) FROM commits)",
        ]),
        Vec::<String>::new(),
        "no grammar for .txt, so no file_stats row"
    );
}

/// A path that turns from a file into a directory (and back) matches git.
#[test]
fn scenario_file_becomes_directory_matches_git() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    write(&env, &[("a", "1\n2\n")]);
    commit_all(&env, T0, "feat: file");
    std::fs::remove_file(env.repo.join("a")).unwrap();
    write(&env, &[("a/inner.txt", "3\n")]);
    commit_all(&env, T0 + DAY, "refactor: dir");
    std::fs::remove_dir_all(env.repo.join("a")).unwrap();
    write(&env, &[("a", "4\n")]);
    commit_all(&env, T0 + 2 * DAY, "refactor: file again");
    env.arch_stdout(&["index"]);
    env.assert_matches_git();
}

/// A move plus a rewrite past git's similarity threshold is a delete and
/// an add, for git and for us.
#[test]
fn scenario_rewrite_past_rename_threshold_matches_git() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    write(&env, &[("a.txt", &numbered(20))]);
    commit_all(&env, T0, "feat: a");
    std::fs::remove_file(env.repo.join("a.txt")).unwrap();
    let body: String = (1..=20)
        .map(|i| {
            if i <= 2 {
                format!("line {i}\n")
            } else {
                format!("new {i}\n")
            }
        })
        .collect();
    write(&env, &[("b.txt", &body)]);
    commit_all(&env, T0 + DAY, "refactor: rewrite");
    env.arch_stdout(&["index"]);
    env.assert_matches_git();
}

// ---- unusual history ----

/// Octopus merge: every branch's lines are born at the merge.
#[test]
fn scenario_octopus_merge_matches_git() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    write(&env, &[("base.txt", "b\n")]);
    commit_all(&env, T0, "feat: base");
    for (i, br) in ["x", "y", "z"].iter().enumerate() {
        git_at(&env, &at(T0), &["checkout", "-q", "-b", br, "main"]);
        write(&env, &[(&format!("{br}.txt"), &numbered(i + 2))]);
        commit_all(&env, T0 + (i as i64 + 1) * DAY, &format!("feat: {br}"));
    }
    git_at(&env, &at(T0), &["checkout", "-q", "main"]);
    git_at(
        &env,
        &at(T0 + 5 * DAY),
        &["merge", "-q", "--no-ff", "x", "y", "z", "-m", "octopus"],
    );
    let merge = git_at(&env, &at(T0), &["rev-parse", "HEAD"]);
    env.arch_stdout(&["index"]);

    truth::assert_line_counts(&env.repo, &env.births());
    assert_eq!(
        env.sql_i64(&format!(
            "SELECT COUNT(*) FROM line_births WHERE birth_sha = '{merge}'"
        )),
        2 + 3 + 4
    );
}

/// Criss-cross merges (each branch merges the other) fold to HEAD's lines.
#[test]
fn scenario_criss_cross_merge_matches_git() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    write(&env, &[("f.txt", &numbered(10))]);
    commit_all(&env, T0, "feat: base");
    git_at(&env, &at(T0), &["branch", "side"]);
    write(&env, &[("m.txt", "m\n")]);
    commit_all(&env, T0 + DAY, "feat: main");
    git_at(&env, &at(T0), &["checkout", "-q", "side"]);
    write(&env, &[("s.txt", "s\n")]);
    commit_all(&env, T0 + DAY, "feat: side");
    git_at(
        &env,
        &at(T0 + 2 * DAY),
        &["merge", "-q", "--no-ff", "main", "-m", "side <- main"],
    );
    git_at(&env, &at(T0), &["checkout", "-q", "main"]);
    git_at(
        &env,
        &at(T0 + 2 * DAY),
        &["merge", "-q", "--no-ff", "side~1", "-m", "main <- side"],
    );
    write(&env, &[("m.txt", "m\nm2\n")]);
    commit_all(&env, T0 + 3 * DAY, "feat: main 2");
    git_at(
        &env,
        &at(T0 + 4 * DAY),
        &["merge", "-q", "--no-ff", "side", "-m", "main <- side 2"],
    );
    env.arch_stdout(&["index"]);
    truth::assert_line_counts(&env.repo, &env.births());
}

/// A second root (orphan history) merged in with unrelated histories.
#[test]
fn scenario_unrelated_root_merge_matches_git() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    write(&env, &[("main.txt", "1\n2\n")]);
    commit_all(&env, T0, "feat: main");
    git_at(&env, &at(T0), &["checkout", "-q", "--orphan", "vendor"]);
    git_at(&env, &at(T0), &["rm", "-rq", "--cached", "."]);
    std::fs::remove_file(env.repo.join("main.txt")).unwrap();
    write(&env, &[("lib/v.txt", &numbered(3))]);
    commit_all(&env, T0 + DAY, "feat: vendor root");
    git_at(&env, &at(T0), &["checkout", "-q", "main"]);
    git_at(
        &env,
        &at(T0 + 2 * DAY),
        &[
            "merge",
            "-q",
            "--allow-unrelated-histories",
            "vendor",
            "-m",
            "merge vendor",
        ],
    );
    env.arch_stdout(&["index"]);
    truth::assert_line_counts(&env.repo, &env.births());
    assert_eq!(
        env.sql_i64(
            "SELECT COUNT(*) FROM commits c WHERE NOT EXISTS \
                     (SELECT 1 FROM commit_parents p WHERE p.sha = c.sha)"
        ),
        2,
        "two roots"
    );
}

/// History rewritten under an indexed cache (reset + new commits, as after
/// a force-push): the incremental index drops the lost commits.
#[test]
fn scenario_rewritten_history_reindexes_incrementally() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    for i in 0..3 {
        write(&env, &[("f.txt", &numbered(i + 1))]);
        commit_all(&env, T0 + i as i64 * DAY, "feat: grow");
    }
    env.arch_stdout(&["index"]);
    git_at(&env, &at(T0), &["reset", "-q", "--hard", "HEAD~2"]);
    for i in 0..2 {
        write(&env, &[("g.txt", &numbered(i + 5))]);
        commit_all(&env, T0 + (5 + i as i64) * DAY, "feat: other");
    }
    env.arch_stdout(&["index"]);
    assert_eq!(env.sql_i64("SELECT COUNT(*) FROM commits"), 3);
    env.assert_matches_git();
}

// ---- repo shapes the CLI refuses or warns about ----

#[test]
fn scenario_bare_detached_and_shallow_repos() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    for i in 0..2 {
        write(&env, &[("f.txt", &numbered(i + 1))]);
        commit_all(&env, T0 + i as i64 * DAY, "feat: f");
    }
    let src = env.repo.clone();
    let run = |repo: &std::path::Path| {
        Command::new(BIN)
            .arg("--repo")
            .arg(repo)
            .arg("index")
            .env("XDG_DATA_HOME", &env.xdg_data)
            .env("XDG_CONFIG_HOME", &env.xdg_config)
            .output()
            .unwrap()
    };
    let clone = |args: &[&str], dst: &str| {
        let dst = env.root.path().join(dst);
        let ok = Command::new("git")
            .args(["clone", "-q"])
            .args(args)
            .arg(format!("file://{}", src.display()))
            .arg(&dst)
            .status()
            .unwrap();
        assert!(ok.success());
        dst
    };

    let bare = run(&clone(&["--bare"], "bare.git"));
    assert!(!bare.status.success());
    assert!(String::from_utf8_lossy(&bare.stderr).contains("bare repositories are not supported"));

    let shallow = run(&clone(&["--depth", "1"], "shallow"));
    assert!(shallow.status.success());
    assert!(String::from_utf8_lossy(&shallow.stderr).contains("shallow clone"));

    git_at(&env, &at(T0), &["checkout", "-q", "--detach", "HEAD~1"]);
    let detached = run(&env.repo);
    assert!(!detached.status.success());
    assert!(String::from_utf8_lossy(&detached.stderr).contains("detached HEAD"));
}

// ---- languages ----

/// Grammar-less languages and the Kotlin/C#/Swift grammars reach `stats`,
/// `burndown` and `hotspot`.
#[test]
fn scenario_new_languages_reach_reports() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    init(&env);
    write(
        &env,
        &[
            ("ci.yml", "# ci\non: push\n"),
            ("Cargo.toml", "[package]\nname = \"x\" # c\n"),
            ("Makefile", "all:\n\techo hi\n"),
            ("app/Main.kt", "fun main() {\n    println(1)\n}\n"),
            ("Program.cs", "class P {\n    void M() {}\n}\n"),
            ("main.swift", "// s\nfunc go() {}\n"),
        ],
    );
    commit_all(&env, T0, "feat: polyglot");
    env.arch_stdout(&["index", "--bucket", "commit"]);

    let stats: Vec<String> = env
        .rows(&["stats"])
        .iter()
        .map(|r| {
            let c: Vec<&str> = r.split('\t').collect();
            // lang files code comments funcs
            format!("{} {} {} {} {}", c[0], c[1], c[3], c[4], c[8])
        })
        .collect();
    for want in [
        "YAML 1 1 1 0",
        "TOML 1 2 0 0",
        "Makefile 1 2 0 0",
        "Kotlin 1 3 0 1",
        "C# 1 3 0 1",
        "Swift 1 1 1 1",
    ] {
        assert!(
            stats.iter().any(|s| s == want),
            "missing {want:?} in {stats:#?}"
        );
    }
    assert!(env
        .rows(&["burndown", "--by", "language"])
        .contains(&format!("{T0}\tYAML\t1")));
    let hot = env.rows(&["hotspot", "--lang", "kotlin"]);
    assert!(
        hot.iter().any(|r| r.contains("\tmain\tfn\tKotlin\t")),
        "{hot:?}"
    );
}

/// Reader closing the pipe early (`| head`) ends the run quietly: exit 0,
/// nothing on stderr. Output is far past the pipe buffer, so a write fails.
#[test]
fn scenario_closed_stdout_exits_quietly() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }
    let env = Env::new();
    dsl::build(
        &env.repo,
        &[Action::commit(
            "Alice",
            "2025-01-01T00:00:00Z",
            "feat: a",
            &[("a.rs", "fn f() {}\n")],
        )],
    );
    env.arch_stdout(&["index"]);
    let mut child = Command::new(BIN)
        .arg("--repo")
        .arg(&env.repo)
        .args(["sql", "SELECT * FROM range(1000000)"])
        .env("XDG_DATA_HOME", &env.xdg_data)
        .env("XDG_CONFIG_HOME", &env.xdg_config)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn git-archaeologist");
    drop(child.stdout.take());
    let out = child.wait_with_output().unwrap();
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(out.status.success(), "status {}: {err}", out.status);
    assert!(err.is_empty(), "stderr: {err}");
}
