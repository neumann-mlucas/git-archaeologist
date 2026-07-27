use std::hash::Hasher;
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use directories::ProjectDirs;

/// Opened, validated repository handle.
pub struct Repo {
    pub git: gix::Repository,
    pub root: PathBuf,
}

impl Repo {
    /// Path to the cache DuckDB file under the user's XDG data dir.
    /// Keyed by `<worktree basename>-<hash of canonical worktree path>` so
    /// multiple clones of the same project at different paths get distinct
    /// caches, but the same path always resolves back to the same file.
    ///
    /// Linux: ~/.local/share/git-archaeologist/caches/<key>/cache.duckdb
    /// macOS: ~/Library/Application Support/git-archaeologist/caches/…
    pub fn cache_path(&self) -> PathBuf {
        let dirs = ProjectDirs::from("", "", "git-archaeologist")
            .expect("resolving XDG data dir");
        dirs.data_dir()
            .join("caches")
            .join(cache_key(&self.root))
            .join("cache.duckdb")
    }

    pub fn branch_name(&self) -> Result<String> {
        let head = self.git.head().context("reading HEAD ref")?;
        match head.referent_name() {
            Some(name) => Ok(name.shorten().to_string()),
            None => bail!("detached HEAD"),
        }
    }

    /// True if `.git/shallow` exists — history is truncated.
    pub fn is_shallow(&self) -> bool {
        self.git.path().join("shallow").exists()
    }
}

fn cache_key(root: &Path) -> String {
    let canonical = root.to_string_lossy();
    let name = root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("repo");
    let mut h = std::collections::hash_map::DefaultHasher::new();
    h.write(canonical.as_bytes());
    let hash = h.finish();
    format!("{name}-{hash:016x}")
}

/// Discover the repo at `path`, validate: not bare, not detached HEAD.
pub fn open(path: &Path) -> Result<Repo> {
    let git = gix::discover(path).with_context(|| format!("no git repo at {}", path.display()))?;

    if git.is_bare() {
        bail!("bare repositories are not supported");
    }

    let root = git
        .work_dir()
        .context("repo has no working directory")?
        .canonicalize()
        .context("canonicalizing worktree root")?;

    let repo = Repo { git, root };

    // Reject detached HEAD.
    let _ = repo.branch_name()?;

    // Ensure cache dir exists.
    if let Some(parent) = repo.cache_path().parent() {
        std::fs::create_dir_all(parent).context("creating cache dir")?;
    }

    Ok(repo)
}
