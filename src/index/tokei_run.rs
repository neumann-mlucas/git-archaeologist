use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use tokei::{Config, LanguageType};

use crate::repo::Repo;

#[derive(Debug, Clone)]
pub struct FileStat {
    pub path: String,
    pub language: String,
    pub code: u32,
    pub comments: u32,
    pub blanks: u32,
}

/// Cross-commit memoization: same blob OID → same parse result, so we skip
/// re-parsing shared blobs. Huge on repos like nvim where most files stay
/// identical between sampled commits.
#[derive(Default)]
pub struct BlobCache {
    entries: HashMap<gix::ObjectId, Option<CachedParse>>,
}

#[derive(Clone)]
struct CachedParse {
    language: String,
    code: u32,
    comments: u32,
    blanks: u32,
}

/// Blobs above this size are almost always binaries or generated content
/// that isn't worth counting. Tokei on a 50MB blob is a hot loop.
const MAX_BLOB_BYTES: usize = 2 * 1024 * 1024;

/// Snapshot LOC per file at `sha` — walks the tree in git object DB, feeds
/// blobs to tokei parsers. `cache` dedupes work across calls.
pub fn snapshot(repo: &Repo, sha: &str, cache: &mut BlobCache) -> Result<Vec<FileStat>> {
    let oid: gix::ObjectId = sha.parse().context("parsing commit sha")?;
    let commit = repo
        .git
        .find_object(oid)
        .context("finding commit object")?
        .try_into_commit()
        .context("expected commit object")?;

    let tree = commit.tree().context("resolving commit tree")?;
    let entries = tree
        .traverse()
        .breadthfirst
        .files()
        .context("walking tree entries")?;

    let cfg = Config::default();
    let mut out = Vec::with_capacity(entries.len());

    for entry in entries {
        if !entry.mode.is_blob() {
            continue;
        }
        let path_str = entry.filepath.to_string();

        // Extension-based language detection is cheap; skip tokei-unknown paths
        // before ever touching the object DB.
        let lang = match LanguageType::from_path(Path::new(&path_str), &cfg) {
            Some(l) => l,
            None => continue,
        };

        let cached = match cache.entries.get(&entry.oid) {
            Some(v) => v.clone(),
            None => {
                let parsed = parse_blob(repo, entry.oid, lang, &cfg);
                cache.entries.insert(entry.oid, parsed.clone());
                parsed
            }
        };

        if let Some(c) = cached {
            out.push(FileStat {
                path: path_str,
                language: c.language,
                code: c.code,
                comments: c.comments,
                blanks: c.blanks,
            });
        }
    }

    Ok(out)
}

fn parse_blob(
    repo: &Repo,
    oid: gix::ObjectId,
    lang: LanguageType,
    cfg: &Config,
) -> Option<CachedParse> {
    let blob = repo.git.find_blob(oid).ok()?;
    if blob.data.len() > MAX_BLOB_BYTES {
        return None;
    }
    // Fast binary reject: if the first 8KiB contains a NUL byte, treat as
    // binary. Tokei tolerates arbitrary bytes but wastes time on them.
    let head = &blob.data[..blob.data.len().min(8192)];
    if head.contains(&0u8) {
        return None;
    }
    let stats = lang.parse_from_slice(&blob.data, cfg);
    Some(CachedParse {
        // .name() is a stable human-readable label ("Rust", "C++"); Debug
        // format leaks internal variant names (CPlusPlus) that can shift
        // across tokei versions.
        language: lang.name().to_string(),
        code: stats.code as u32,
        comments: stats.comments as u32,
        blanks: stats.blanks as u32,
    })
}
