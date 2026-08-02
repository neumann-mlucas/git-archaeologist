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

#[derive(Debug, Clone)]
pub struct FuncDef {
    pub name: String,
    /// 'fn' | 'method' | 'test'
    pub kind: String,
    /// 1-based line numbers, inclusive.
    pub start_line: u32,
    pub end_line: u32,
}

#[derive(Debug, Clone)]
pub struct FileSnapshot {
    pub stat: FileStat,
    pub funcs: Vec<FuncDef>,
}

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
    funcs: Vec<FuncDef>,
}

const MAX_BLOB_BYTES: usize = 2 * 1024 * 1024;

#[derive(Copy, Clone, Debug)]
enum TestDetect {
    /// Path-based only. Applies to JS/TS/C/C++.
    None,
    /// Rust — walk preceding siblings of the def for an `attribute_item`
    /// whose source contains "test", OR check ancestor `mod_item`s for a
    /// `cfg(test)` attribute.
    RustAttr,
    /// Python / Go — `name.starts_with(prefix)`. Case-sensitive.
    NamePrefix(&'static str),
}

#[derive(Clone)]
struct LangSpec {
    label: &'static str,
    language: Language,
    comment_kinds: &'static [&'static str],
    /// Node kinds that count as a top-level function definition.
    fn_kinds: &'static [&'static str],
    /// Node kinds that count as a method (function inside a type/class/impl).
    method_kinds: &'static [&'static str],
    /// Ancestor kinds under which `fn_kinds` should be reported as
    /// `method` instead of `fn` (e.g. Rust's `impl_item`, Python's
    /// `class_definition`, C++'s `class_specifier` / `struct_specifier`).
    method_ancestor_kinds: &'static [&'static str],
    test_detect: TestDetect,
}

pub struct LangRegistry {
    by_ext: HashMap<&'static str, LangSpec>,
    parser: Parser,
}

