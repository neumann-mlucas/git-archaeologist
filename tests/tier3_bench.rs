//! Tier 3 bench harness (SPEC §Test strategy, TASKS Phase 5 Tier 3).
//!
//! Gated behind `--features bench-large`. Clones each fixture in
//! `benches/fixtures.toml` once (cached), runs `index` + every query
//! subcommand, asserts per-class perf ceilings, and reports timings +
//! peak RSS.
//!
//! Skipped cleanly per-fixture when no network and no cached clone.
//! Set `TIER3_SKIP_LARGE=1` to skip mid-large fixtures (CI cost lever).
//!
//! ponytail: hercules / git-of-theseus side-by-side rows deferred —
//! their CLIs shift across versions and we have no reliable install to
//! test against. Upgrade path: when either tool is on PATH in CI,
//! shell out for an equivalent burndown/loc pass on the same fixture
//! and log a side-by-side row (non-gating per TASKS Phase 5 Tier 3).

#![cfg(feature = "bench-large")]

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

const BIN: &str = env!("CARGO_BIN_EXE_git-archaeologist");
const CACHE_DIR_NAME: &str = "git-archaeologist-tests";

#[derive(Deserialize)]
struct Fixtures {
    fixture: Vec<Fixture>,
}

#[derive(Deserialize, Clone)]
struct Fixture {
    name: String,
    url: String,
    #[allow(dead_code)]
    tag: String,
    sha: String,
    class: String, // "small" | "mid" | "mid-large"
}

fn load_fixtures() -> Fixtures {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/fixtures.toml");
    let text = std::fs::read_to_string(&path).expect("read benches/fixtures.toml");
    toml::from_str(&text).expect("parse benches/fixtures.toml")
}

fn cache_root() -> PathBuf {
    if let Ok(p) = std::env::var("XDG_CACHE_HOME") {
        return PathBuf::from(p).join(CACHE_DIR_NAME);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache").join(CACHE_DIR_NAME);
    }
    std::env::temp_dir().join(CACHE_DIR_NAME)
}

