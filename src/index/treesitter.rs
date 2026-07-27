use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use tree_sitter::{Language, Node, Parser, Tree};

use crate::repo::Repo;

#[derive(Debug, Clone)]
pub struct FileStat {
    pub path: String,
    pub language: String,
    pub code: u32,
    pub comments: u32,
    pub blanks: u32,
}

/// Blob-parse memoization: same OID → same result. Used across sampled
/// commits so unchanged files aren't re-parsed. Direct port of the tokei
/// BlobCache; only the CachedParse content changes.
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

const MAX_BLOB_BYTES: usize = 2 * 1024 * 1024;

/// Language registry — hooks a `tree_sitter::Language` and the node kinds
/// that count as comments, keyed by file extension. Step A ships Rust only;
/// Step C adds the long tail (python, ts, go, c, cpp, java, …).
struct LangSpec {
    /// Display label written to the DB. Matches gross tokei labels where
    /// possible so downstream palette hashing stays roughly stable.
    label: &'static str,
    language: Language,
    comment_kinds: &'static [&'static str],
}

pub struct LangRegistry {
    /// Keyed by lowercased file extension (no leading dot).
    by_ext: HashMap<&'static str, LangSpec>,
    parser: Parser,
}

impl LangRegistry {
    pub fn new() -> Self {
        let mut by_ext = HashMap::new();
        let rust_lang: Language = tree_sitter_rust::LANGUAGE.into();
        by_ext.insert(
            "rs",
            LangSpec {
                label: "Rust",
                language: rust_lang.clone(),
                comment_kinds: &["line_comment", "block_comment"],
            },
        );

        Self {
            by_ext,
            parser: Parser::new(),
        }
    }

    fn spec_for(&self, path: &str) -> Option<&LangSpec> {
        let ext = Path::new(path).extension().and_then(|s| s.to_str())?;
        self.by_ext.get(ext.to_ascii_lowercase().as_str())
    }
}

impl Default for LangRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Snapshot LOC per file at `sha`. Mirrors the old `tokei_run::snapshot`
/// signature so `index::mod` can swap over cleanly.
pub fn snapshot(
    repo: &Repo,
    sha: &str,
    cache: &mut BlobCache,
    registry: &mut LangRegistry,
) -> Result<Vec<FileStat>> {
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

    let mut out = Vec::with_capacity(entries.len());

    for entry in entries {
        if !entry.mode.is_blob() {
            continue;
        }
        let path_str = entry.filepath.to_string();

        // Language detection is extension-based (cheap). Unknown extensions
        // are skipped entirely — tree-sitter can't parse without a grammar
        // and there's no reasonable fallback line count without one.
        let has_spec = registry.spec_for(&path_str).is_some();
        if !has_spec {
            continue;
        }

        let cached = match cache.entries.get(&entry.oid) {
            Some(v) => v.clone(),
            None => {
                let parsed = parse_blob(repo, entry.oid, &path_str, registry);
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
    path: &str,
    registry: &mut LangRegistry,
) -> Option<CachedParse> {
    let blob = repo.git.find_blob(oid).ok()?;
    if blob.data.len() > MAX_BLOB_BYTES {
        return None;
    }
    // Fast binary reject: first 8KiB contains NUL → skip.
    let head = &blob.data[..blob.data.len().min(8192)];
    if head.contains(&0u8) {
        return None;
    }

    let spec = registry.spec_for(path)?;
    let label = spec.label.to_string();
    let language = spec.language.clone();
    let comment_kinds: Vec<&'static str> = spec.comment_kinds.to_vec();

    registry.parser.set_language(&language).ok()?;
    let tree: Tree = registry.parser.parse(&*blob.data, None)?;

    let (code, comments, blanks) = count_lines(&blob.data, tree.root_node(), &comment_kinds);
    Some(CachedParse {
        language: label,
        code,
        comments,
        blanks,
    })
}

/// Classify every line of source as blank / comment / code.
/// Rules:
///   - blank:   line has no non-whitespace bytes at all.
///   - code:    line has non-whitespace bytes OUTSIDE any comment-node byte
///              range. (Matches tokei: `let x = 1; // trailing` is code.)
///   - comment: line has non-whitespace bytes AND every one falls inside
///              a comment-node byte range.
fn count_lines(source: &[u8], root: Node, comment_kinds: &[&str]) -> (u32, u32, u32) {
    if source.is_empty() {
        return (0, 0, 0);
    }

    // Collect comment byte ranges.
    let mut ranges: Vec<(usize, usize)> = Vec::new();
    collect_comment_ranges(root, comment_kinds, &mut ranges);
    ranges.sort_unstable_by_key(|r| r.0);

    let mut blanks = 0u32;
    let mut comments = 0u32;
    let mut code = 0u32;

    let mut classify = |start: usize, end: usize| {
        let line = &source[start..end];
        match classify_line(line, start, &ranges) {
            LineKind::Blank => blanks += 1,
            LineKind::Comment => comments += 1,
            LineKind::Code => code += 1,
        }
    };

    let mut line_start = 0usize;
    for (i, &b) in source.iter().enumerate() {
        if b == b'\n' {
            classify(line_start, i);
            line_start = i + 1;
        }
    }
    // Trailing partial line without a final newline.
    if line_start < source.len() {
        classify(line_start, source.len());
    }
    (code, comments, blanks)
}

#[derive(Copy, Clone)]
enum LineKind {
    Blank,
    Comment,
    Code,
}

fn classify_line(line: &[u8], line_start_byte: usize, ranges: &[(usize, usize)]) -> LineKind {
    let mut any_nonws = false;
    let mut any_nonws_outside_comment = false;
    for (idx, b) in line.iter().enumerate() {
        if !b.is_ascii_whitespace() {
            any_nonws = true;
            let abs = line_start_byte + idx;
            if !byte_inside_any(abs, ranges) {
                any_nonws_outside_comment = true;
                break;
            }
        }
    }
    if !any_nonws {
        LineKind::Blank
    } else if any_nonws_outside_comment {
        LineKind::Code
    } else {
        LineKind::Comment
    }
}

fn byte_inside_any(abs: usize, ranges: &[(usize, usize)]) -> bool {
    // Small counts of comment ranges — linear scan is fine. For very large
    // files this could switch to a binary search on start byte.
    ranges.iter().any(|(s, e)| abs >= *s && abs < *e)
}

fn collect_comment_ranges(node: Node, comment_kinds: &[&str], out: &mut Vec<(usize, usize)>) {
    if comment_kinds.contains(&node.kind()) {
        out.push((node.start_byte(), node.end_byte()));
        return;
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        collect_comment_ranges(child, comment_kinds, out);
    }
}
