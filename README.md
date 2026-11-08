# git-archaeologist

[![CI](https://github.com/neumann-mlucas/git-archaeologist/actions/workflows/ci.yml/badge.svg)](https://github.com/neumann-mlucas/git-archaeologist/actions/workflows/ci.yml)
[![License](https://img.shields.io/github/license/neumann-mlucas/git-archaeologist)](LICENSE)

Time series from a git repository's history: LOC by language, author and
module, cohort survival, file coupling, function churn, commit types, file
age. Output is TSV / CSV / JSON / Parquet; pipe it to your plotter.

One DuckDB cache per repo. After the first index every query returns in
under a second.

```
$ git-archaeologist stats          # uv, top rows
Lang      Files    Code  Comments  Test %  Funcs  Age Days  Owner
Rust        485  187012     25783    51.4   5557       830  Charlie Marsh
other       177                                0       817  Andrew Gallant
Markdown     75   16492         0     0.0      0       822  Zanie Blue
TOML         98    4924       499     0.0      0       817  Andrew Gallant
Python       66    4407       480     8.0    181       838  Charlie Marsh
YAML         11    2827       202     0.0      0       922  Charlie Marsh
Total       931  232025     27127    41.6   5740       823  Charlie Marsh
```

Status: **v1 in development.** Design in [docs/SPEC.md](docs/SPEC.md),
plan in [docs/TASKS.md](docs/TASKS.md).

## Install

Build from source until `cargo dist` prebuilts land. Needs system
`libduckdb` (Arch `pacman -S duckdb`, Debian / Ubuntu
`apt install libduckdb-dev`, macOS `brew install duckdb`).

```sh
cargo install --path .
```

Binary is `git-archaeologist`. Alias to `git-arch` if you prefer.

## Quick start

```sh
cd repo
git-archaeologist stats                       # tokei-style snapshot + age, owner, churn
git-archaeologist burndown --by language      # LOC per language over time
git-archaeologist survival --fit exp          # half-life of a line, in days
git-archaeologist coupling --top 20           # files that change together
```

The first query builds the cache (`index` does it explicitly).

## Subcommands

Samples are real output from the pinned bench fixtures
(`benches/fixtures.toml`), mostly godot 4.3 rolled up to years:

| fixture                                           | commits (non-merge) | authors | span              | bucket | index  | cache  |
| ------------------------------------------------- | ------------------- | ------- | ----------------- | ------ | ------ | ------ |
| [ratatui](https://github.com/ratatui/ratatui)     | 2,589 (2,574)       | 367     | 2016-10 → 2026-07 | day    | 38 s   | 149 MB |
| [uv](https://github.com/astral-sh/uv)             | 13,164 (13,111)     | 725     | 2023-10 → 2026-08 | week   | 213 s  | 175 MB |
| [godot](https://github.com/godotengine/godot) 4.3 | 66,099 (36,084)     | 3,258   | 2013-04 → 2024-08 | month  | 69 min | 1.0 GB |

Every ref is walked, so commit counts exceed `git rev-list --count HEAD`.

### `stats`

Snapshot of HEAD (parsed from git, not the working tree), grouped
`--by lang|dir|file` (`--depth N` for `dir`), with a `Total` row. Beyond
tokei's line counts: test code and its share of code lines (see
Test detection under [Reference](#reference)), function count and longest function,
median age of surviving lines, top owner, 90-day churn.

Counts every tracked file, dotfiles included (tokei skips them by
default). Files with no known language show as `other`, lines only.

`--rev <commit>` snapshots another commit. `--rev A..B` prints the
per-group delta B − A; an empty side means `HEAD`, so `--rev v1.0..`
is "what changed since v1.0". Both are parse-only: no age, owner or
churn, and files with no grammar are left out. uv, `--rev 0.4.0..`:

```
Lang      Files  Lines   Code  Test Code  Test Pct
Rust        111  47274  38180      22563         2
JSON          1   7267   7267          0         0
Markdown      5   5451   3751          0         0
Python        2    607    480        297       6.6
Total       131  61665  50513      22860         1
```

### `burndown`

Cumulative LOC per bucket, `--by language | author | module | kind`.
Godot's six biggest languages at year end (`C` includes `.h` headers):

```
year          C++          C   PO File      XML     C#    GLSL
2014      269,794    586,674         -   82,704      -   1,253
2016      316,858    706,158   111,684   51,278      -   1,530
2018      545,254    775,210   246,266   67,220  6,959   7,046
2020      817,543    883,885 1,332,796   85,010 14,707  10,786
2022    1,202,540    968,933   892,000  115,973 27,849  20,195
2024    1,299,960  1,186,469   467,164  145,363 34,947  22,276
```

C++ overtook C in 2022. Translation catalogs ballooned to 1.3M lines in
2020 and have shrunk since. `--by kind` splits test code from the rest:
at 4.3, godot is 3.16M lines of code and 52k of tests.

### `survival`

Kaplan-Meier survival per birth cohort. `--fit exp` fits
`ln(survival) = −λ·age` and reports the half-life in days:

```
metric      value   reason
half_life   915.241 ok (n=124, deaths=11293117)
```

Godot cohorts:

```
year       born      alive        dead  survival%
2014  2,288,926    290,709   1,998,217      12.7%
2016    923,515     65,348     858,167       7.1%
2018    815,886    153,942     661,944      18.9%
2020  3,113,651    671,919   2,441,732      21.6%
2021  1,144,092    480,393     663,699      42.0%
2022  2,272,324    672,826   1,599,498      29.6%
2023  1,518,544  1,172,834     345,710      77.2%
2024    723,117    661,815      61,302      91.5%
```

87% of the 2014 open-source release is gone; the Godot 4 rewrite years
churned millions of lines that did not last.

Lines are tracked along HEAD's first-parent chain, so lines from a merged
branch are born at the merge that landed them.

### `cohort`

Surviving LOC per bucket, stacked by birth cohort
(`bucket, cohort, alive_lines`): the series behind a git-of-theseus
stream plot.

### `coupling`

Top-N file pairs by how often they change in the same commit.
`--max-files-per-commit N` (default 50) drops squash and import commits.

```
path_a                                 path_b                           co_commits
editor/editor_node.cpp                 editor/editor_node.h                    291
scene/gui/text_edit.cpp                scene/gui/text_edit.h                   211
scene/main/viewport.cpp                scene/main/viewport.h                   172
servers/visual/visual_server_raster.h  servers/visual_server.h                 150
scene/main/node.cpp                    scene/main/node.h                       142
```

### `hotspot`

Top-N functions by churn per bucket, `--lang <L>` required. Godot C++:

```
bucket  path                                       func                  added  deleted  churn
202007  modules/gdscript/gdscript_parser.cpp       annotation_exists         4    6,837  6,841
202211  thirdparty/vulkan/.../vulkan_funcs.hpp     VULKAN_HPP_INLINE     2,870    3,206  6,076
202011  core/variant_op.cpp                        evaluate              1,381    4,282  5,663
202109  thirdparty/glslang/.../glslang_tab.cpp     yyparse               2,885    2,256  5,141
202007  modules/gdscript/gdscript_parser.cpp       GDScriptParser        2,964    1,997  4,961
```

A hunk goes to the function that holds its first line in the file's
latest snapshot. Cheap, but old hunks drift: the 2020 GDScript parser
rewrite shows up as `annotation_exists`.

### `churn`

Added / deleted per group per bucket, `--by module | lang | author`
(`--depth N` path segments for `module`). Godot net lines per year, top
five modules by lifetime churn:

```
year        editor   thirdparty       doc   drivers     tools
2014             -            -    62,201   862,999   290,687
2016             -      102,579     9,925   -77,188   275,675
2018       178,217     -135,533    10,021    23,291         -
2020        43,731      652,248 1,422,245   -22,082         -
2022       886,526      -76,692    19,799     7,885         -
2024       101,104      162,478     6,897     4,019         -

net        484,457    1,263,738   198,016   925,095   573,505
churn    7,705,423    6,080,158 4,203,614 2,653,165 1,885,913
```

`tools/` ends in 2017 when the editor moved to `editor/`.

### `classify`

Conventional Commit types per bucket. ratatui per year:

```
year  commits  feats  fixes  reverts  breaking  untyped
2018       84     11      5        0         0       47
2020      148     25     23        0         0       37
2022       29      3      1        0         1        4
2023      445     76     63        1        19       20
2024      772    162    114        0        40       12
2025      473     52     35        1        11       12
```

`untyped` drops to ~2% once the project adopts Conventional Commits in
2023. `!` or a `BREAKING CHANGE:` footer marks breaking; git's own
`Revert "…"` counts as a revert.

### `age`

File-age histogram in 7-day bins, measured from today.

### Utilities

- `index` / `reindex`: build or wipe the cache.
- `export parquet|csv|json <dir>`: one file per table.
- `sql "<query>"`: raw DuckDB against the cache.

## Flags

Common to every query:

- `--from` / `--to YYYY-MM-DD`: inclusive UTC days.
- `--lang rust,python`: language filter.
- `--author SUBSTR`: canonical name or email substring.
- `--path PREFIX`: path prefix filter.
- `--format tsv|csv|json|table`: default `tsv` on a pipe, `table` on a TTY.
- `--bucket auto|day|week|month|tag|commit`: fixed at index time; re-key
  with `index --bucket X`.

`git-archaeologist <cmd> --help` lists the rest.

## Recipes

```sh
# plots in the terminal (youplot); output is long (one row per group),
# so pivot to one column per series first. The table is needed because
# PIVOT reads its input twice and stdin can only be read once.
git-archaeologist burndown --by language --lang rust,python,toml \
  | duckdb -c "CREATE TABLE t AS FROM read_csv('/dev/stdin');
               COPY (PIVOT t ON language USING sum(loc) ORDER BY bucket)
               TO '/dev/stdout' (DELIMITER '\t')" \
  | uplot lines -H --xlabel bucket --ylabel loc
git-archaeologist coupling --top 20 \
  | duckdb -c "COPY (SELECT path_a || ' + ' || path_b, co_commits
                     FROM read_csv('/dev/stdin'))
               TO '/dev/stdout' (DELIMITER '\t')" \
  | uplot barplot -H

# DuckDB over a Parquet export
git-archaeologist export parquet /tmp/repo/
duckdb -c "SELECT * FROM read_parquet('/tmp/repo/commits.parquet') LIMIT 5"

# raw SQL against the live cache: code per language at the latest sample
git-archaeologist sql "SELECT fs.language, SUM(fs.code)
                       FROM   file_stats fs JOIN commits c USING (sha)
                       WHERE  c.bucket_key = (SELECT MAX(bucket_key)
                                              FROM commits WHERE is_sampled)
                       GROUP  BY 1 ORDER BY 2 DESC"
```

A Claude Code skill in `skills/git-arch-analyze/` runs every subcommand
and writes a narrative report. Install it with
`ln -sf "$PWD/skills/git-arch-analyze" ~/.claude/skills/`, then run
`/git-arch-analyze [path]`.

## Reference

<details>
<summary><b>Test detection</b></summary>

Static, per code line, no toolchain or test run. A line is test code if:

1. **Path**: a `test(s)/`, `__tests__/`, `spec(s)/`, `benches/`,
   `testdata/`, `e2e/` directory, or the runner's file convention: Go
   `*_test.go`, pytest `test_*.py` / `*_test.py` / `conftest.py`, jest
   `*.test.*` / `*.spec.*`, JUnit `*Test.java` / `*IT.java`, RSpec
   `*_spec.rb`, Rust `tests.rs`, C/C++ `test_*` / `*_test` / `*_unittest`.
2. **Syntax** (tree-sitter region, attributes and decorators included):
   - Rust: items under `#[cfg(test)]` (predicate must imply `test`;
     `not(test)` / `any(test, x)` don't), `#![cfg(test)]` files, fns with
     `#[test]`, `#[<crate>::test]`, `#[bench]`, `#[rstest]`, `#[test_case]`.
   - Python: `def test*`, `class Test*`, `*TestCase` subclasses, pytest
     fixtures.
   - Go: `Test|Benchmark|Fuzz|Example` + non-lowercase (the `go test` rule).
   - JS/TS: top-level `describe/it/test(...)` with a string title (incl.
     `.each/.only/.skip`), lifecycle hooks with a function arg.
   - Java: classes with a `@Test`-family method, JUnit 3 `TestCase`.
   - Ruby: `def test_*`, `*Test` / `*TestCase` classes, RSpec blocks.
   - C++: GoogleTest / Catch2 / Boost.Test macros.
   - PHP: `*TestCase` subclasses.

`funcs.kind = 'test'` uses the same regions. Not detected: production
code that only tests call (needs a call graph).

</details>

<details>
<summary><b>Language support</b></summary>

Detection is by exact file name (`Makefile`, `Dockerfile`,
`CMakeLists.txt`), then extension. No content sniffing.

- **Tree-sitter, functions + test regions:** Rust, Python, JavaScript,
  TypeScript (`.ts` + `.tsx`), Go, C++, Java, Ruby, PHP; C (path-rule
  tests only).
- **Tree-sitter, best-effort functions:** Lua, Vim Script, Bash, OCaml,
  Scala, Haskell, Kotlin, C#, Swift.
- **Tree-sitter, lines only:** HTML, CSS.
- **No grammar, lines only** (comment prefixes + block delimiters):
  YAML, TOML, XML, INI, Dockerfile, Makefile, CMake, SQL, Protocol
  Buffers, Nix, HCL, Sass, LESS, Vue, Svelte, Perl, R, Julia, Elixir,
  Erlang, Dart, Zig, Objective-C, Objective-C++, Groovy, Gradle,
  PowerShell, GLSL, GDScript, PO File, Fish, Zsh, Nushell,
  ReStructuredText, Markdown, JSON.

Anything else (`.txt`, extensionless files) still counts in cohort,
survival and `stats` (as `other`), but not in `burndown --by language`.

</details>

<details>
<summary><b>Cache and config</b></summary>

One DuckDB file under XDG data, keyed by canonical worktree path so
checkouts don't collide:

```
~/.local/share/git-archaeologist/caches/<repo>-<hash>/cache.duckdb
```

`reindex` wipes it. A schema version change wipes it on next open.

Optional `~/.config/git-archaeologist/config.toml`:

```toml
default_bucket = "auto"   # commit | day | week | month | tag | auto
```

Optional `~/.config/git-archaeologist/aliases.toml` merges identity
variants; user aliases override `.mailmap`:

```toml
[[alias]]
canonical_name  = "Alice"
canonical_email = "alice@example.com"
raw = [
    { name = "Alice Smith", email = "alice@work.com" },
    { name = "alice",       email = "alice@personal.com" },
]
```

</details>

## Development

```sh
just --list         # recipes
just lint           # rustfmt, clippy -D warnings, dprint
just test           # unit + golden + scenarios
just e2e            # ratatui smoke vs git, blame, tokei (clones once)
just bench          # ratatui / uv / godot perf ceilings (SKIP_LARGE=1 skips godot)
just regen-goldens  # after an intentional output change
```

GitHub CI runs only fmt, clippy and `cargo test`. Godot's index time is
a known problem; see [docs/TASKS.md](docs/TASKS.md) §Perf plan.

Built on [`gix`](https://github.com/GitoxideLabs/gitoxide),
[`duckdb`](https://duckdb.org),
[`tree-sitter`](https://tree-sitter.github.io),
[`imara-diff`](https://github.com/pascalkuthe/imara-diff),
[`clap`](https://docs.rs/clap) and [`rayon`](https://docs.rs/rayon).

Prior art: [git-of-theseus](https://github.com/erikbern/git-of-theseus)
(cohort stacks and survival curves, Python) and
[hercules](https://github.com/src-d/hercules) (burndown, coupling
matrix).
