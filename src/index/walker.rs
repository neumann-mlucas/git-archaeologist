use anyhow::Result;
use time::OffsetDateTime;

use crate::repo::Repo;

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub sha: String,
    pub parent_sha: Option<String>,
    pub author_name: String,
    pub author_email: String,
    pub committed_at: OffsetDateTime,
    pub is_merge: bool,
}

/// Walk HEAD in reverse chronological order (oldest first).
///
/// Skips merges when `skip_merges` is true.
pub fn walk(_repo: &Repo, _skip_merges: bool) -> Result<Vec<CommitInfo>> {
    // Use gix rev-walk from HEAD, collect CommitInfo.
    // Return oldest-first for stable bucketing.
    todo!("implement rev walk via gix::Repository::rev_walk")
}
