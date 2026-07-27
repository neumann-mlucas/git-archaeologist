pub mod bucket;
pub mod churn;
pub mod mailmap;
pub mod tokei_run;
pub mod walker;

use anyhow::Result;
use crossbeam_channel::Sender;

use crate::cache::Cache;
use crate::repo::Repo;

/// Progress event emitted during indexing.
#[derive(Debug, Clone)]
pub enum Progress {
    Started { total_commits: usize },
    Commit { done: usize, total: usize },
    Finished,
}

pub struct IndexOptions {
    pub force_full: bool,
    pub bucket_override: Option<bucket::BucketSize>,
}

/// Run the full or incremental indexer.
pub fn run(
    _repo: &Repo,
    _cache: &mut Cache,
    _opts: IndexOptions,
    _progress: Option<Sender<Progress>>,
) -> Result<()> {
    // 1. Load mailmap + user aliases → sync authors table
    // 2. Walk rev-list, decide bucketing, mark sampled commits
    // 3. Populate churn (all commits) + file_stats (sampled only)
    // 4. Update meta.indexed_head_sha
    todo!("wire full indexer — see SPEC §Indexing pipeline")
}
