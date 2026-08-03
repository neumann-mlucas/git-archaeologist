//! Shared fixture loader for Tier 2 (`e2e`) and Tier 3 (`bench-large`).
//! `benches/fixtures.toml` is the single source of truth for pinned repos.

#![allow(dead_code)] // Each test binary uses a subset of these helpers.

use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const CACHE_DIR_NAME: &str = "git-archaeologist-tests";

#[derive(Deserialize)]
struct FixturesFile {
    fixture: Vec<Fixture>,
}

#[derive(Deserialize, Clone)]
pub struct Fixture {
    pub name: String,
    pub url: String,
    pub tag: String,
    pub sha: String,
    pub class: String, // "small" | "mid" | "mid-large"
}

pub fn load_fixtures() -> Vec<Fixture> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("benches/fixtures.toml");
    let text = std::fs::read_to_string(&path).expect("read benches/fixtures.toml");
    let f: FixturesFile = toml::from_str(&text).expect("parse benches/fixtures.toml");
    f.fixture
}

pub fn load_fixture(name: &str) -> Fixture {
    load_fixtures()
        .into_iter()
        .find(|f| f.name == name)
        .unwrap_or_else(|| panic!("no fixture named {name:?} in benches/fixtures.toml"))
}

pub fn cache_root() -> PathBuf {
    if let Ok(p) = std::env::var("XDG_CACHE_HOME") {
        return PathBuf::from(p).join(CACHE_DIR_NAME);
    }
    if let Some(home) = std::env::var_os("HOME") {
        return PathBuf::from(home).join(".cache").join(CACHE_DIR_NAME);
    }
    std::env::temp_dir().join(CACHE_DIR_NAME)
}

pub fn have_git() -> bool {
    Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

pub fn have_network(url: &str) -> bool {
    Command::new("git")
        .args(["ls-remote", "--heads", url, "HEAD"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Clone fixture if missing, then check out the pinned SHA on a real
/// branch (git-archaeologist rejects detached HEAD per SPEC §Scope).
pub fn ensure_fixture(fx: &Fixture) -> Option<PathBuf> {
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
