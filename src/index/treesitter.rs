use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use rayon::prelude::*;
use tree_sitter::{Language, Node, Parser, Tree};

use crate::repo::Repo;

#[derive(Debug, Clone)]
pub struct FileStat {
    pub path: String,
    pub language: String,
    pub code: u32,
    pub comments: u32,
    pub blanks: u32,
    /// Code lines (not comments/blanks) that are test code: whole file for
    /// a test path, else lines inside syntactic test regions.
    pub test_code: u32,
}

#[derive(Debug, Clone)]
pub struct FuncDef {
    pub name: String,
    /// 'fn' | 'method' | 'test'
    pub kind: String,
    /// 1-based line numbers, inclusive.
    pub start_line: u32,
    pub end_line: u32,
    /// xxh64 of the function body normalized (whitespace-collapsed,
    /// blank-lines dropped, comments retained). Stable across formatting
    /// churn; distinct value per real body rewrite.
    pub body_hash: u64,
}

#[derive(Debug, Clone)]
pub struct FileSnapshot {
    pub stat: FileStat,
    pub funcs: Vec<FuncDef>,
}

#[derive(Clone)]
struct CachedParse {
    language: String,
    code: u32,
    comments: u32,
    blanks: u32,
    /// Code lines inside syntactic test regions (path rule applied later).
    test_code: u32,
    funcs: Vec<FuncDef>,
}

const MAX_BLOB_BYTES: usize = 2 * 1024 * 1024;

/// Syntactic test-region detector per language. Regions are whole nodes
/// (attributes/decorators included); everything inside is test code.
#[derive(Copy, Clone, Debug)]
enum TestDetect {
    /// Path rules only.
    None,
    /// `#[cfg(test)]` items, `#[test]` / `#[<x>::test]` / `#[bench]` /
    /// `#[rstest]` / `#[test_case]` fns.
    Rust,
    /// `def test_*`, `class Test*` / `*TestCase` subclasses, pytest
    /// fixtures.
    Python,
    /// `func Test|Benchmark|Fuzz|Example<Upper|_|end>`.
    Go,
    /// Top-level `describe/it/test/...(...)` call statements (jest, mocha,
    /// vitest), incl. `.each/.only/.skip` chains.
    Js,
    /// Classes holding a JUnit/TestNG test-annotated method; such methods.
    Java,
    /// `def test_*`, minitest/Test::Unit classes, RSpec blocks.
    Ruby,
    /// GoogleTest / Catch2 / doctest macros (`TEST`, `TEST_F`, `TEST_CASE`…).
    Cpp,
    /// PHPUnit: classes extending `*TestCase`, `test*` methods.
    Php,
}

/// How a language's lines get classified.
#[derive(Clone)]
enum Parse {
    /// tree-sitter: comment nodes, functions, test regions.
    Grammar(Language),
    /// No grammar: comments by line prefix and block delimiters, lines
    /// only (no functions, path-rule tests only).
    Plain {
        line: &'static [&'static str],
        block: &'static [(&'static str, &'static str)],
    },
}

#[derive(Clone)]
struct LangSpec {
    label: &'static str,
    parse: Parse,
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
    /// Whole file names that carry no telling extension (`Makefile`).
    by_name: HashMap<&'static str, LangSpec>,
    parser: Parser,
}

/// Spec for a language without a grammar.
fn plain(
    label: &'static str,
    line: &'static [&'static str],
    block: &'static [(&'static str, &'static str)],
) -> LangSpec {
    LangSpec {
        label,
        parse: Parse::Plain { line, block },
        comment_kinds: &[],
        fn_kinds: &[],
        method_kinds: &[],
        method_ancestor_kinds: &[],
        test_detect: TestDetect::None,
    }
}