fn have_git() -> bool {
    Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn have_network(url: &str) -> bool {
    Command::new("git")
        .args(["ls-remote", "--heads", url, "HEAD"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Clone fixture if missing, then check out the pinned SHA on a real branch
/// (git-archaeologist rejects detached HEAD per SPEC §Scope).
fn ensure_fixture(fx: &Fixture) -> Option<PathBuf> {
    let root = cache_root();
    std::fs::create_dir_all(&root).ok()?;
    let repo = root.join(&fx.name);

    if !repo.join(".git").exists() {
        eprintln!("cloning {} into {} …", fx.url, repo.display());
        let status = Command::new("git")
            .args(["clone", &fx.url, repo.to_str()?])
            .status()
            .ok()?;
        if !status.success() {
            return None;
        }
    }

    // Ensure the SHA is fetched — cached clones may pre-date the pin.
    let _ = Command::new("git")
        .args(["fetch", "--quiet", "origin", &fx.sha])
        .current_dir(&repo)
        .status();

    let branch = format!("pin-{}", &fx.sha[..12]);
    let out = Command::new("git")
        .args(["checkout", "--quiet", "-B", &branch, &fx.sha])
        .current_dir(&repo)
        .output()
        .ok()?;
    if !out.status.success() {
        eprintln!(
            "checkout {}: {}",
            fx.sha,
            String::from_utf8_lossy(&out.stderr)
        );
        return None;
    }
    Some(repo)
}

/// Peak RSS of all exited children so far, in KB.
/// Linux: `ru_maxrss` is KB already. macOS: bytes → divide by 1024.
///
/// ponytail: RUSAGE_CHILDREN is monotonic across the whole test run, so
/// the value reported for fixture N is max(RSS across fixtures 1..=N),
/// not fixture-N peak in isolation. Fine as long as fixtures run in
/// increasing size (mid-large last, whose peak dominates); upgrade to
/// per-fixture isolation via `Command::spawn` + subprocess getrusage
/// if a smaller fixture ever spikes above a larger one.
fn peak_rss_kb() -> u64 {
    unsafe {
        let mut u: libc::rusage = std::mem::zeroed();
        libc::getrusage(libc::RUSAGE_CHILDREN, &mut u);
        #[cfg(target_os = "linux")]
        {
            u.ru_maxrss as u64
        }
        #[cfg(target_os = "macos")]
        {
            (u.ru_maxrss as u64) / 1024
        }
        #[cfg(not(any(target_os = "linux", target_os = "macos")))]
        {
            let _ = u;
            0
        }
    }
}

fn arch(repo: &Path, xdg_data: &Path, xdg_config: &Path, args: &[&str]) -> Output {
    Command::new(BIN)
        .arg("--repo")
        .arg(repo)
        .args(args)
        .env("XDG_DATA_HOME", xdg_data)
        .env("XDG_CONFIG_HOME", xdg_config)
        .env("TERM", "dumb")
        .output()
        .expect("run git-archaeologist")
}

struct Report {
    name: String,
    class: String,
    index: Duration,
    queries: Vec<(String, Duration)>,
    peak_rss_kb: u64,
}

impl Report {
    fn print(&self) {
        eprintln!("=== tier3: {} ({}) ===", self.name, self.class);
        eprintln!("  index         {:>8.2}s", self.index.as_secs_f64());
        for (label, e) in &self.queries {
            eprintln!("  {:<12}  {:>7.0}ms", label, e.as_secs_f64() * 1000.0);
        }
        eprintln!("  peak RSS      {:>8.0} MB", self.peak_rss_kb as f64 / 1024.0);
    }
}

// Ceilings are sized for `cargo test --features bench-large` in debug
// (3-4x slower than release). SPEC targets (30s / 500ms small) apply
// to release builds; for meaningful numbers, run:
//   cargo test --release --features bench-large --test tier3_bench
fn perf_ceiling_index(class: &str) -> Duration {
    match class {
        "small" => Duration::from_secs(180),
        "mid" => Duration::from_secs(600),
        "mid-large" => Duration::from_secs(1800),
        _ => panic!("unknown class {class}"),
    }
}

fn perf_ceiling_query(class: &str) -> Duration {
    match class {
        "small" => Duration::from_secs(5),
        "mid" => Duration::from_secs(10),
        "mid-large" => Duration::from_secs(30),
        _ => panic!("unknown class {class}"),
    }
}

fn run_one(fx: &Fixture) -> Option<Report> {
    let repo = ensure_fixture(fx)?;
    let xdg_data = tempfile::TempDir::new().unwrap();
    let xdg_config = tempfile::TempDir::new().unwrap();

    let t0 = Instant::now();
    let out = arch(&repo, xdg_data.path(), xdg_config.path(), &["index"]);
    let index = t0.elapsed();
    assert!(
        out.status.success(),
        "{}: index failed:\n{}",
        fx.name,
        String::from_utf8_lossy(&out.stderr)
    );

    let queries: Vec<(&str, Vec<&str>)> = vec![
        ("burndown-lang", vec!["burndown", "--by", "language", "--format", "tsv"]),
        ("burndown-auth", vec!["burndown", "--by", "author", "--format", "tsv"]),
        ("classify", vec!["classify", "--format", "tsv"]),
        ("churn-module", vec!["churn", "--by", "module", "--format", "tsv"]),
        ("age", vec!["age", "--format", "tsv"]),
        ("coupling", vec!["coupling", "--top", "10", "--format", "tsv"]),
        ("hotspot", vec!["hotspot", "--lang", "rust", "--top", "10", "--format", "tsv"]),
        ("cohort", vec!["cohort", "--format", "tsv"]),
        ("survival", vec!["survival", "--format", "tsv"]),
    ];

    let mut query_times = Vec::with_capacity(queries.len());
    for (label, args) in &queries {
        let t = Instant::now();
        let out = arch(&repo, xdg_data.path(), xdg_config.path(), args);
        let e = t.elapsed();
        assert!(
            out.status.success(),
            "{}: {} failed:\n{}",
            fx.name,
            label,
            String::from_utf8_lossy(&out.stderr)
        );
        assert!(
            !out.stdout.is_empty(),
            "{}: {} empty stdout",
            fx.name,
            label
        );
        query_times.push((label.to_string(), e));
    }

    Some(Report {
        name: fx.name.clone(),
        class: fx.class.clone(),
        index,
        queries: query_times,
        peak_rss_kb: peak_rss_kb(),
    })
}

#[test]
fn tier3_bench_all_fixtures() {
    if !have_git() {
        eprintln!("skip: git not on PATH");
        return;
    }

    let fixtures = load_fixtures();
    let skip_large = std::env::var("TIER3_SKIP_LARGE").is_ok();
    let mut reports = Vec::new();

    for fx in &fixtures.fixture {
        if fx.class == "mid-large" && skip_large {
            eprintln!("skip: {} (TIER3_SKIP_LARGE set)", fx.name);
            continue;
        }
        let cached = cache_root().join(&fx.name).join(".git").exists();
        if !cached && !have_network(&fx.url) {
            eprintln!("skip: {} (no network, no cached clone)", fx.name);
            continue;
        }

        let r = match run_one(fx) {
            Some(r) => r,
            None => {
                eprintln!("skip: {} (fixture setup failed)", fx.name);
                continue;
            }
        };

        let idx_ceiling = perf_ceiling_index(&r.class);
        assert!(
            r.index <= idx_ceiling,
            "{}: index {:?} > ceiling {:?}",
            r.name,
            r.index,
            idx_ceiling
        );
        let q_ceiling = perf_ceiling_query(&r.class);
        for (label, e) in &r.queries {
            assert!(
                *e <= q_ceiling,
                "{}: {} {:?} > ceiling {:?}",
                r.name,
                label,
                e,
                q_ceiling
            );
        }
        if r.class == "mid-large" {
            let mb = r.peak_rss_kb as f64 / 1024.0;
            assert!(mb < 2048.0, "{}: peak RSS {:.0} MB > 2 GB", r.name, mb);
        }

        r.print();
        reports.push(r);
    }

    assert!(
        !reports.is_empty(),
        "no fixtures ran — check network + cache under {}",
        cache_root().display()
    );
}