impl LangRegistry {
    pub fn new() -> Self {
        let mut m = HashMap::new();
        let mut add = |exts: &[&'static str], spec: LangSpec| {
            for e in exts {
                m.insert(*e, spec.clone());
            }
        };

        add(
            &["rs"],
            LangSpec {
                label: "Rust",
                language: tree_sitter_rust::LANGUAGE.into(),
                comment_kinds: &["line_comment", "block_comment"],
                fn_kinds: &["function_item"],
                method_kinds: &[],
                method_ancestor_kinds: &["impl_item"],
                test_detect: TestDetect::RustAttr,
            },
        );
        add(
            &["py"],
            LangSpec {
                label: "Python",
                language: tree_sitter_python::LANGUAGE.into(),
                comment_kinds: &["comment"],
                fn_kinds: &["function_definition"],
                method_kinds: &[],
                method_ancestor_kinds: &["class_definition"],
                test_detect: TestDetect::NamePrefix("test_"),
            },
        );
        add(
            &["js", "mjs", "cjs", "jsx"],
            LangSpec {
                label: "JavaScript",
                language: tree_sitter_javascript::LANGUAGE.into(),
                comment_kinds: &["comment", "html_comment"],
                fn_kinds: &["function_declaration", "generator_function_declaration"],
                method_kinds: &["method_definition"],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::None,
            },
        );
        add(
            &["ts"],
            LangSpec {
                label: "TypeScript",
                language: tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
                comment_kinds: &["comment", "html_comment"],
                fn_kinds: &[
                    "function_declaration",
                    "function_signature",
                    "generator_function_declaration",
                ],
                method_kinds: &["method_definition", "method_signature"],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::None,
            },
        );
        add(
            &["tsx"],
            LangSpec {
                label: "TSX",
                language: tree_sitter_typescript::LANGUAGE_TSX.into(),
                comment_kinds: &["comment", "html_comment"],
                fn_kinds: &[
                    "function_declaration",
                    "function_signature",
                    "generator_function_declaration",
                ],
                method_kinds: &["method_definition", "method_signature"],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::None,
            },
        );
        add(
            &["go"],
            LangSpec {
                label: "Go",
                language: tree_sitter_go::LANGUAGE.into(),
                comment_kinds: &["comment"],
                fn_kinds: &["function_declaration"],
                method_kinds: &["method_declaration"],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::NamePrefix("Test"),
            },
        );
        add(
            &["c", "h"],
            LangSpec {
                label: "C",
                language: tree_sitter_c::LANGUAGE.into(),
                comment_kinds: &["comment"],
                fn_kinds: &["function_definition"],
                method_kinds: &[],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::None,
            },
        );
        add(
            &["cpp", "cc", "cxx", "hpp", "hh", "hxx"],
            LangSpec {
                label: "C++",
                language: tree_sitter_cpp::LANGUAGE.into(),
                comment_kinds: &["comment"],
                fn_kinds: &["function_definition"],
                method_kinds: &[],
                method_ancestor_kinds: &["class_specifier", "struct_specifier"],
                test_detect: TestDetect::None,
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
}

impl Default for LangRegistry {
    fn default() -> Self {
        Self::new()
    }
}

pub fn snapshot(
    repo: &Repo,
    sha: &str,
    cache: &mut BlobCache,
    registry: &mut LangRegistry,
) -> Result<Vec<FileSnapshot>> {
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
            // Path-based test fallback: if the file lives in a test dir /
            // uses a test-name suffix, mark every func as 'test'. Overrides
            // the per-lang detector.
            let mut funcs = c.funcs;
            if is_test_path(&path_str) {
                for f in funcs.iter_mut() {
                    f.kind = "test".to_string();
                }
            }
            out.push(FileSnapshot {
                stat: FileStat {
                    path: path_str,
                    language: c.language,
                    code: c.code,
                    comments: c.comments,
                    blanks: c.blanks,
                },
                funcs,
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
    let head = &blob.data[..blob.data.len().min(8192)];
    if head.contains(&0u8) {
        return None;
    }

    let spec = registry.spec_for(path)?;
    let label = spec.label.to_string();
    let language = spec.language.clone();
    let spec_snapshot = spec.clone();

    registry.parser.set_language(&language).ok()?;
    let tree: Tree = registry.parser.parse(&*blob.data, None)?;

    let (code, comments, blanks) =
        count_lines(&blob.data, tree.root_node(), spec_snapshot.comment_kinds);
    let funcs = extract_funcs(&blob.data, tree.root_node(), &spec_snapshot);

    Some(CachedParse {
        language: label,
        code,
        comments,
        blanks,
        funcs,
    })
}

fn is_test_path(path: &str) -> bool {
    let p = path.to_ascii_lowercase();
    if p.starts_with("tests/") || p.contains("/tests/") {
        return true;
    }
    if p.starts_with("__tests__/") || p.contains("/__tests__/") {
        return true;
    }
    if p.contains(".spec.") {
        return true;
    }
    for suf in [
        ".test.js",
        ".test.ts",
        ".test.jsx",
        ".test.tsx",
        "_test.c",
        "_test.cc",
        "_test.cpp",
        "_test.cxx",
    ] {
        if p.ends_with(suf) {
            return true;
        }
    }
    // Python test convention.
    if let Some(name) = Path::new(&p).file_name().and_then(|s| s.to_str()) {
        if name.starts_with("test_") && name.ends_with(".py") {
            return true;
        }
        for prefix in ["test_"] {
            for ext in [".c", ".cc", ".cpp", ".cxx"] {
                if name.starts_with(prefix) && name.ends_with(ext) {
                    return true;
                }
            }
        }
    }
    false
}

/// Walk the AST and emit one `FuncDef` per top-level (or method-level)
/// function declaration recognized by `spec`.
fn extract_funcs(source: &[u8], root: Node, spec: &LangSpec) -> Vec<FuncDef> {
    let mut out = Vec::new();
    walk_defs(source, root, spec, &mut out);
    out
}

fn walk_defs(source: &[u8], node: Node, spec: &LangSpec, out: &mut Vec<FuncDef>) {
    let kind = node.kind();
    let is_fn = spec.fn_kinds.contains(&kind);
    let is_method = spec.method_kinds.contains(&kind);

    if is_fn || is_method {
        if let Some(def) = build_def(source, node, spec, is_method) {
            out.push(def);
        }
        // Do NOT recurse into a function body — nested closures/inner fns
        // aren't top-level defs. Same convention as SPEC's "top-level def".
        return;
    }

    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        walk_defs(source, child, spec, out);
    }
}

fn build_def(source: &[u8], node: Node, spec: &LangSpec, is_method: bool) -> Option<FuncDef> {
    let name = extract_name(source, node)?;
    let start_line = node.start_position().row as u32 + 1;
    let end_line = node.end_position().row as u32 + 1;

    let mut kind = if is_method {
        "method"
    } else if ancestor_matches(node, spec.method_ancestor_kinds) {
        "method"
    } else {
        "fn"
    }
    .to_string();

    if is_test(source, node, spec, &name) {
        kind = "test".to_string();
    }

    Some(FuncDef {
        name,
        kind,
        start_line,
        end_line,
    })
}

fn ancestor_matches(node: Node, kinds: &[&str]) -> bool {
    if kinds.is_empty() {
        return false;
    }
    let mut cur = node.parent();
    while let Some(p) = cur {
        if kinds.contains(&p.kind()) {
            return true;
        }
        cur = p.parent();
    }
    false
}

fn extract_name(source: &[u8], node: Node) -> Option<String> {
    if let Some(n) = node.child_by_field_name("name") {
        return node_text(source, n);
    }
    // C/C++ path: function_definition has a `declarator` field; recurse
    // through nested declarators to find the identifier at the leaf.
    if let Some(decl) = node.child_by_field_name("declarator") {
        return find_ident(source, decl);
    }
    None
}

fn find_ident(source: &[u8], node: Node) -> Option<String> {
    match node.kind() {
        "identifier" | "field_identifier" | "type_identifier" => {
            return node_text(source, node);
        }
        _ => {}
    }
    if let Some(d) = node.child_by_field_name("declarator") {
        if let Some(n) = find_ident(source, d) {
            return Some(n);
        }
    }
    let mut cursor = node.walk();
    for child in node.children(&mut cursor) {
        if let Some(n) = find_ident(source, child) {
            return Some(n);
        }
    }
    None
}

fn node_text(source: &[u8], node: Node) -> Option<String> {
    let slice = source.get(node.start_byte()..node.end_byte())?;
    std::str::from_utf8(slice).ok().map(|s| s.to_string())
}

fn is_test(source: &[u8], node: Node, spec: &LangSpec, name: &str) -> bool {
    match spec.test_detect {
        TestDetect::None => false,
        TestDetect::NamePrefix(prefix) => name.starts_with(prefix),
        TestDetect::RustAttr => rust_test_attr(source, node),
    }
}

/// Rust-specific: check preceding sibling `attribute_item`s of this
/// function for `#[test]` / `#[tokio::test]`-style markers, OR check any
/// ancestor `mod_item` for a `#[cfg(test)]` attribute.
fn rust_test_attr(source: &[u8], node: Node) -> bool {
    // Preceding attribute siblings.
    let mut prev = node.prev_sibling();
    while let Some(sib) = prev {
        if sib.kind() == "attribute_item" || sib.kind() == "inner_attribute_item" {
            if let Some(txt) = node_text(source, sib) {
                if txt.contains("test") {
                    return true;
                }
            }
            prev = sib.prev_sibling();
        } else {
            break;
        }
    }
    // Ancestor mod with cfg(test).
    let mut cur = node.parent();
    while let Some(p) = cur {
        if p.kind() == "mod_item" {
            // Look at siblings preceding this mod for cfg(test).
            let mut ap = p.prev_sibling();
            while let Some(sib) = ap {
                if sib.kind() == "attribute_item" || sib.kind() == "inner_attribute_item" {
                    if let Some(txt) = node_text(source, sib) {
                        if txt.contains("cfg(test)") {
                            return true;
                        }
                    }
                    ap = sib.prev_sibling();
                } else {
                    break;
                }
            }
        }
        cur = p.parent();
    }
    false
}

fn count_lines(source: &[u8], root: Node, comment_kinds: &[&str]) -> (u32, u32, u32) {
    if source.is_empty() {
        return (0, 0, 0);
    }

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

    fn count(lang: Language, comment_kinds: &[&str], src: &str) -> (u32, u32, u32) {
        let mut parser = Parser::new();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(src.as_bytes(), None).unwrap();
        count_lines(src.as_bytes(), tree.root_node(), comment_kinds)
    }

    fn parse_and_extract(src: &str, spec: &LangSpec) -> Vec<FuncDef> {
        let mut parser = Parser::new();
        parser.set_language(&spec.language).unwrap();
        let tree = parser.parse(src.as_bytes(), None).unwrap();
        extract_funcs(src.as_bytes(), tree.root_node(), spec)
    }

    fn spec_for(reg: &LangRegistry, ext: &str) -> LangSpec {
        reg.by_ext.get(ext).expect("spec").clone()
    }

    #[test]
    fn rust_mixed() {
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
        assert_eq!(code, 5);
        assert_eq!(comments, 3);
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
        assert!(r.spec_for("foo.ts").is_some());
        assert!(r.spec_for("foo.tsx").is_some());
        assert!(r.spec_for("Cargo.toml").is_none());
        assert!(r.spec_for("README.md").is_none());
        assert!(r.spec_for("foo.unknown").is_none());
        assert!(r.spec_for("no_extension").is_none());
    }

    // ---- extract_funcs tests ----

    #[test]
    fn rust_extracts_fn_method_test() {
        let src = "\
fn top() {}

#[test]
fn t1() {}

impl Foo {
    fn m(&self) {}
}

#[cfg(test)]
mod tests {
    fn inside() {}
}
";
        let reg = LangRegistry::new();
        let spec = spec_for(&reg, "rs");
        let defs = parse_and_extract(src, &spec);
        let by_name: HashMap<_, _> = defs.iter().map(|d| (d.name.clone(), d.kind.clone())).collect();
        assert_eq!(by_name.get("top").map(|s| s.as_str()), Some("fn"));
        assert_eq!(by_name.get("t1").map(|s| s.as_str()), Some("test"));
        assert_eq!(by_name.get("m").map(|s| s.as_str()), Some("method"));
        assert_eq!(by_name.get("inside").map(|s| s.as_str()), Some("test"));
    }

    #[test]
    fn python_extracts_fn_method_test() {
        let src = "\
def top():
    pass

def test_it():
    pass

class C:
    def m(self):
        pass
";
        let reg = LangRegistry::new();
        let spec = spec_for(&reg, "py");
        let defs = parse_and_extract(src, &spec);
        let by_name: HashMap<_, _> = defs.iter().map(|d| (d.name.clone(), d.kind.clone())).collect();
        assert_eq!(by_name.get("top").map(|s| s.as_str()), Some("fn"));
        assert_eq!(by_name.get("test_it").map(|s| s.as_str()), Some("test"));
        assert_eq!(by_name.get("m").map(|s| s.as_str()), Some("method"));
    }

    #[test]
    fn go_extracts_test_prefix() {
        let src = "\
package p

func Regular() {}

func TestSomething(t *testing.T) {}

func (r *Recv) Method() {}
";
        let reg = LangRegistry::new();
        let spec = spec_for(&reg, "go");
        let defs = parse_and_extract(src, &spec);
        let by_name: HashMap<_, _> = defs.iter().map(|d| (d.name.clone(), d.kind.clone())).collect();
        assert_eq!(by_name.get("Regular").map(|s| s.as_str()), Some("fn"));
        assert_eq!(by_name.get("TestSomething").map(|s| s.as_str()), Some("test"));
        assert_eq!(by_name.get("Method").map(|s| s.as_str()), Some("method"));
    }

    #[test]
    fn c_extracts_function() {
        let src = "\
int add(int a, int b) {
    return a + b;
}

static void log(const char *m) {}
";
        let reg = LangRegistry::new();
        let spec = spec_for(&reg, "c");
        let defs = parse_and_extract(src, &spec);
        let names: Vec<_> = defs.iter().map(|d| d.name.clone()).collect();
        assert!(names.contains(&"add".to_string()));
        assert!(names.contains(&"log".to_string()));
    }

    #[test]
    fn js_extracts_function_and_method() {
        let src = "\
function top() {}

class C {
    m() {}
}
";
        let reg = LangRegistry::new();
        let spec = spec_for(&reg, "js");
        let defs = parse_and_extract(src, &spec);
        let by_name: HashMap<_, _> = defs.iter().map(|d| (d.name.clone(), d.kind.clone())).collect();
        assert_eq!(by_name.get("top").map(|s| s.as_str()), Some("fn"));
        assert_eq!(by_name.get("m").map(|s| s.as_str()), Some("method"));
    }

    #[test]
    fn ts_extracts_function_and_method() {
        let src = "\
function top(): void {}

class C {
    m(): number { return 1 }
}
";
        let reg = LangRegistry::new();
        let spec = spec_for(&reg, "ts");
        let defs = parse_and_extract(src, &spec);
        let by_name: HashMap<_, _> = defs.iter().map(|d| (d.name.clone(), d.kind.clone())).collect();
        assert_eq!(by_name.get("top").map(|s| s.as_str()), Some("fn"));
        assert_eq!(by_name.get("m").map(|s| s.as_str()), Some("method"));
    }

    #[test]
    fn is_test_path_matrix() {
        assert!(is_test_path("tests/foo.rs"));
        assert!(is_test_path("src/nested/tests/x.py"));
        assert!(is_test_path("app/__tests__/foo.js"));
        assert!(is_test_path("src/foo.test.ts"));
        assert!(is_test_path("src/foo.spec.tsx"));
        assert!(is_test_path("src/test_utils.py"));
        assert!(is_test_path("src/mymodule_test.cpp"));
        assert!(!is_test_path("src/main.rs"));
        assert!(!is_test_path("lib/foo.py"));
    }

    #[test]
    fn is_test_path_overrides_kind_in_snapshot() {
        // Just check the helper; the snapshot integration is covered by
        // index::tests::smoke_index_tiny_repo via funcs table.
        assert!(is_test_path("tests/mod.rs"));
    }
}