const HASH: &[&str] = &["#"];
const SLASH: &[&str] = &["//"];
const C_BLOCK: &[(&str, &str)] = &[("/*", "*/")];
const XML_BLOCK: &[(&str, &str)] = &[("<!--", "-->")];

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
                parse: Parse::Grammar(tree_sitter_rust::LANGUAGE.into()),
                comment_kinds: &["line_comment", "block_comment"],
                fn_kinds: &["function_item"],
                method_kinds: &[],
                method_ancestor_kinds: &["impl_item"],
                test_detect: TestDetect::Rust,
            },
        );
        add(
            &["py"],
            LangSpec {
                label: "Python",
                parse: Parse::Grammar(tree_sitter_python::LANGUAGE.into()),
                comment_kinds: &["comment"],
                fn_kinds: &["function_definition"],
                method_kinds: &[],
                method_ancestor_kinds: &["class_definition"],
                test_detect: TestDetect::Python,
            },
        );
        add(
            &["js", "mjs", "cjs", "jsx"],
            LangSpec {
                label: "JavaScript",
                parse: Parse::Grammar(tree_sitter_javascript::LANGUAGE.into()),
                comment_kinds: &["comment", "html_comment"],
                fn_kinds: &["function_declaration", "generator_function_declaration"],
                method_kinds: &["method_definition"],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::Js,
            },
        );
        add(
            &["ts"],
            LangSpec {
                label: "TypeScript",
                parse: Parse::Grammar(tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into()),
                comment_kinds: &["comment", "html_comment"],
                fn_kinds: &[
                    "function_declaration",
                    "function_signature",
                    "generator_function_declaration",
                ],
                method_kinds: &["method_definition", "method_signature"],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::Js,
            },
        );
        add(
            &["tsx"],
            LangSpec {
                label: "TSX",
                parse: Parse::Grammar(tree_sitter_typescript::LANGUAGE_TSX.into()),
                comment_kinds: &["comment", "html_comment"],
                fn_kinds: &[
                    "function_declaration",
                    "function_signature",
                    "generator_function_declaration",
                ],
                method_kinds: &["method_definition", "method_signature"],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::Js,
            },
        );
        add(
            &["go"],
            LangSpec {
                label: "Go",
                parse: Parse::Grammar(tree_sitter_go::LANGUAGE.into()),
                comment_kinds: &["comment"],
                fn_kinds: &["function_declaration"],
                method_kinds: &["method_declaration"],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::Go,
            },
        );
        add(
            &["c", "h"],
            LangSpec {
                label: "C",
                parse: Parse::Grammar(tree_sitter_c::LANGUAGE.into()),
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
                parse: Parse::Grammar(tree_sitter_cpp::LANGUAGE.into()),
                comment_kinds: &["comment"],
                fn_kinds: &["function_definition"],
                method_kinds: &[],
                method_ancestor_kinds: &["class_specifier", "struct_specifier"],
                test_detect: TestDetect::Cpp,
            },
        );
        add(
            &["lua"],
            LangSpec {
                label: "Lua",
                parse: Parse::Grammar(tree_sitter_lua::LANGUAGE.into()),
                comment_kinds: &["comment"],
                fn_kinds: &["function_declaration"],
                method_kinds: &[],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::None,
            },
        );
        add(
            &["vim"],
            LangSpec {
                label: "Vim Script",
                // Older API: `fn language() -> Language` instead of a
                // `LANGUAGE: LanguageFn` const like everyone else. No
                // .into() call.
                parse: Parse::Grammar(tree_sitter_vim::language()),
                comment_kinds: &["comment"],
                fn_kinds: &["function_definition"],
                method_kinds: &[],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::None,
            },
        );
        add(
            &["java"],
            LangSpec {
                label: "Java",
                parse: Parse::Grammar(tree_sitter_java::LANGUAGE.into()),
                comment_kinds: &["line_comment", "block_comment"],
                fn_kinds: &[],
                method_kinds: &["method_declaration", "constructor_declaration"],
                method_ancestor_kinds: &["class_declaration"],
                test_detect: TestDetect::Java,
            },
        );
        add(
            &["rb"],
            LangSpec {
                label: "Ruby",
                parse: Parse::Grammar(tree_sitter_ruby::LANGUAGE.into()),
                comment_kinds: &["comment"],
                fn_kinds: &["method"],
                method_kinds: &[],
                method_ancestor_kinds: &["class"],
                test_detect: TestDetect::Ruby,
            },
        );
        add(
            &["sh", "bash"],
            LangSpec {
                label: "Bash",
                parse: Parse::Grammar(tree_sitter_bash::LANGUAGE.into()),
                comment_kinds: &["comment"],
                fn_kinds: &["function_definition"],
                method_kinds: &[],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::None,
            },
        );
        add(
            &["html", "htm"],
            LangSpec {
                label: "HTML",
                parse: Parse::Grammar(tree_sitter_html::LANGUAGE.into()),
                comment_kinds: &["comment"],
                fn_kinds: &[],
                method_kinds: &[],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::None,
            },
        );
        add(
            &["css"],
            LangSpec {
                label: "CSS",
                parse: Parse::Grammar(tree_sitter_css::LANGUAGE.into()),
                comment_kinds: &["comment"],
                fn_kinds: &[],
                method_kinds: &[],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::None,
            },
        );
        add(
            &["json"],
            LangSpec {
                label: "JSON",
                parse: Parse::Grammar(tree_sitter_json::LANGUAGE.into()),
                comment_kinds: &[],
                fn_kinds: &[],
                method_kinds: &[],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::None,
            },
        );
        add(
            &["php"],
            LangSpec {
                label: "PHP",
                parse: Parse::Grammar(tree_sitter_php::LANGUAGE_PHP.into()),
                comment_kinds: &["comment"],
                fn_kinds: &["function_definition"],
                method_kinds: &["method_declaration"],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::Php,
            },
        );
        add(
            &["ml", "mli"],
            LangSpec {
                label: "OCaml",
                parse: Parse::Grammar(tree_sitter_ocaml::LANGUAGE_OCAML.into()),
                comment_kinds: &["comment"],
                fn_kinds: &["let_binding"],
                method_kinds: &[],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::None,
            },
        );
        add(
            &["scala", "sc"],
            LangSpec {
                label: "Scala",
                parse: Parse::Grammar(tree_sitter_scala::LANGUAGE.into()),
                comment_kinds: &["comment", "block_comment"],
                fn_kinds: &["function_definition"],
                method_kinds: &[],
                method_ancestor_kinds: &["class_definition", "object_definition"],
                test_detect: TestDetect::None,
            },
        );
        add(
            &["hs"],
            LangSpec {
                label: "Haskell",
                parse: Parse::Grammar(tree_sitter_haskell::LANGUAGE.into()),
                comment_kinds: &["comment"],
                fn_kinds: &["function"],
                method_kinds: &[],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::None,
            },
        );
        add(
            &["md", "markdown"],
            LangSpec {
                label: "Markdown",
                parse: Parse::Grammar(tree_sitter_md::LANGUAGE.into()),
                comment_kinds: &[],
                fn_kinds: &[],
                method_kinds: &[],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::None,
            },
        );
        add(
            &["kt", "kts"],
            LangSpec {
                label: "Kotlin",
                parse: Parse::Grammar(tree_sitter_kotlin_ng::LANGUAGE.into()),
                comment_kinds: &["line_comment", "block_comment"],
                fn_kinds: &["function_declaration"],
                method_kinds: &[],
                method_ancestor_kinds: &["class_declaration", "object_declaration"],
                test_detect: TestDetect::None,
            },
        );
        add(
            &["cs"],
            LangSpec {
                label: "C#",
                parse: Parse::Grammar(tree_sitter_c_sharp::LANGUAGE.into()),
                comment_kinds: &["comment"],
                fn_kinds: &["local_function_statement"],
                method_kinds: &["method_declaration", "constructor_declaration"],
                method_ancestor_kinds: &[],
                test_detect: TestDetect::None,
            },
        );
        add(
            &["swift"],
            LangSpec {
                label: "Swift",
                parse: Parse::Grammar(tree_sitter_swift::LANGUAGE.into()),
                comment_kinds: &["comment", "multiline_comment"],
                fn_kinds: &["function_declaration", "init_declaration"],
                method_kinds: &[],
                // class_declaration also covers struct / enum / extension.
                method_ancestor_kinds: &["class_declaration", "protocol_declaration"],
                test_detect: TestDetect::None,
            },
        );

        // No grammar: lines only. Labels follow tokei's names.
        add(&["yml", "yaml"], plain("YAML", HASH, &[]));
        add(&["toml"], plain("TOML", HASH, &[]));
        add(
            &["xml", "xsd", "xsl", "xslt", "plist"],
            plain("XML", &[], XML_BLOCK),
        );
        add(&["ini", "cfg"], plain("INI", &[";", "#"], &[]));
        add(&["dockerfile"], plain("Dockerfile", HASH, &[]));
        add(&["mk", "mak"], plain("Makefile", HASH, &[]));
        add(&["cmake"], plain("CMake", HASH, &[]));
        add(&["sql"], plain("SQL", &["--"], C_BLOCK));
        add(&["proto"], plain("Protocol Buffers", SLASH, C_BLOCK));
        add(&["nix"], plain("Nix", HASH, C_BLOCK));
        add(
            &["tf", "tfvars", "hcl"],
            plain("HCL", &["#", "//"], C_BLOCK),
        );
        add(&["scss", "sass"], plain("Sass", SLASH, C_BLOCK));
        add(&["less"], plain("LESS", SLASH, C_BLOCK));
        // ponytail: one comment syntax for the whole file; tokei splits the
        // <script>/<style> parts. Fine for LOC trends.
        add(
            &["vue"],
            plain("Vue", SLASH, &[("<!--", "-->"), ("/*", "*/")]),
        );
        add(
            &["svelte"],
            plain("Svelte", SLASH, &[("<!--", "-->"), ("/*", "*/")]),
        );
        add(&["pl", "pm"], plain("Perl", HASH, &[]));
        add(&["r"], plain("R", HASH, &[]));
        add(&["jl"], plain("Julia", HASH, &[("#=", "=#")]));
        add(&["ex", "exs"], plain("Elixir", HASH, &[]));
        add(&["erl", "hrl"], plain("Erlang", &["%"], &[]));
        add(&["dart"], plain("Dart", SLASH, C_BLOCK));
        add(&["zig"], plain("Zig", SLASH, &[]));
        add(&["m"], plain("Objective-C", SLASH, C_BLOCK));
        add(&["mm"], plain("Objective-C++", SLASH, C_BLOCK));
        add(&["groovy"], plain("Groovy", SLASH, C_BLOCK));
        add(&["gradle"], plain("Gradle", SLASH, C_BLOCK));
        add(&["ps1", "psm1"], plain("PowerShell", HASH, &[("<#", "#>")]));
        add(
            &["glsl", "vert", "frag", "geom", "comp", "tesc", "tese"],
            plain("GLSL", SLASH, C_BLOCK),
        );
        add(&["gd"], plain("GDScript", HASH, &[]));
        add(&["po", "pot"], plain("PO File", HASH, &[]));
        add(&["fish"], plain("Fish", HASH, &[]));
        add(&["zsh"], plain("Zsh", HASH, &[]));
        add(&["nu"], plain("Nushell", HASH, &[]));
        add(&["rst"], plain("ReStructuredText", &[], &[]));

        let mut by_name = HashMap::new();
        for (names, spec) in [
            (
                &["Makefile", "makefile", "GNUmakefile"][..],
                plain("Makefile", HASH, &[]),
            ),
            (
                &["Dockerfile", "Containerfile"],
                plain("Dockerfile", HASH, &[]),
            ),
            (&["CMakeLists.txt"], plain("CMake", HASH, &[])),
        ] {
            for n in names {
                by_name.insert(*n, spec.clone());
            }
        }

        Self {
            by_ext: m,
            by_name,
            parser: Parser::new(),
        }
    }

    pub fn label_for(&self, path: &str) -> Option<&'static str> {
        self.spec_for(path).map(|s| s.label)
    }

    fn spec_for(&self, path: &str) -> Option<&LangSpec> {
        let p = Path::new(path);
        let name = p.file_name().and_then(|s| s.to_str())?;
        if let Some(spec) = self.by_name.get(name) {
            return Some(spec);
        }
        let ext = p.extension().and_then(|s| s.to_str())?;
        self.by_ext.get(ext.to_ascii_lowercase().as_str())
    }
}

