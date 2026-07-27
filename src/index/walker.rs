use anyhow::{Context, Result};
use gix::traverse::commit::simple::Sorting;
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

/// Walk HEAD, return oldest-first, optionally skipping merges.
pub fn walk(repo: &Repo, skip_merges: bool) -> Result<Vec<CommitInfo>> {
    let head_id = repo
        .git
        .head_id()
        .context("resolving HEAD id")?;

    let walk = repo
        .git
        .rev_walk([head_id])
        .sorting(Sorting::ByCommitTimeNewestFirst)
        .all()
        .context("starting rev walk")?;

    let mut out: Vec<CommitInfo> = Vec::new();

    for info in walk {
        let info = info.context("walking commit")?;
        let commit = info.object().context("loading commit object")?;

        let parents: Vec<_> = commit.parent_ids().collect();
        let is_merge = parents.len() > 1;
        if skip_merges && is_merge {
            continue;
        }

        let author = commit.author().context("decoding commit author")?;
        let time = author.time;

        let committed_at = OffsetDateTime::from_unix_timestamp(time.seconds)
            .unwrap_or(OffsetDateTime::UNIX_EPOCH);

        out.push(CommitInfo {
            sha: commit.id().to_string(),
            parent_sha: parents.first().map(|p| p.to_string()),
            author_name: author.name.to_string(),
            author_email: author.email.to_string(),
            committed_at,
            is_merge,
        });
    }

    // Convert to oldest-first for stable bucketing.
    out.reverse();
    Ok(out)
}
