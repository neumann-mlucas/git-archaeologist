use std::collections::HashSet;

use anyhow::{Context, Result};
use gix::bstr::ByteSlice;
use gix::traverse::commit::simple::Sorting;
use time::OffsetDateTime;

use crate::index::parse::{parse_conv_commit, parse_trailers, ConvCommit, Trailer};
use crate::repo::Repo;

#[derive(Debug, Clone)]
pub struct CommitInfo {
    pub sha: String,
    pub parents: Vec<String>,
    pub author_name: String,
    pub author_email: String,
    pub committer_name: String,
    pub committer_email: String,
    pub committed_at: OffsetDateTime,
    pub is_merge: bool,
    pub conv: ConvCommit,
    pub trailers: Vec<Trailer>,
    pub ignored_blame: bool,
}

#[derive(Debug, Clone)]
pub struct TagInfo {
    pub name: String,
    pub sha: String,
    pub tagged_at: i64,
}

/// Walk the full DAG reachable from every local branch tip. Oldest-first.
///
/// `ignore_revs` is the parsed `.git-blame-ignore-revs` (lowercased hex);
/// commits whose sha is a member get `ignored_blame = true`.
pub fn walk(repo: &Repo, ignore_revs: &HashSet<String>) -> Result<Vec<CommitInfo>> {
    let tips = ref_tips(repo)?;
    if tips.is_empty() {
        return Ok(Vec::new());
    }

    let walk = repo
        .git
        .rev_walk(tips)
        .sorting(Sorting::ByCommitTimeNewestFirst)
        .all()
        .context("starting rev walk")?;

    let mut out: Vec<CommitInfo> = Vec::new();

    for info in walk {
        let info = info.context("walking commit")?;
        let commit = info.object().context("loading commit object")?;

        let parents: Vec<String> = commit.parent_ids().map(|p| p.to_string()).collect();
        let is_merge = parents.len() > 1;

        let author = commit.author().context("decoding commit author")?;
        let committer = commit.committer().context("decoding commit committer")?;
        let time = committer.time; // author's clock can be edited; committer's is closer to "when landed"

        let committed_at = OffsetDateTime::from_unix_timestamp(time.seconds)
            .unwrap_or(OffsetDateTime::UNIX_EPOCH);

        let msg_ref = commit.message_raw().context("decoding commit message")?;
        let msg = msg_ref.to_str_lossy();

        let sha = commit.id().to_string();
        let ignored_blame = ignore_revs.contains(&sha.to_ascii_lowercase());

        out.push(CommitInfo {
            sha,
            parents,
            author_name: author.name.to_string(),
            author_email: author.email.to_string(),
            committer_name: committer.name.to_string(),
            committer_email: committer.email.to_string(),
            committed_at,
            is_merge,
            conv: parse_conv_commit(&msg),
            trailers: parse_trailers(&msg),
            ignored_blame,
        });
    }

    out.reverse();
    Ok(out)
}

/// Collect every reachable commit-id from local refs (heads + tags). Uses
/// only local refs — a bare inspection of the current worktree, matching
/// `git rev-list --branches --tags`.
fn ref_tips(repo: &Repo) -> Result<Vec<gix::ObjectId>> {
    let mut seen: HashSet<gix::ObjectId> = HashSet::new();
    let mut out = Vec::new();

    // Always include HEAD so a detached-but-not-tagged tip still gets walked.
    if let Ok(head_id) = repo.git.head_id() {
        let id = head_id.detach();
        if seen.insert(id) {
            out.push(id);
        }
    }

    let refs = repo.git.references().context("opening ref platform")?;
    for res in refs.all().context("iterating all refs")? {
        let Ok(mut r) = res else { continue };
        let Ok(id) = r.peel_to_id_in_place() else { continue };
        let id = id.detach();
        if seen.insert(id) {
            out.push(id);
        }
    }
    Ok(out)
}

/// Enumerate tags (annotated + lightweight), keyed by short name.
pub fn tags(repo: &Repo) -> Result<Vec<TagInfo>> {
    let mut out = Vec::new();
    let refs = repo.git.references().context("opening ref platform")?;
    for res in refs.tags().context("iterating tag refs")? {
        let Ok(mut r) = res else { continue };
        let name = r.name().shorten().to_string();

        // Peel to the target commit id.
        let Ok(peeled_id) = r.peel_to_id_in_place() else { continue };
        let commit_id = peeled_id.detach();

        // Tag date: annotated tag has its own tagger time; lightweight tag
        // falls back to the pointed commit's committer time.
        let tagged_at = match r.id().object() {
            Ok(obj) => match obj.try_into_tag() {
                Ok(tag) => tag
                    .tagger()
                    .ok()
                    .flatten()
                    .map(|s| s.time.seconds)
                    .unwrap_or_else(|| commit_time(repo, commit_id).unwrap_or(0)),
                Err(_) => commit_time(repo, commit_id).unwrap_or(0),
            },
            Err(_) => commit_time(repo, commit_id).unwrap_or(0),
        };

        out.push(TagInfo {
            name,
            sha: commit_id.to_string(),
            tagged_at,
        });
    }
    Ok(out)
}

fn commit_time(repo: &Repo, id: gix::ObjectId) -> Option<i64> {
    let commit = repo.git.find_commit(id).ok()?;
    let sig = commit.committer().ok()?;
    Some(sig.time.seconds)
}