impl Default for LangRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Two-phase parallel treesitter for a set of sampled commits:
///
///   1. Serial tree walk per sha collects `(path, oid)` pairs and the
///      unique blob-id set. Cheap: no blob I/O, just tree traversal +
///      the extension registry.
///   2. Parallel parse of the unique blob set. Each rayon worker gets
///      its own `gix::Repository` + `LangRegistry` (both `!Sync`).
///      Because we parse each blob at most once, per-worker state is
///      just a scratch buffer — no lost dedup.
///   3. Assembly per sha reads from the parse map and applies the
///      path-based test-marker fallback.
///
/// This is a straight win over the in-loop serial `snapshot()` when
/// the repo has many sampled commits sharing files (the common case).
pub fn batch_sampled(repo: &Repo, sampled_shas: &[String]) -> HashMap<String, Vec<FileSnapshot>> {
    // Phase 1: enumerate sampled trees. Serial — tree walks touch loose
    // objects, which gix caches per-repo; parallel here would fight the
    // cache.
    let mut registry = LangRegistry::new();
    let mut per_sha: HashMap<String, Vec<(String, gix::ObjectId, &'static str)>> =
        HashMap::with_capacity(sampled_shas.len());
    // Parse unit = (blob, language): the same blob under two extensions
    // (e.g. an empty `__init__.py` and `mod.rs`) is two different parses.
    // Value = any path with that language; the spec is by extension, so
    // which one is irrelevant.
    let mut unique: HashMap<(gix::ObjectId, &'static str), String> = HashMap::new();

    for sha in sampled_shas {
        let entries = match enumerate_tree(&repo.git, sha, &mut registry) {
            Some(v) => v,
            None => continue,
        };
        for (path, oid, label) in &entries {
            unique.entry((*oid, label)).or_insert_with(|| path.clone());
        }
        per_sha.insert(sha.clone(), entries);
    }

    // Phase 2: parse each unique (blob, language) in parallel.
    let git_dir: PathBuf = repo.git.path().to_path_buf();
    let unique_vec: Vec<((gix::ObjectId, &'static str), String)> = unique.into_iter().collect();
    let parse_pairs: Vec<((gix::ObjectId, &'static str), Option<CachedParse>)> = unique_vec
        .par_iter()
        .map_init(
            || {
                (
                    gix::open(&git_dir).expect("open per-worker repo"),
                    LangRegistry::new(),
                )
            },
            |(git, reg), (key, sample_path)| (*key, parse_blob(git, key.0, sample_path, reg)),
        )
        .collect();
    let parse_map: HashMap<(gix::ObjectId, &'static str), Option<CachedParse>> =
        parse_pairs.into_iter().collect();

    // Phase 3: assemble per-sha FileSnapshot lists from the parse map.
    per_sha
        .into_iter()
        .map(|(sha, entries)| {
            let files = entries
                .into_iter()
                .filter_map(|(path, oid, label)| {
                    let c = parse_map.get(&(oid, label))?.clone()?;
                    let mut funcs = c.funcs;
                    let test_path = is_test_path(&path);
                    if test_path {
                        for f in funcs.iter_mut() {
                            f.kind = "test".to_string();
                        }
                    }
                    Some(FileSnapshot {
                        stat: FileStat {
                            path,
                            language: c.language,
                            code: c.code,
                            comments: c.comments,
                            blanks: c.blanks,
                            test_code: if test_path { c.code } else { c.test_code },
                        },
                        funcs,
                    })
                })
                .collect();
            (sha, files)
        })
        .collect()
}

/// Walk a commit's tree, return every blob path + its ObjectId + language
/// label, filtered to files the registry can classify. Serial; returns `None` on any
/// unrecoverable gix error (bad sha, missing tree, etc.).
fn enumerate_tree(
    git: &gix::Repository,
    sha: &str,
    registry: &mut LangRegistry,
) -> Option<Vec<(String, gix::ObjectId, &'static str)>> {
    let oid: gix::ObjectId = sha.parse().ok()?;
    let commit = git.find_object(oid).ok()?.try_into_commit().ok()?;
    let tree = commit.tree().ok()?;
    let entries = tree.traverse().breadthfirst.files().ok()?;

    let mut out = Vec::with_capacity(entries.len());
    for entry in entries {
        if !entry.mode.is_blob() {
            continue;
        }
        let path_str = entry.filepath.to_string();
        let Some(label) = registry.label_for(&path_str) else {
            continue;
        };
        out.push((path_str, entry.oid, label));
    }
    Some(out)
}

fn parse_blob(
    git: &gix::Repository,
    oid: gix::ObjectId,
    path: &str,
    registry: &mut LangRegistry,
) -> Option<CachedParse> {
    let blob = git.find_blob(oid).ok()?;
    if blob.data.len() > MAX_BLOB_BYTES {
        return None;
    }
    let head = &blob.data[..blob.data.len().min(8192)];
    if head.contains(&0u8) {
        return None;
    }

    let spec = registry.spec_for(path)?.clone();
    let (code, comments, blanks, test_code, funcs) = match &spec.parse {
        Parse::Plain { line, block } => {
            let ranges = plain_comment_ranges(&blob.data, line, block);
            let (c, m, b, t) = count_lines(&blob.data, &ranges, &[]);
            (c, m, b, t, Vec::new())
        }
        Parse::Grammar(language) => {
            registry.parser.set_language(language).ok()?;
            let tree: Tree = registry.parser.parse(&*blob.data, None)?;
            let root = tree.root_node();
            let tests = test_regions(&blob.data, root, spec.test_detect);
            let ranges = ts_comment_ranges(root, spec.comment_kinds);
            let (c, m, b, t) = count_lines(&blob.data, &ranges, &tests);
            let mut funcs = extract_funcs(&blob.data, root, &spec);
            mark_test_funcs(&mut funcs, &tests);
            (c, m, b, t, funcs)
        }
    };

    Some(CachedParse {
        language: spec.label.to_string(),
        code,
        comments,
        blanks,
        test_code,
        funcs,
    })
}

/// Whole-file test paths: directory conventions plus file-name rules that
/// the ecosystem's test runner itself keys on (Go `_test.go`, pytest
/// `test_*.py`, JUnit `*Test.java`, jest `*.test.ts`, RSpec `*_spec.rb`…).
fn is_test_path(path: &str) -> bool {
    const DIRS: &[&str] = &[
        "test",
        "tests",
        "__tests__",
        "__mocks__",
        "spec",
        "specs",
        "benches",
        "testdata",
        "e2e",
    ];
    let mut parts: Vec<&str> = path.split('/').collect();
    let Some(name) = parts.pop() else {
        return false;
    };
    if parts
        .iter()
        .any(|d| DIRS.contains(&d.to_ascii_lowercase().as_str()))
    {
        return true;
    }
    let (stem, ext) = match name.rsplit_once('.') {
        Some((s, e)) => (s, e.to_ascii_lowercase()),
        None => return false,
    };
    // `foo.test.ts`, `foo.spec.js`, `foo.test.d.ts`…
    if stem.split('.').skip(1).any(|p| p == "test" || p == "spec") {
        return true;
    }
    match ext.as_str() {
        "go" => stem.ends_with("_test"),
        "py" => stem.starts_with("test_") || stem.ends_with("_test") || stem == "conftest",
        "rb" => stem.ends_with("_test") || stem.ends_with("_spec") || stem.starts_with("test_"),
        "rs" => stem == "tests",
        "java" | "kt" | "kts" | "scala" | "php" | "hs" | "cs" | "swift" => {
            ["Test", "Tests", "IT", "Spec", "Suite", "TestCase"]
                .iter()
                .any(|s| stem.ends_with(s) && stem.len() > s.len())
        }
        "c" | "cc" | "cpp" | "cxx" => {
            stem.starts_with("test_") || stem.ends_with("_test") || stem.ends_with("_unittest")
        }
        "lua" => stem.ends_with("_spec") || stem.ends_with("_test"),
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" => stem.ends_with("_test"),
        _ => false,
    }
}

/// Walk the AST and emit one `FuncDef` per top-level (or method-level)
/// function declaration recognized by `spec`.
fn extract_funcs(source: &[u8], root: Node, spec: &LangSpec) -> Vec<FuncDef> {
    let mut out = Vec::new();
    walk_defs(source, root, spec, &mut out);
    // Minified code repeats a name on one line; `funcs` keys on (name, line).
    let mut seen = HashSet::new();
    out.retain(|f: &FuncDef| seen.insert((f.name.clone(), f.start_line)));
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

    let kind = if is_method || ancestor_matches(node, spec.method_ancestor_kinds) {
        "method"
    } else {
        "fn"
    }
    .to_string();

    let body = source
        .get(node.start_byte()..node.end_byte())
        .unwrap_or(&[]);
    let body_hash = hash_body_normalized(body);

    Some(FuncDef {
        name,
        kind,
        start_line,
        end_line,
        body_hash,
    })
}

/// xxh64 of the body with whitespace runs collapsed to single spaces per
/// line, trailing whitespace trimmed, and blank lines dropped. Comments are
/// kept (doc drift belongs in the comment-density lens, not here).
fn hash_body_normalized(body: &[u8]) -> u64 {
    use xxhash_rust::xxh64::Xxh64;

    let mut h = Xxh64::new(0);
    let mut line_buf: Vec<u8> = Vec::with_capacity(128);

    for line in body.split(|&b| b == b'\n') {
        line_buf.clear();
        let mut prev_ws = true;
        for &b in line {
            if b.is_ascii_whitespace() {
                if !prev_ws {
                    line_buf.push(b' ');
                    prev_ws = true;
                }
            } else {
                line_buf.push(b);
                prev_ws = false;
            }
        }
        while line_buf.last() == Some(&b' ') {
            line_buf.pop();
        }
        if !line_buf.is_empty() {
            h.update(&line_buf);
            h.update(b"\n");
        }
    }
    h.digest()
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

/// Row span (0-based, inclusive) of a syntactic test region.
type Region = (usize, usize);

/// Test regions of one parsed file, sorted and non-overlapping (a region
/// is never descended into).
fn test_regions(source: &[u8], root: Node, detect: TestDetect) -> Vec<Region> {
    let mut out = Vec::new();
    match detect {
        TestDetect::None => {}
        // Rust: `#![cfg(test)]` at file top → whole file.
        TestDetect::Rust
            if children(root)
                .any(|c| c.kind() == "inner_attribute_item" && rust_cfg_test(&text(source, c))) =>
        {
            let end = root.end_position();
            out.push((0, end.row - usize::from(end.column == 0 && end.row > 0)));
        }
        // JS: only top-level statements — `test(...)` deeper is app code.
        TestDetect::Js => {
            for c in children(root) {
                if js_test_stmt(source, c) {
                    out.push(span(c));
                }
            }
        }
        _ => walk_regions(source, root, detect, &mut out),
    }
    out
}

fn walk_regions(source: &[u8], node: Node, detect: TestDetect, out: &mut Vec<Region>) {
    if let Some(r) = region_of(source, node, detect) {
        out.push(r);
        return;
    }
    for c in children(node) {
        walk_regions(source, c, detect, out);
    }
}

fn region_of(source: &[u8], node: Node, detect: TestDetect) -> Option<Region> {
    let name = || {
        node.child_by_field_name("name")
            .map(|n| text(source, n))
            .unwrap_or_default()
    };
    let hit = match (detect, node.kind()) {
        (TestDetect::Rust, k) if k.ends_with("_item") && k != "attribute_item" => {
            // Attributes (and doc comments between them) are preceding
            // siblings; the region starts at the first one.
            let mut start = node;
            let mut test = false;
            let mut prev = node.prev_sibling();
            while let Some(p) = prev {
                match p.kind() {
                    "attribute_item" => {
                        let t = text(source, p);
                        test |= rust_cfg_test(&t) || (k == "function_item" && rust_test_attr(&t));
                        start = p;
                    }
                    "line_comment" | "block_comment" => {}
                    _ => break,
                }
                prev = p.prev_sibling();
            }
            return test.then(|| (start.start_position().row, node.end_position().row));
        }
        (TestDetect::Python, "decorated_definition") => {
            let def = node.child_by_field_name("definition")?;
            let fixture = children(node)
                .filter(|c| c.kind() == "decorator")
                .any(|d| py_is_fixture(&text(source, d)));
            fixture || py_test_def(source, def)
        }
        (TestDetect::Python, "function_definition" | "class_definition") => {
            py_test_def(source, node)
        }
        (TestDetect::Go, "function_declaration") => go_test_name(&name()),
        (TestDetect::Java, "class_declaration") => {
            let sup = node
                .child_by_field_name("superclass")
                .map(|n| text(source, n))
                .unwrap_or_default();
            sup.trim_end().ends_with("TestCase") || has_java_test_method(source, node)
        }
        (TestDetect::Java, "method_declaration") => java_test_method(source, node),
        (TestDetect::Ruby, "method") => name().starts_with("test_"),
        (TestDetect::Ruby, "class") => {
            let sup = node
                .child_by_field_name("superclass")
                .map(|n| text(source, n))
                .unwrap_or_default();
            let sup = sup.trim_start_matches('<').trim();
            sup.ends_with("Test") || sup.ends_with("TestCase") || sup.ends_with("Spec")
        }
        (TestDetect::Ruby, "call") => ruby_spec_block(source, node),
        (TestDetect::Cpp, "function_definition") => {
            let decl = node.child_by_field_name("declarator")?;
            decl.kind() == "function_declarator"
                && decl
                    .child_by_field_name("declarator")
                    .is_some_and(|d| CPP_TEST_MACROS.contains(&text(source, d).as_str()))
        }
        (TestDetect::Php, "class_declaration") => children(node)
            .filter(|c| c.kind() == "base_clause")
            .any(|b| text(source, b).trim_end().ends_with("TestCase")),
        _ => false,
    };
    hit.then(|| span(node))
}

fn span(node: Node) -> Region {
    (node.start_position().row, node.end_position().row)
}

fn children(node: Node) -> impl Iterator<Item = Node> {
    (0..node.child_count()).filter_map(move |i| node.child(i as u32))
}

fn text(source: &[u8], node: Node) -> String {
    String::from_utf8_lossy(&source[node.start_byte()..node.end_byte()]).into_owned()
}

/// `#[test]`, `#[tokio::test(..)]`, `#[bench]`, `#[rstest]`,
/// `#[test_case(..)]`, `#[wasm_bindgen_test]`, `#[quickcheck]`.
fn rust_test_attr(attr: &str) -> bool {
    let body: String = attr.chars().filter(|c| !c.is_whitespace()).collect();
    let Some(body) = body.strip_prefix("#[") else {
        return false;
    };
    let path = body.split(['(', '=', ']']).next().unwrap_or("");
    let last = path.rsplit("::").next().unwrap_or("");
    matches!(last, "test" | "bench" | "rstest" | "quickcheck")
        || last.starts_with("test_")
        || last.ends_with("_test")
}

/// `#[cfg(P)]` / `#![cfg(P)]` where predicate P can only hold under
/// `cfg(test)`: `test`, `all(.., test, ..)`, `any(..)` with every arm
/// implying test. `not(..)` never implies test.
fn rust_cfg_test(attr: &str) -> bool {
    let body: String = attr.chars().filter(|c| !c.is_whitespace()).collect();
    let body = body.trim_start_matches('#').trim_start_matches('!');
    let Some(pred) = body
        .strip_prefix("[cfg(")
        .and_then(|b| b.strip_suffix(")]"))
    else {
        return false;
    };
    cfg_implies_test(pred)
}

fn cfg_implies_test(p: &str) -> bool {
    let split_args = |inner: &str| -> Vec<String> {
        let (mut depth, mut cur, mut out) = (0i32, String::new(), Vec::new());
        for ch in inner.chars() {
            match ch {
                '(' => depth += 1,
                ')' => depth -= 1,
                ',' if depth == 0 => {
                    out.push(std::mem::take(&mut cur));
                    continue;
                }
                _ => {}
            }
            cur.push(ch);
        }
        if !cur.is_empty() {
            out.push(cur);
        }
        out
    };
    let inner = |f: &str| p.strip_prefix(f).and_then(|r| r.strip_suffix(')'));
    if p == "test" {
        true
    } else if let Some(i) = inner("all(") {
        split_args(i).iter().any(|a| cfg_implies_test(a))
    } else if let Some(i) = inner("any(") {
        let args = split_args(i);
        !args.is_empty() && args.iter().all(|a| cfg_implies_test(a))
    } else {
        false
    }
}

/// pytest defaults (`python_functions = test*`, `python_classes = Test*`)
/// plus unittest `*TestCase` subclasses.
fn py_test_def(source: &[u8], def: Node) -> bool {
    let name = def
        .child_by_field_name("name")
        .map(|n| text(source, n))
        .unwrap_or_default();
    match def.kind() {
        "function_definition" => name.starts_with("test"),
        "class_definition" => {
            name.starts_with("Test")
                || def.child_by_field_name("superclasses").is_some_and(|s| {
                    text(source, s)
                        .trim_matches(|c| c == '(' || c == ')')
                        .split(',')
                        .any(|b| b.trim().ends_with("TestCase"))
                })
        }
        _ => false,
    }
}

fn py_is_fixture(decorator: &str) -> bool {
    let d = decorator.trim_start_matches('@').trim();
    let path = d.split('(').next().unwrap_or("");
    path == "fixture" || path.ends_with(".fixture")
}

/// `go test` rule: Test/Benchmark/Fuzz/Example then end, `_`, or a
/// non-lowercase rune (`Testify` is not a test).
fn go_test_name(name: &str) -> bool {
    ["Test", "Benchmark", "Fuzz", "Example"].iter().any(|p| {
        name.strip_prefix(p)
            .is_some_and(|rest| rest.chars().next().is_none_or(|c| !c.is_lowercase()))
    })
}

const JS_TEST_FNS: &[&str] = &[
    "describe",
    "it",
    "test",
    "suite",
    "context",
    "specify",
    "fdescribe",
    "xdescribe",
    "fit",
    "xit",
    "xtest",
];
const JS_HOOKS: &[&str] = &[
    "beforeEach",
    "afterEach",
    "beforeAll",
    "afterAll",
    "before",
    "after",
];

/// `describe('x', fn)` / `it.each(t)('x', fn)` / `beforeEach(fn)` as a
/// statement. Named blocks need a string title, hooks a function arg —
/// keeps app-level `test(x)` calls out.
fn js_test_stmt(source: &[u8], stmt: Node) -> bool {
    if stmt.kind() != "expression_statement" {
        return false;
    }
    let Some(call) = stmt
        .named_child(0)
        .filter(|c| c.kind() == "call_expression")
    else {
        return false;
    };
    let mut callee = call.child_by_field_name("function");
    while let Some(c) = callee {
        match c.kind() {
            "member_expression" => callee = c.child_by_field_name("object"),
            "call_expression" => callee = c.child_by_field_name("function"),
            _ => break,
        }
    }
    let Some(root) = callee.filter(|c| c.kind() == "identifier") else {
        return false;
    };
    let root = text(source, root);
    let first_arg = call
        .child_by_field_name("arguments")
        .and_then(|a| a.named_child(0))
        .map(|a| a.kind());
    if JS_TEST_FNS.contains(&root.as_str()) {
        matches!(first_arg, Some("string" | "template_string"))
    } else if JS_HOOKS.contains(&root.as_str()) {
        matches!(
            first_arg,
            Some("arrow_function" | "function_expression" | "function")
        )
    } else {
        false
    }
}

const JAVA_TEST_ANNOTATIONS: &[&str] = &[
    "Test",
    "ParameterizedTest",
    "RepeatedTest",
    "TestFactory",
    "TestTemplate",
    "Property",
];

fn java_test_method(source: &[u8], method: Node) -> bool {
    children(method)
        .filter(|c| c.kind() == "modifiers")
        .flat_map(children)
        .filter(|a| matches!(a.kind(), "marker_annotation" | "annotation"))
        .filter_map(|a| a.child_by_field_name("name"))
        .any(|n| {
            let t = text(source, n);
            JAVA_TEST_ANNOTATIONS.contains(&t.rsplit('.').next().unwrap_or(""))
        })
}

fn has_java_test_method(source: &[u8], node: Node) -> bool {
    children(node).any(|c| {
        (c.kind() == "method_declaration" && java_test_method(source, c))
            || has_java_test_method(source, c)
    })
}

const RSPEC_BLOCKS: &[&str] = &[
    "describe",
    "context",
    "it",
    "specify",
    "shared_examples",
    "shared_context",
    "feature",
    "scenario",
];

/// RSpec `describe/context/it ... do` blocks, bare or on `RSpec`.
fn ruby_spec_block(source: &[u8], call: Node) -> bool {
    let method = call
        .child_by_field_name("method")
        .map(|m| text(source, m))
        .unwrap_or_default();
    let receiver_ok = call
        .child_by_field_name("receiver")
        .is_none_or(|r| text(source, r) == "RSpec");
    let has_block = call.child_by_field_name("block").is_some()
        || children(call).any(|c| matches!(c.kind(), "do_block" | "block"));
    receiver_ok && has_block && RSPEC_BLOCKS.contains(&method.as_str())
}

const CPP_TEST_MACROS: &[&str] = &[
    "TEST",
    "TEST_F",
    "TEST_P",
    "TYPED_TEST",
    "TYPED_TEST_P",
    "TEST_CASE",
    "TEST_CASE_METHOD",
    "TEMPLATE_TEST_CASE",
    "SCENARIO",
    "BOOST_AUTO_TEST_CASE",
    "BOOST_FIXTURE_TEST_CASE",
];

/// A function is a test when its first line falls in a test region.
fn mark_test_funcs(funcs: &mut [FuncDef], regions: &[Region]) {
    for f in funcs {
        if in_regions(f.start_line as usize - 1, regions) {
            f.kind = "test".to_string();
        }
    }
}

fn in_regions(row: usize, regions: &[Region]) -> bool {
    let i = regions.partition_point(|r| r.0 <= row);
    i > 0 && row <= regions[i - 1].1
}

/// (code, comments, blanks, test_code) — test_code counts code lines
/// whose row is inside a test region.
/// Byte ranges of `comment_kinds` nodes, sorted by start.
fn ts_comment_ranges(root: Node, comment_kinds: &[&str]) -> Vec<(usize, usize)> {
    let mut ranges = Vec::new();
    collect_comment_ranges(root, comment_kinds, &mut ranges);
    ranges.sort_unstable_by_key(|r| r.0);
    ranges
}

/// Comment byte ranges for a grammar-less language: a `line` prefix runs to
/// end of line, a `block` pair spans to its close (or end of file). Skips
/// double-quoted strings so `"src/*.rs"` opens nothing. Sorted, disjoint.
///
/// ponytail: double quotes only, no per-language string or raw-string
/// rules. A line marker inside a single-quoted string misfiles that
/// line's tail (harmless: code before it keeps the line code); a block
/// opener there swallows lines up to the next close. Add per-language
/// quotes if tokei drift shows up on a fixture.
fn plain_comment_ranges(src: &[u8], line: &[&str], block: &[(&str, &str)]) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < src.len() {
        let rest = &src[i..];
        if rest[0] == b'"' {
            // Skip to the closing quote on the same line.
            let mut j = i + 1;
            while j < src.len() && src[j] != b'"' && src[j] != b'\n' {
                j += if src[j] == b'\\' { 2 } else { 1 };
            }
            i = (j + 1).min(src.len());
            continue;
        }
        if let Some((open, close)) = block.iter().find(|(o, _)| rest.starts_with(o.as_bytes())) {
            let body = i + open.len();
            let end = src[body..]
                .windows(close.len())
                .position(|w| w == close.as_bytes())
                .map_or(src.len(), |p| body + p + close.len());
            out.push((i, end));
            i = end;
            continue;
        }
        if line.iter().any(|p| rest.starts_with(p.as_bytes())) {
            let end = rest
                .iter()
                .position(|&b| b == b'\n')
                .map_or(src.len(), |p| i + p);
            out.push((i, end));
            i = end;
            continue;
        }
        i += 1;
    }
    out
}

fn count_lines(source: &[u8], ranges: &[(usize, usize)], tests: &[Region]) -> (u32, u32, u32, u32) {
    if source.is_empty() {
        return (0, 0, 0, 0);
    }

    let mut blanks = 0u32;
    let mut comments = 0u32;
    let mut code = 0u32;
    let mut test_code = 0u32;
    let mut row = 0usize;

    let mut classify = |start: usize, end: usize| {
        let line = &source[start..end];
        match classify_line(line, start, ranges) {
            LineKind::Blank => blanks += 1,
            LineKind::Comment => comments += 1,
            LineKind::Code => {
                code += 1;
                if in_regions(row, tests) {
                    test_code += 1;
                }
            }
        }
        row += 1;
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
    (code, comments, blanks, test_code)
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

/// `ranges` sorted by start and disjoint (comment nodes don't nest in
/// collection) → binary search instead of a linear scan per byte.
fn byte_inside_any(abs: usize, ranges: &[(usize, usize)]) -> bool {
    let i = ranges.partition_point(|r| r.0 <= abs);
    i > 0 && abs < ranges[i - 1].1
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

    impl LangSpec {
        fn grammar(&self) -> &Language {
            match &self.parse {
                Parse::Grammar(l) => l,
                Parse::Plain { .. } => panic!("{} has no grammar", self.label),
            }
        }
    }

    fn count(lang: Language, comment_kinds: &[&str], src: &str) -> (u32, u32, u32) {
        let mut parser = Parser::new();
        parser.set_language(&lang).unwrap();
        let tree = parser.parse(src.as_bytes(), None).unwrap();
        let (c, m, b, _) = count_lines(
            src.as_bytes(),
            &ts_comment_ranges(tree.root_node(), comment_kinds),
            &[],
        );
        (c, m, b)
    }

    fn parse_and_extract(src: &str, spec: &LangSpec) -> Vec<FuncDef> {
        let mut parser = Parser::new();
        parser.set_language(spec.grammar()).unwrap();
        let tree = parser.parse(src.as_bytes(), None).unwrap();
        let mut funcs = extract_funcs(src.as_bytes(), tree.root_node(), spec);
        let regions = test_regions(src.as_bytes(), tree.root_node(), spec.test_detect);
        mark_test_funcs(&mut funcs, &regions);
        funcs
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
        let (code, comments, blanks) =
            count(tree_sitter_python::LANGUAGE.into(), &["comment"], src);
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
        let (code, comments, blanks) = count(tree_sitter_go::LANGUAGE.into(), &["comment"], src);
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
        let (code, comments, blanks) = count(tree_sitter_cpp::LANGUAGE.into(), &["comment"], src);
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
        assert_eq!(r.label_for("Cargo.toml"), Some("TOML"));
        assert_eq!(r.label_for("a/b/Makefile"), Some("Makefile"));
        assert_eq!(r.label_for("CMakeLists.txt"), Some("CMake"));
        assert_eq!(r.label_for("notes.txt"), None);
        assert_eq!(r.label_for("x.KT"), Some("Kotlin"));
        assert!(r.spec_for("README.md").is_some()); // Markdown grammar added
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
        let by_name: HashMap<_, _> = defs
            .iter()
            .map(|d| (d.name.clone(), d.kind.clone()))
            .collect();
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
        let by_name: HashMap<_, _> = defs
            .iter()
            .map(|d| (d.name.clone(), d.kind.clone()))
            .collect();
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
        let by_name: HashMap<_, _> = defs
            .iter()
            .map(|d| (d.name.clone(), d.kind.clone()))
            .collect();
        assert_eq!(by_name.get("Regular").map(|s| s.as_str()), Some("fn"));
        assert_eq!(
            by_name.get("TestSomething").map(|s| s.as_str()),
            Some("test")
        );
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
        let by_name: HashMap<_, _> = defs
            .iter()
            .map(|d| (d.name.clone(), d.kind.clone()))
            .collect();
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
        let by_name: HashMap<_, _> = defs
            .iter()
            .map(|d| (d.name.clone(), d.kind.clone()))
            .collect();
        assert_eq!(by_name.get("top").map(|s| s.as_str()), Some("fn"));
        assert_eq!(by_name.get("m").map(|s| s.as_str()), Some("method"));
    }

    fn kinds(ext: &str, src: &str) -> HashMap<String, String> {
        let reg = LangRegistry::new();
        parse_and_extract(src, &spec_for(&reg, ext))
            .into_iter()
            .map(|d| (d.name, d.kind))
            .collect()
    }

    #[test]
    fn kotlin_extracts_fn_and_method() {
        let k = kinds(
            "kt",
            "fun top() {}\n\nclass C {\n    fun m() = 1\n}\n\nobject O {\n    fun o() {}\n}\n",
        );
        assert_eq!(k["top"], "fn");
        assert_eq!(k["m"], "method");
        assert_eq!(k["o"], "method");
    }

    #[test]
    fn csharp_extracts_methods_and_local_fn() {
        let k = kinds(
            "cs",
            "class C {\n  C() {}\n  int M() { int L() => 1; return L(); }\n}\n",
        );
        assert_eq!(k["C"], "method");
        assert_eq!(k["M"], "method");
        // Local fns live inside a method body: not top-level defs.
        assert!(!k.contains_key("L"));
    }

    #[test]
    fn swift_extracts_fn_and_method() {
        let k = kinds(
            "swift",
            "func top() {}\nstruct S { func m() {} }\nextension S { func e() {} }\n",
        );
        assert_eq!(k["top"], "fn");
        assert_eq!(k["m"], "method");
        assert_eq!(k["e"], "method");
    }

    fn plain_count(path: &str, src: &str) -> (u32, u32, u32) {
        let reg = LangRegistry::new();
        let Parse::Plain { line, block } = reg.spec_for(path).unwrap().parse else {
            panic!("{path} has a grammar");
        };
        let ranges = plain_comment_ranges(src.as_bytes(), line, block);
        let (c, m, b, _) = count_lines(src.as_bytes(), &ranges, &[]);
        (c, m, b)
    }

    #[test]
    fn plain_counts_line_comments() {
        // code, comment, blank, code with trailing comment
        assert_eq!(plain_count("a.yaml", "a: 1\n# c\n\nb: 2 # c\n"), (2, 1, 1));
        // `#` inside a string opens nothing
        assert_eq!(plain_count("a.toml", "x = \"#a\"\n  # c\n"), (1, 1, 0));
    }

    #[test]
    fn plain_counts_block_comments() {
        let sql = "/* a\n b */\nSELECT 1; -- c\n-- d\nSELECT '/' || \"/*\";\n";
        assert_eq!(plain_count("q.sql", sql), (2, 3, 0));
        // unclosed block runs to end of file
        assert_eq!(plain_count("a.proto", "message M {}\n/* x\ny\n"), (1, 2, 0));
        assert_eq!(plain_count("a.xml", "<a>\n<!-- c -->\n</a>\n"), (2, 1, 0));
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
        assert!(is_test_path("pkg/foo_test.go"));
        assert!(!is_test_path("pkg/footest.go"));
        assert!(is_test_path("app/conftest.py"));
        assert!(is_test_path("src/main/FooTest.java"));
        assert!(!is_test_path("src/main/Test.java"));
        assert!(!is_test_path("src/main/Contest.java"));
        assert!(is_test_path("spec/models/user_spec.rb"));
        assert!(is_test_path("src/a/tests.rs"));
        assert!(!is_test_path("src/latest/x.rs"));
        assert!(!is_test_path("src/attest.py"));
    }

    #[test]
    fn is_test_path_overrides_kind_in_snapshot() {
        // Just check the helper; the snapshot integration is covered by
        // index::tests::smoke_index_tiny_repo via funcs table.
        assert!(is_test_path("tests/mod.rs"));
    }

    /// 1-based inclusive line spans of the test regions in `src`.
    fn regions(ext: &str, src: &str) -> Vec<(usize, usize)> {
        let reg = LangRegistry::new();
        let spec = spec_for(&reg, ext);
        let mut parser = Parser::new();
        parser.set_language(spec.grammar()).unwrap();
        let tree = parser.parse(src.as_bytes(), None).unwrap();
        test_regions(src.as_bytes(), tree.root_node(), spec.test_detect)
            .into_iter()
            .map(|(a, b)| (a + 1, b + 1))
            .collect()
    }

    #[test]
    fn rust_test_regions() {
        let src = "\
fn prod() {}
#[cfg(not(test))]
fn not_test() {}
/// doc
#[test]
fn t() {}
#[tokio::test(flavor = \"multi_thread\")]
async fn t2() {}
#[doc = \"latest\"]
fn also_prod() {}
#[cfg(all(test, feature = \"x\"))]
mod tests {
    use super::*;
    fn helper() {}
}
#[cfg(any(test, debug_assertions))]
fn maybe() {}
";
        assert_eq!(regions("rs", src), [(5, 6), (7, 8), (11, 15)]);
        assert_eq!(regions("rs", "#![cfg(test)]\nfn a() {}\n"), [(1, 2)]);
    }

    #[test]
    fn rust_cfg_predicates() {
        assert!(rust_cfg_test("#[cfg(test)]"));
        assert!(rust_cfg_test("#[cfg( all( unix, test ) )]"));
        assert!(rust_cfg_test("#[cfg(any(test, all(test, unix)))]"));
        assert!(!rust_cfg_test("#[cfg(any(test, unix))]"));
        assert!(!rust_cfg_test("#[cfg(not(test))]"));
        assert!(!rust_cfg_test("#[cfg_attr(test, derive(Debug))]"));
    }

    #[test]
    fn python_test_regions() {
        let src = "\
def prod(): pass
def test_a():
    pass
@pytest.fixture
def db():
    return 1
class TestX:
    def helper(self): pass
class Foo(unittest.TestCase):
    pass
class Testimony: pass
@property
def contest(): pass
";
        // pytest's `Test*` prefix rule includes `Testimony` — matches the runner.
        assert_eq!(
            regions("py", src),
            [(2, 3), (4, 6), (7, 8), (9, 10), (11, 11)]
        );
    }

    #[test]
    fn go_test_regions() {
        let src = "\
package p
func Testify() {}
func TestA(t *testing.T) {}
func Test(t *testing.T) {}
func BenchmarkB(b *testing.B) {}
func Test_c(t *testing.T) {}
func ExampleFoo() {}
func FuzzX(f *testing.F) {}
";
        assert_eq!(
            regions("go", src),
            [(3, 3), (4, 4), (5, 5), (6, 6), (7, 7), (8, 8)]
        );
    }

    #[test]
    fn js_ts_test_regions() {
        let src = "\
import x from 'y';
describe('suite', () => {
  it('a', () => {});
});
test.each([1])('t %s', (n) => {});
beforeEach(() => {});
test(value);
function f() { it('nested', () => {}); }
";
        assert_eq!(regions("ts", src), [(2, 4), (5, 5), (6, 6)]);
        assert_eq!(regions("js", src), [(2, 4), (5, 5), (6, 6)]);
    }

    #[test]
    fn java_test_regions() {
        let src = "\
class Prod {
  void run() {}
}
class FooTest {
  private int x;
  @Test
  void a() {}
}
class Legacy extends junit.framework.TestCase {
}
";
        assert_eq!(regions("java", src), [(4, 8), (9, 10)]);
    }

    #[test]
    fn ruby_test_regions() {
        let src = "\
def prod; end
class FooTest < Minitest::Test
  def helper; end
end
RSpec.describe Foo do
  it 'works' do
  end
end
describe 'x' do
end
it_is = 1
";
        assert_eq!(regions("rb", src), [(2, 4), (5, 8), (9, 10)]);
    }

    #[test]
    fn cpp_test_regions() {
        let src = "\
int prod() { return 1; }
TEST(Suite, Name) {
  EXPECT_EQ(1, 1);
}
TEST_F(Fixture, Name) {
}
";
        assert_eq!(regions("cpp", src), [(2, 4), (5, 6)]);
    }

    #[test]
    fn php_test_regions() {
        let src = "\
<?php
class Prod {}
class FooTest extends TestCase {
  public function testA() {}
}
";
        assert_eq!(regions("php", src), [(3, 5)]);
    }

    #[test]
    fn test_code_counts_only_code_lines_in_regions() {
        let src = "\
fn prod() {}

#[cfg(test)]
mod tests {
    // comment

    #[test]
    fn t() {}
}
";
        let reg = LangRegistry::new();
        let spec = spec_for(&reg, "rs");
        let mut parser = Parser::new();
        parser.set_language(spec.grammar()).unwrap();
        let tree = parser.parse(src.as_bytes(), None).unwrap();
        let r = test_regions(src.as_bytes(), tree.root_node(), spec.test_detect);
        let (code, comments, blanks, test_code) = count_lines(
            src.as_bytes(),
            &ts_comment_ranges(tree.root_node(), spec.comment_kinds),
            &r,
        );
        assert_eq!((code, comments, blanks, test_code), (6, 1, 2, 5));
    }

    #[test]
    fn body_hash_is_whitespace_insensitive() {
        let a = b"fn f() {\n    let x = 1;\n    x + 2\n}";
        let b = b"fn f() {\n\tlet   x   =   1;\n    x + 2\n}\n";
        let c = b"fn f() {\n\n\n    let x = 1;\n    x + 2\n}\n";
        assert_eq!(hash_body_normalized(a), hash_body_normalized(b));
        assert_eq!(hash_body_normalized(a), hash_body_normalized(c));
    }

    #[test]
    fn body_hash_differs_on_logic_change() {
        let a = b"fn f() { x + 1 }";
        let b = b"fn f() { x + 2 }";
        assert_ne!(hash_body_normalized(a), hash_body_normalized(b));
    }

    #[test]
    fn body_hash_detects_comment_edit() {
        // Comments are retained in the hash — doc drift belongs to the
        // comment-density lens, but a rewrite that only edits comments still
        // shows as distinct here. Alternative (strip comments) would hide
        // AI-era comment inflation from func_longevity entirely.
        let a = b"// old\nfn f() { 1 }";
        let b = b"// new\nfn f() { 1 }";
        assert_ne!(hash_body_normalized(a), hash_body_normalized(b));
    }
}
