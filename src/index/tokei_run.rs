use anyhow::Result;

#[derive(Debug, Clone)]
pub struct FileStat {
    pub path: String,
    pub language: String,
    pub code: u32,
    pub comments: u32,
    pub blanks: u32,
}

/// Run tokei against the tree at `sha`, without checking anything out.
///
/// Reads blobs from the git ODB and feeds them to tokei parsers.
pub fn snapshot(_repo: &crate::repo::Repo, _sha: &str) -> Result<Vec<FileStat>> {
    // 1. Peel commit → tree
    // 2. Walk tree entries, recurse into subtrees
    // 3. For each blob, detect language by filename via `tokei::LanguageType::from_path`
    // 4. Read blob bytes, call `language_type.parse_from_slice(...)` (or similar)
    // 5. Collect into FileStat rows
    todo!("iterate tree blobs, parse each with tokei")
}
