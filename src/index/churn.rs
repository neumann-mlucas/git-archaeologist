use anyhow::Result;

#[derive(Debug, Clone)]
pub struct FileChurn {
    pub path: String,
    pub added: u32,
    pub deleted: u32,
}

/// Compute churn (adds/dels per path) for a single commit vs its first parent.
///
/// Uses gix blob-diff (or numstat via git binary as fallback).
pub fn for_commit(_repo: &crate::repo::Repo, _sha: &str) -> Result<Vec<FileChurn>> {
    todo!("diff commit vs parent, collect per-path add/del counts")
}
