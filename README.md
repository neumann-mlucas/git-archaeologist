# git-archaeologist

Rust CLI that computes derived time series from a git repository — LOC by
language / author / module, cohort survival, file coupling, function-level
churn, commit classification, file age — and emits them as
TSV / CSV / JSON / Parquet. One DuckDB cache per repo; every query returns
in under a second after the initial index.

Backed by `gix` (pure-Rust git), `duckdb` (columnar cache), and
`tree-sitter` (21 grammars, plus line counts for 30+ languages without
one). No plotting, no UI, no daemon.

Status: **v1 in development.** See [SPEC.md](SPEC.md) and [TASKS.md](TASKS.md).

## Metrics

Samples below are real output from the three fixtures the test harness
pins (`benches/fixtures.toml`), indexed with the current build on an
8-core laptop. Every ref is walked, so commit counts exceed `git
rev-list --count HEAD`.

| fixture                                           | commits (non-merge) | authors | span              | bucket | index  | cache  |
| ------------------------------------------------- | ------------------- | ------- | ----------------- | ------ | ------ | ------ |
| [ratatui](https://github.com/ratatui/ratatui)     | 2,589 (2,574)       | 367     | 2016-10 → 2026-07 | day    | 38 s   | 149 MB |
| [uv](https://github.com/astral-sh/uv)             | 13,164 (13,111)     | 725     | 2023-10 → 2026-08 | week   | 213 s  | 175 MB |
| [godot](https://github.com/godotengine/godot) 4.3 | 66,099 (36,084)     | 3,258   | 2013-04 → 2024-08 | month  | 69 min | 1.0 GB |

Godot's index time is a known problem; see TASKS §Perf plan. Most
samples use godot, rolled up to years for print.

### `burndown`

Cumulative LOC per bucket. `--by language | author | module | kind`.
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

C++ overtook C in 2022. Translation catalogs (`PO File`) ballooned to
1.3M lines in 2020 and have been shrinking since. C# arrives with the
Mono module in 2018.

`--by kind` splits test code from the rest; at 4.3, godot is 3.16M lines
of code and 52k lines of test code.

### `survival`

Kaplan-Meier survival per birth cohort. `--fit exp` fits
`ln(survival) = −λ·age` and returns a half-life in days, with ages taken
from each bucket's last commit time (so any `--bucket` gives days).
Lines are tracked along HEAD's first-parent chain, so counts match HEAD
exactly, and lines from a merged branch are born at the merge that landed
them: `survival`, `cohort` and `stats` age/owner credit that commit, not
the branch commit.

```
metric      value   reason
half_life   915.241 ok (n=124, deaths=11293117)
```

Godot cohorts rolled up to years:

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

Code from the 2014 open-source release is 87% gone; the Godot 4
rewrite years (2020, 2022) churned millions of lines that did not last.

### `cohort`

Cohort-stacked surviving LOC per bucket (`bucket, cohort, alive_lines`):
the per-bucket series behind a Theseus-style stream plot. `survival
--fit exp` is the scalar that summarizes its decay.

### `coupling`

Top-N file pairs by co-occurrence in commits. `--max-files-per-commit N`
(default 50) drops squash / import commits. On godot the top ten are all
`.cpp` / `.h` pairs plus the visual-server interface:

```
path_a                                 path_b                           co_commits
editor/editor_node.cpp                 editor/editor_node.h                    291
scene/gui/text_edit.cpp                scene/gui/text_edit.h                   211
scene/main/viewport.cpp                scene/main/viewport.h                   172
servers/visual/visual_server_raster.h  servers/visual_server.h                 150
scene/main/node.cpp                    scene/main/node.h                       142
```

### `hotspot`

Top-N functions by churn per bucket. `--lang <L>` required. Columns:
`bucket, path, func, kind, language, added, deleted, churn`. `--lang c++`
on godot:

```
bucket  path                                         func                  added  deleted  churn
202007  modules/gdscript/gdscript_parser.cpp         annotation_exists         4    6,837  6,841
202211  thirdparty/vulkan/include/vulkan/vulkan_funcs.hpp  VULKAN_HPP_INLINE  2,870  3,206  6,076
202011  core/variant_op.cpp                          evaluate              1,381    4,282  5,663
202109  thirdparty/glslang/.../glslang_tab.cpp       yyparse               2,885    2,256  5,141
202007  modules/gdscript/gdscript_parser.cpp         GDScriptParser        2,964    1,997  4,961
```

A hunk is credited to the function whose span, in the file's latest
snapshot, holds the hunk's first line. Cheap, but old hunks drift: a
pure deletion lands on whichever function now sits at that line (the
GDScript parser rewrite of 2020-07 shows up as `annotation_exists`).

### `stats`

tokei-style snapshot of HEAD (parsed live, not the working tree), grouped
`--by lang|dir|file` (default `lang`, `--depth N` for `dir`), with a
`Total` row. Beyond line counts: test code (`test_code`, `test_pct` of
code lines — see [Test detection](#test-detection)), function count +
longest function, median age of surviving lines, top owner by surviving
lines, 90-day churn (relative to today). Files with no language show as
`other` (lines only). Filters: `--lang`, `--path`. uv, top rows:

```
Lang      Files   Lines    Code  Comments  Blanks  Test Code  Test Pct  Funcs  Max Func  Median Age Days  Owner           Owner Pct
Rust        485  238858  187012     25783   26063      96211      51.4   5557      1162              829  Charlie Marsh        50.6
other       177   52711                                                     0                        816  Andrew Gallant       35.1
Markdown     75   24709   16492         0    8217          0         0      0                        821  Zanie Blue           49.1
JSON          4   15845   15845         0       0          0         0      0                        765  Zanie Blue           67.6
TOML         98    5919    4924       499     496          0         0      0                        816  Andrew Gallant       42.8
Python       66    5830    4407       480     943        352         8    181       214              838  Charlie Marsh        50.5
YAML         11    3469    2827       202     440          0         0      0                        921  Charlie Marsh        37.1
Total       931  348130  232025     27127   36267      96563      41.6   5740      1162              822  Charlie Marsh        43.1
```

Half of uv's Rust is tests. `--rev <commit>` snapshots another commit;
`--rev A..B` prints the per-group delta B − A (empty side = `HEAD`,
groups with no change dropped, `test_pct` in percentage points). Both
are parse-only: no `median_age_days`/`owner`/`churn_90d`, grammar-less
files omitted, `lines` = code + comments + blanks.

#### Test detection

Static, per code line, no toolchain or test run. A line is test code if:

1. **Path** — a `test(s)/`, `__tests__/`, `spec(s)/`, `benches/`,
   `testdata/`, `e2e/` directory, or the runner's own file convention:
   Go `*_test.go`, pytest `test_*.py` / `*_test.py` / `conftest.py`, jest
   `*.test.*` / `*.spec.*`, JUnit `*Test.java` / `*IT.java`, RSpec
   `*_spec.rb`, Rust `tests.rs`, C/C++ `test_*` / `*_test` / `*_unittest`.
2. **Syntax** (tree-sitter region, attributes/decorators included):
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

### `age`

File-age histogram, 7-day bins, measured from today (not from HEAD).
Godot's pinned 4.3 is two years old, so nothing is younger than 812
days: 4,644 files are 2–5 years old, 6,378 older.

### `churn`

Added / deleted per group per bucket. `--by module | lang | author`.
`--by module --depth N` (default `1`) groups by the first N path
segments; files with fewer segments keep their full path. Godot, net
lines (added − deleted) per year, top five modules by lifetime churn:

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

`tools/` ends in 2017 when the editor moved to `editor/`; `doc/`'s 2020
spike is the class-reference translation import.

### `classify`

Conventional Commit type counts per bucket: `commits, fixes, feats,
reverts, breaking, untyped`. Types are case-insensitive, `!` or a
`BREAKING CHANGE:` footer marks breaking, git's own `Revert "…"` counts
as a revert. ratatui per year:

```
year  commits  feats  fixes  reverts  breaking  untyped
2018       84     11      5        0         0       47
2020      148     25     23        0         0       37
2022       29      3      1        0         1        4
2023      445     76     63        1        19       20
2024      772    162    114        0        40       12
2025      473     52     35        1        11       12
```

The project adopted Conventional Commits around its 2023 revival:
`untyped` drops from most commits to ~2%.

### Utility subcommands

- `index` / `reindex` — build or wipe the cache.
- `export parquet|csv|json <dir>` — one file per table.
- `sql "<query>"` — raw DuckDB against the cache.

## Install

Build from source until `cargo dist` prebuilts land:

```sh
cargo install --path .
```

`duckdb` links against system `libduckdb`:

- Arch: `pacman -S duckdb`
- Debian / Ubuntu: `apt install libduckdb-dev`
- macOS: `brew install duckdb`

Binary is `git-archaeologist`. Alias to `git-arch` if you prefer.

## Quick start

```sh
cd repo
git-archaeologist index
git-archaeologist burndown --by language | uplot line
git-archaeologist survival --fit exp
git-archaeologist coupling --top 20
```

`index` runs implicitly on the first query.

## Flags

Common to every query subcommand:

- `--from YYYY-MM-DD` / `--to YYYY-MM-DD` — inclusive UTC days.
- `--lang rust,python` — comma-list language filter.
- `--author SUBSTR` — canonical-name / email substring.
- `--path PREFIX` — path prefix filter.
- `--bucket auto|day|week|month|tag|commit` — bucketing, fixed at index
  time. On a query it only applies if that query builds the cache;
  otherwise it warns. Re-key with `index --bucket X`. Keys: commit =
  unix seconds, day = YYYYMMDD, week = ISO YYYYWW, month = YYYYMM.
- `--format tsv|csv|json|table` — default: `tsv` on pipe, `table` on TTY.
- `--by language|author|module` — group axis for `burndown` / `churn`
  (`burndown` also takes `kind`: test vs non-test code).

Subcommand-specific:

- `coupling --max-files-per-commit N` (default 50).
- `survival --fit exp` — half-life in days; NULL + `reason` when there
  are < 100 deletion events or < 2 partly-dead cohorts.
- `hotspot --top N --lang L` — `--lang` required.
- `stats --by lang|dir|file --depth N [--rev C | --rev A..B]` — commit snapshot or delta; only `--lang`/`--path` filters.
- `churn --depth N` (default 1) — path-segment granularity for
  `--by module`.

## Recipes

### youplot

```sh
git-archaeologist burndown --by language | uplot line --xlabel date --ylabel loc
git-archaeologist coupling --top 20      | uplot barplot
```

### DuckDB CLI

```sh
git-archaeologist export parquet /tmp/repo/
duckdb -c "SELECT * FROM read_parquet('/tmp/repo/commits.parquet') LIMIT 5"
```

For current-snapshot queries prefer the built-in `sql` subcommand — it
joins through `commits.bucket_key` correctly against the live cache
without an export step.

### Raw SQL

Code per language at the latest sampled commit:

```sh
git-archaeologist sql "SELECT fs.language, SUM(fs.code)
                       FROM   file_stats fs
                       JOIN   commits c USING (sha)
                       WHERE  c.bucket_key = (SELECT MAX(bucket_key)
                                              FROM commits WHERE is_sampled)
                       GROUP  BY 1
                       ORDER  BY 2 DESC"
```

## Claude Code skill

`skills/git-arch-analyze/` indexes a repo, runs every subcommand, and
writes `ARCHAEOLOGY.md` with the metrics plus a narrative
interpretation.

```sh
mkdir -p ~/.claude/skills
ln -sf "$PWD/skills/git-arch-analyze" ~/.claude/skills/git-arch-analyze
```

Invoke from any repo: `/git-arch-analyze [path]`.

## Cache

DuckDB file under XDG data, keyed by canonical worktree path so multiple
checkouts don't collide:

```
~/.local/share/git-archaeologist/caches/<repo>-<hash>/cache.duckdb
```

`reindex` wipes it. Schema-version drift auto-wipes on next open (one
line to stderr).

## Config

Optional `~/.config/git-archaeologist/config.toml`:

```toml
default_bucket = "auto"   # commit | day | week | month | tag | auto
```

Optional `~/.config/git-archaeologist/aliases.toml` — merges identity
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

## Language support

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

## Development

```sh
just --list         # recipes
just lint           # rustfmt, clippy -D warnings, dprint
just test           # unit + golden + scenarios
just e2e            # ratatui smoke vs git, blame, tokei (clones once)
just bench          # ratatui / uv / godot perf ceilings (SKIP_LARGE=1 skips godot)
just regen-goldens  # after an intentional output change
```

GitHub CI runs only fmt, clippy and `cargo test`.

## Stack

- [`gix`](https://github.com/GitoxideLabs/gitoxide) — pure-Rust git, `blob_diff` for hunks + rename detection.
- [`duckdb`](https://duckdb.org) — columnar OLAP cache, native parquet.
- [`tree-sitter`](https://tree-sitter.github.io) — per-language LOC + function extraction.
- [`imara-diff`](https://github.com/pascalkuthe/imara-diff) — line-hunk producer over blob pairs.
- [`clap`](https://docs.rs/clap) — CLI.
- [`rayon`](https://docs.rs/rayon) — parallel churn walk, cohort fold, tree-sitter snapshot.

## Prior art

- [**git-of-theseus**](https://github.com/erikbern/git-of-theseus) — cohort stacks + survival curves in Python; slower, single-purpose.
- [**hercules**](https://github.com/src-d/hercules) — one-pass DAG, coupling matrix, per-author burndown; larger dep footprint.
