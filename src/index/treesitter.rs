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
/// that count as comments, keyed by file extension.
#[derive(Clone)]
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
        let mut m = HashMap::new();

        // Helper: attach `spec` to every extension in `exts`.
        let mut add = |exts: &[&'static str], spec: LangSpec| {
            for e in exts {
                m.insert(*e, spec.clone());
            }
        };

        // C-family and friends unify comments under a single `comment` kind.
        add(
            &["rs"],
            LangSpec {
                label: "Rust",
                language: tree_sitter_rust::LANGUAGE.into(),
                comment_kinds: &["line_comment", "block_comment"],
            },
        );
        add(
            &["py"],
            LangSpec {
                label: "Python",
                language: tree_sitter_python::LANGUAGE.into(),
                comment_kinds: &["comment"],
            },
        );
        add(
            &["js", "mjs", "cjs", "jsx"],
            LangSpec {
                label: "JavaScript",
                language: tree_sitter_javascript::LANGUAGE.into(),
                comment_kinds: &["comment", "html_comment"],
            },
        );
        add(
            &["ts"],
            LangSpec {
                label: "TypeScript",
                language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                comment_kinds: &["comment", "html_comment"],
            },
        );
        add(
            &["tsx"],
            LangSpec {
                label: "TSX",
                language: tree_sitter_typescript::LANGUAGE_TSX.into(),
                comment_kinds: &["comment", "html_comment"],
            },
        );
        add(
            &["go"],
            LangSpec {
                label: "Go",
                language: tree_sitter_go::LANGUAGE.into(),
                comment_kinds: &["comment"],
            },
        );
        add(
            &["c", "h"],
            LangSpec {
                label: "C",
                language: tree_sitter_c::LANGUAGE.into(),
                comment_kinds: &["comment"],
            },
        );
        add(
            &["cpp", "cc", "cxx", "hpp", "hh", "hxx"],
            LangSpec {
                label: "C++",
                language: tree_sitter_cpp::LANGUAGE.into(),
                comment_kinds: &["comment"],
            },
        );
        add(
            &["java"],
            LangSpec {
                label: "Java",
                language: tree_sitter_java::LANGUAGE.into(),
                comment_kinds: &["line_comment", "block_comment"],
            },
        );
        add(
            &["rb"],
            LangSpec {
                label: "Ruby",
                language: tree_sitter_ruby::LANGUAGE.into(),
                comment_kinds: &["comment"],
            },
        );
        add(
            &["sh", "bash"],
            LangSpec {
                label: "Bash",
                language: tree_sitter_bash::LANGUAGE.into(),
                comment_kinds: &["comment"],
            },
        );
        add(
            &["html", "htm"],
            LangSpec {
                label: "HTML",
                language: tree_sitter_html::LANGUAGE.into(),
                comment_kinds: &["comment"],
            },
        );
        add(
            &["css"],
            LangSpec {
                label: "CSS",
                language: tree_sitter_css::LANGUAGE.into(),
                comment_kinds: &["comment", "js_comment"],
            },
        );
        add(
            &["json"],
            LangSpec {
                label: "JSON",
                language: tree_sitter_json::LANGUAGE.into(),
                comment_kinds: &["comment"],
            },
        );
        add(
            &["yaml", "yml"],
            LangSpec {
                label: "YAML",
                language: tree_sitter_yaml::LANGUAGE.into(),
                comment_kinds: &["comment"],
            },
        );
        add(
            &["toml"],
            LangSpec {
                label: "TOML",
                language: tree_sitter_toml_ng::LANGUAGE.into(),
                comment_kinds: &["comment"],
            },
        );
        add(
            &["md", "markdown"],
            LangSpec {
                label: "Markdown",
                language: tree_sitter_md::LANGUAGE.into(),
                comment_kinds: &[],
            },
        );
        add(
            &["scala", "sc"],
            LangSpec {
                label: "Scala",
                language: tree_sitter_scala::LANGUAGE.into(),
                comment_kinds: &["comment", "block_comment"],
            },
        );
        add(
            &["hs"],
            LangSpec {
                label: "Haskell",
                language: tree_sitter_haskell::LANGUAGE.into(),
                comment_kinds: &["comment", "haddock"],
            },
        );
        add(
            &["zig"],
            LangSpec {
                label: "Zig",
                language: tree_sitter_zig::LANGUAGE.into(),
                comment_kinds: &["comment"],
            },
        );

        Self {
            by_ext: m,
            parser: Parser::new(),
        }
    }

    fn spec_for(&self, path: &str) -> Option<&LangSpec> {
        let ext = Path::new(path).extension().and_then(|s| s.to_str())?;
        self.by_ext.get(ext.to_ascii_lowercase().as_str())
    }

    /// Public accessor — is this path recognized by any grammar?
    pub fn is_known(&self, path: &str) -> bool {
        self.spec_for(path).is_some()
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
///   - blank: line has no non-whitespace bytes at all.
///   - code: line has non-whitespace bytes OUTSIDE any comment-node byte
///     range. (Matches tokei: `let x = 1; // trailing` is code.)
///   - comment: line has non-whitespace bytes AND every one falls inside
///     a comment-node byte range.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Parse `src` under `lang`, count lines against `comment_kinds`.
    fn count(lang: Language, comment_kinds: &[&str], src: &str) -> (u32, u32, u32) {
        let mut parser = Parser::new();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(src.as_bytes(), None).unwrap();
        count_lines(src.as_bytes(), tree.root_node(), comment_kinds)
    }

    #[test]
    fn rust_mixed() {
        // 5 code, 3 comments, 2 blanks (10 lines total).
        let src = "\
// header comment
fn main() {
    let x = 1; // trailing comment counts as code

    // full-line comment
    let y = x + 1;
    println!(\"{y}\");
}
";
        let (code, comments, blanks) = count(
            tree_sitter_rust::LANGUAGE.into(),
            &["line_comment", "block_comment"],
            src,
        );
        assert_eq!(code, 5, "code count");
        assert_eq!(comments, 2, "comment count");
        assert_eq!(blanks, 1, "blank count");
    }

    #[test]
    fn python_docstring_is_not_a_comment() {
        // Python `#` comments only. Triple-quoted docstrings are string
        // expressions, not comments — should count as code.
        let src = "\
# leading comment
def add(a, b):
    \"\"\"docstring line 1
    docstring line 2\"\"\"
    return a + b

# trailing
";
        let (code, comments, blanks) = count(
            tree_sitter_python::LANGUAGE.into(),
            &["comment"],
            src,
        );
        assert_eq!(comments, 2);
        assert_eq!(blanks, 1);
        // 4 code lines: `def`, docstring line 1, docstring line 2, `return`.
        assert_eq!(code, 4);
    }

    #[test]
    fn go_block_comment() {
        let src = "\
package main

/* block comment
   spans lines */
func main() {
    println(\"hi\")
}
";
        let (code, comments, blanks) = count(
            tree_sitter_go::LANGUAGE.into(),
            &["comment"],
            src,
        );
        assert_eq!(comments, 2);
        assert_eq!(blanks, 1);
        assert_eq!(code, 4);
    }

    #[test]
    fn cpp_mixed_styles() {
        let src = "\
// line comment
#include <cstdio>

/* block
   comment */
int main() {
    printf(\"hi\"); // trailing
    return 0;
}
";
        let (code, comments, blanks) = count(
            tree_sitter_cpp::LANGUAGE.into(),
            &["comment"],
            src,
        );
        // 5 code lines: #include, int main() {, printf trailing, return, }.
        assert_eq!(code, 5);
        assert_eq!(comments, 3);
        assert_eq!(blanks, 1);
    }

    #[test]
    fn toml_comment() {
        let src = "\
# top-level comment
[package]
name = \"foo\"

# section comment
version = \"1.0\"
";
        let (code, comments, blanks) = count(
            tree_sitter_toml_ng::LANGUAGE.into(),
            &["comment"],
            src,
        );
        assert_eq!(code, 3);
        assert_eq!(comments, 2);
        assert_eq!(blanks, 1);
    }

    #[test]
    fn empty_source() {
        assert_eq!(
            count(tree_sitter_rust::LANGUAGE.into(), &["line_comment"], ""),
            (0, 0, 0)
        );
    }

    #[test]
    fn registry_extension_lookup() {
        let r = LangRegistry::new();
        assert!(r.spec_for("foo.rs").is_some());
        assert!(r.spec_for("foo.PY").is_some());
        assert!(r.spec_for("foo.cxx").is_some());
        assert!(r.spec_for("Cargo.toml").is_some());
        assert!(r.spec_for("README.md").is_some());
        assert!(r.spec_for("foo.unknown").is_none());
        assert!(r.spec_for("no_extension").is_none());
    }
}
