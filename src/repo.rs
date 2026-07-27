use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};

/// Opened, validated repository handle.
pub struct Repo {
    pub git: gix::Repository,
    pub root: PathBuf,
}

impl Repo {
    /// Path to the cache SQLite file inside `.git/`.
    pub fn cache_path(&self) -> PathBuf {
        self.git
            .path()
            .join("git-archaeologist")
            .join("cache.sqlite")
    }

    pub fn head_sha(&self) -> Result<String> {
        let head = self.git.head_commit().context("resolving HEAD")?;
        Ok(head.id().to_string())
    }

    pub fn branch_name(&self) -> Result<String> {
        let head = self.git.head().context("reading HEAD ref")?;
        match head.referent_name() {
            Some(name) => Ok(name.shorten().to_string()),
            None => bail!("detached HEAD"),
        }
    }
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
