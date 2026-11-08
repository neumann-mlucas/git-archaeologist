# git-archaeologist — Specification (v1, rev 2)

## Purpose

Fast, single-binary Rust CLI to extract every interesting series out of a
git repository, dump it as parquet / TSV, and stay out of the plotting
business.

Companion tools do the drawing (`youplot`, `duckdb` CLI, `datasette`,
Marimo, whatever). We produce the numbers.

Answers questions like:

- "How much of the code written in 2020 is still alive today?"
- "What's the half-life of a line of code in this repo?"
- "Which files always change together?"
- "Which functions are churn hotspots?"
- "How has the language mix shifted per release tag?"
- "Who wrote what, respecting mailmap + `Co-authored-by:` trailers?"

## Prior art we deliberately learn from

- **git-of-theseus** — cohort stacks + survival curves. We steal both. Slow
  Python; we replace it with Rust + DuckDB.
- **hercules** — one-pass DAG, coupling matrix, per-author burndown. We
  steal the _analyses_; drop the Babelfish/UAST dep (dead) and the Python
  plotter split (`labours`). Single binary instead.

## Scope v1

### In scope

- Single repo, current working directory (or path arg).
- Read-only on the repository; never mutates `.git` or `.mailmap`.
- Full commit DAG (not first-parent linearization).
- Per-line cohort tracking via real diff hunks + rename detection.
- Metrics enumerated in §Metrics.
- Output = parquet dump + subcommand-per-metric emitting TSV/CSV/JSON on
  stdout for piping.
- Function-level metrics for the tree-sitter languages linked in
  (§Language stack: 21 grammars), line counts for 30+ more without one.

### Explicitly out of scope (v1)

- **No TUI.** Deleted from the prior spec. Analyzer + pipeable output is
  the entire surface.
- **No built-in plotting.** Users pipe to `youplot` / `duckdb` / anything.
  Zero matplotlib, plotters, or ratatui in the dependency tree.
- Multi-repo dashboards.
- Bare repos, detached HEAD — rejected on startup.
- Shallow clones — warn + degrade, do not fail.
- Cross-branch compare as a first-class mode.
- Windows — Linux + macOS only in v1.
- Complexity metrics (cyclomatic etc.) — post-v1.
- Sentiment / NLP on commit messages — never.

## Metrics

Every metric is a DuckDB query on the indexed schema (§Data model). New
metric = new query. No new Rust code per metric.

### Line-level (all languages, no tree-sitter needed)

1. **Burndown by language** — cumulative LOC per bucket, grouped by lang.
2. **Burndown by author** — same, grouped by canonical author.
3. **Cohort burndown** — cumulative surviving LOC per bucket, colored by
   _birth bucket_. Answers "how much 2020 code is still here?" Requires
   hunk-level attribution.
4. **Survival curve (Kaplan-Meier)** — % of lines from cohort N still
   alive at age T. Optional exponential fit → half-life scalar per repo
   (gated on ≥ 100 deletion events and ≥ 2 partly-dead cohorts, else
   NULL + `reason`). Age is in days, from each bucket's last commit
   time, so the half-life is in days whatever the bucket size.
5. **Coupling matrix** — top-N file pairs by co-occurrence in commits.
   Cheap self-join on `(sha, path)`. `--max-files-per-commit N` (default
   50) drops squash/import commits from the count.
6. **Commit classification over time** — from Conventional Commit prefix
   in message (`feat:` / `fix:` / `revert:` / `!`). Emits % bugfix,
   revert rate, breaking-change rate per bucket.
7. **File age histogram** — bucketed `now − first_touched_at` per path in
   scope.
8. **Churn per module** — sum of `+lines`/`-lines` per directory per
   bucket. Module = first `--depth N` path segments (default 1); paths
   with fewer segments keep their full path.

### Function-level (tree-sitter langs only — §Language stack)

Test detection is definition-based for Rust (`#[test]`, `#[cfg(test)]`),
Python (`def test_*`), Go (`func Test*`). JS/TS is best-effort call-site
match on `describe`/`it`/`test(...)` — pattern-recognized via
tree-sitter but less reliable than def-based. C/C++ is call-site match
on `TEST(...)` / `TEST_F(...)` (gtest) and `TEST_CASE(...)` (catch2);
no compiler-enforced convention exists, so accuracy tracks JS/TS.
Fallback: any file under `tests/`, `__tests__/`, `*.test.{js,ts,jsx,tsx}`,
`*.spec.*`, or matching `*_test.{c,cc,cpp,cxx}` / `test_*.{c,cc,cpp,cxx}`
counts as test.

9. **Function hotspot** — churn per top-level def per bucket. Sort desc,
   surface top N. A hunk is credited to the def (in the file's latest
   snapshot) whose line span holds the hunk's first new line: cheap,
   approximate for old hunks once lines shift.
10. **Function cohort** — birth bucket of each top-level def. Enables
    "% of functions from era X still present". Not a subcommand: `sql`
    over `funcs` + `commits`.
11. **Test-vs-code split** — attribute lines to `test` or `code` based on
    function/type attributes (`#[test]`, `def test_*`, `@Test`, etc.),
    not path heuristics.
12. **Comment density** — exact comment / code / blank ratio per bucket,
    grouped by lang. Exact because tree-sitter, not regex.

## Sampling (bucketing)

Snapshot line-level state at bucket boundaries only. Hunks + churn are
full-resolution — every non-merge commit.

Bucket sizes:

| Commit count | Default bucket |
| ------------ | -------------- |
| < 500        | commit         |
| 500 – 5,000  | day            |
| 5k – 50k     | week           |
| > 50k        | month          |

Overrides: `--bucket [auto|commit|day|week|month|tag]`, applied at index
time (`index --bucket X` re-keys the cache in place). On a query the flag
only applies when that query triggers the first index; otherwise the
query warns that the cache is bucketed differently.

Keys: commit = unix seconds, day = YYYYMMDD, week = ISO YYYYWW, month =
YYYYMM, tag = tag index.

`tag` is new: bucket boundary = each annotated/lightweight tag reachable
from HEAD, sorted by tagger date. Enables "burndown per release" without
external tooling.

Sampled commit within a time bucket = **last non-merge commit in the
bucket**.

## Git info consumed

Everything the prior spec ignored:

- **Full parent DAG** — `commit_parents` table with parent index.
- **Diff hunks** (via `gix::blob_diff`) — `(start, +len, -len)`.
  Required for cohort/survival. Not just numstat totals.
- **Rename detection** — on. Follows path chain so cohort birth date
  survives moves.
- **Committer identity** in addition to author.
- **`Co-authored-by:` / `Signed-off-by:` trailers** — parsed from commit
  message body, emit additional `(sha, author_id, role)` rows.
- **Commit message prefix** — Conventional Commit `type[!]:` extracted
  into `commits.msg_type` + `commits.is_breaking`.
- **Tags** — `refs/tags/*` walked, stored in `tags` table.
- **`.git-blame-ignore-revs`** — read at index time; listed commits get
  `commits.ignored_blame = true`. No report uses the flag yet: `stats`
  ownership still credits them.
- **Commit timezone offset** — not stored; `authored_at` is UTC unix
  seconds (committer time).
- **File mode + binary detection** — symlinks, submodules, binaries
  skipped in line counting.

## Filters

All filtering happens in SQL. There is no filter UI. The CLI exposes the
common ones as flags:

- `--from YYYY-MM-DD`, `--to YYYY-MM-DD`
- `--lang rust,python,...`
- `--author name-substring` (matches canonical name or email)
- `--path prefix/`

No `--exclude` in v1: `sql` covers exclusion
(`WHERE path NOT LIKE 'vendor/%'`). Module depth is `churn --depth N`.
Anything more exotic → `git-arch sql "SELECT ..."`.

`--from` / `--to` are inclusive UTC days on `commits.authored_at`.

## Author normalization

1. Load `.mailmap` at repo root via `gix::mailmap`.
2. Load user aliases from `$XDG_CONFIG_HOME/git-archaeologist/aliases.toml`.
3. Merge: user aliases override mailmap.
4. Parse `Co-authored-by:` / `Signed-off-by:` trailers; canonicalize
   through same mailmap + alias pipeline.
5. Store canonical identities in `authors`; every raw
   `(name, email)` observed is a row in `author_aliases`.

`aliases.toml` schema unchanged from prior spec.

## Language stack — 21 grammars, plus line counts without one

Grammars linked in statically (`src/index/treesitter.rs`, `LangRegistry`):

**Full** — comments, functions, test regions: Rust, Python, JavaScript,
TypeScript (`.ts` + `.tsx`), Go, C++, Java, Ruby, PHP. C (`.c` + `.h`) too,
but with path-rule tests only.

**Best-effort functions** (conventional node names, path-rule tests
only): Lua, Vim Script, Bash, OCaml, Scala, Haskell, Kotlin, C#, Swift.

**Lines only** (grammar, no functions): HTML, CSS. Markdown and JSON
had grammars that extracted nothing; they moved to `Parse::Plain` for
identical counts at ~15% less index time on uv.

**No grammar** — `Parse::Plain`: comments by line prefix and block
delimiters, double-quoted strings skipped; lines only. Labels follow
tokei: YAML, TOML, XML, INI, Dockerfile, Makefile, CMake, SQL, Protocol
Buffers, Nix, HCL, Sass, LESS, Vue, Svelte, Perl, R, Julia, Elixir,
Erlang, Dart, Zig, Objective-C, Objective-C++, Groovy, Gradle,
PowerShell, GLSL, GDScript, PO File, Fish, Zsh, Nushell,
ReStructuredText, Markdown, JSON (the last three: no comment syntax,
every non-blank line is code).

Non-negotiables:

- Detection: exact file name (`Makefile`, `Dockerfile`, `CMakeLists.txt`)
  then extension. No content sniffing.
- Anything else (`.txt`, extensionless files, unknown extensions) has no
  `file_stats` row: it counts in `line_births` and shows as `other` in
  `stats`, but not in `burndown --by language`.

Post-v1 additions still land as pure adds — each new grammar is a
Cargo line + a `LangSpec` entry, does not slow default build meaningfully
(binary +~1 MB per grammar; zero runtime cost unless the repo has
matching files).

## Output

Two shapes:

### 1. Bulk dump

```
git-arch export parquet out/     # one .parquet per table
git-arch export csv    out/      # one .csv per table
git-arch export json   out/      # one .json per table (arrays of objects)
```

### 2. Query subcommand — stdout, pipeable

```
git-arch burndown        --by language        --format tsv
git-arch burndown        --by author          --format tsv
git-arch cohort          --format tsv
git-arch survival        --format tsv
git-arch coupling  --top 50                   --format tsv
git-arch classify        --format tsv
git-arch hotspot   --top 30 --lang rust       --format tsv
git-arch age             --format tsv
git-arch churn     --by module --depth 2      --format tsv
```

Default `--format` is `tsv` when stdout is a pipe, `table` (pretty aligned
columns) when it's a terminal.

### Youplot-friendly examples

```
# LOC per language, stacked area (rendered by uplot)
git-arch burndown --by language --format tsv \
  | uplot line --xlabel date --ylabel loc

# Top 20 coupled file pairs, bar chart
git-arch coupling --top 20 --format tsv \
  | uplot barplot --title 'file coupling'

# Author burndown
git-arch burndown --by author --format tsv \
  | uplot line --xlabel date

# Half-life fit as one number, no plot
git-arch survival --fit exp --format tsv
```

### Raw SQL escape hatch

```
git-arch sql "SELECT lang, SUM(code) FROM file_stats
              WHERE bucket_key >= 20240101 GROUP BY lang"
```

## Data model — DuckDB

Cache path: `$XDG_DATA_HOME/git-archaeologist/caches/<repo>-<hash>/cache.duckdb`.

```sql
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- schema_version, indexed_head_sha, indexed_at, bucket_scheme

CREATE TABLE authors (
    id              INTEGER PRIMARY KEY,
    canonical_name  TEXT NOT NULL,
    canonical_email TEXT NOT NULL,
    UNIQUE(canonical_name, canonical_email)
);

CREATE TABLE author_aliases (
    author_id INTEGER NOT NULL REFERENCES authors(id),
    raw_name  TEXT NOT NULL,
    raw_email TEXT NOT NULL,
    PRIMARY KEY (raw_name, raw_email)
);

CREATE TABLE commits (
    sha           TEXT PRIMARY KEY,
    author_id     INTEGER NOT NULL REFERENCES authors(id),
    committer_id  INTEGER NOT NULL REFERENCES authors(id),
    authored_at   BIGINT  NOT NULL,   -- unix seconds, UTC
    is_merge      BOOLEAN NOT NULL,
    is_sampled    BOOLEAN NOT NULL,   -- line-level snapshot taken here
    bucket_key    BIGINT  NOT NULL,
    msg_type      TEXT,               -- 'feat'|'fix'|'refactor'|... (nullable)
    is_breaking   BOOLEAN NOT NULL,
    is_revert     BOOLEAN NOT NULL,
    ignored_blame BOOLEAN NOT NULL    -- from .git-blame-ignore-revs
);

CREATE TABLE commit_parents (
    sha        TEXT NOT NULL REFERENCES commits(sha),
    parent_sha TEXT NOT NULL,
    parent_idx INTEGER NOT NULL,      -- 0 = first-parent
    PRIMARY KEY (sha, parent_idx)
);

CREATE TABLE commit_trailers (
    sha       TEXT    NOT NULL REFERENCES commits(sha),
    author_id INTEGER NOT NULL REFERENCES authors(id),
    role      TEXT    NOT NULL        -- 'coauthor'|'signoff'
);

CREATE TABLE tags (
    name      TEXT PRIMARY KEY,
    sha       TEXT NOT NULL REFERENCES commits(sha),
    tagged_at BIGINT NOT NULL
);

-- Per-hunk churn. Required for cohort + survival.
CREATE TABLE hunks (
    sha        TEXT    NOT NULL REFERENCES commits(sha),
    path       TEXT    NOT NULL,      -- new-file path
    prev_path  TEXT,                  -- non-null on rename
    old_start  INTEGER NOT NULL,      -- 1-based line in old file
    old_len    INTEGER NOT NULL,      -- lines removed
    new_start  INTEGER NOT NULL,      -- 1-based line in new file
    new_len    INTEGER NOT NULL       -- lines added
);
CREATE INDEX idx_hunks_sha_path ON hunks(sha, path);
CREATE INDEX idx_hunks_path     ON hunks(path);

-- Per-file numstat rollup — cheap for coupling, churn-by-module, etc.
CREATE TABLE file_churn (
    sha     TEXT NOT NULL REFERENCES commits(sha),
    path    TEXT NOT NULL,
    added   INTEGER NOT NULL,
    deleted INTEGER NOT NULL,
    PRIMARY KEY (sha, path)
);

-- Sampled line-level snapshots. Only rows where commits.is_sampled = TRUE.
CREATE TABLE file_stats (
    sha       TEXT    NOT NULL REFERENCES commits(sha),
    path      TEXT    NOT NULL,
    language  TEXT    NOT NULL,
    code      INTEGER NOT NULL,
    comments  INTEGER NOT NULL,
    blanks    INTEGER NOT NULL,
    PRIMARY KEY (sha, path)
);
-- Test/code split is derived at query time from `funcs.kind = 'test'`
-- line-range sums. Per-file bool would misclassify mixed files
-- (e.g. Rust `#[cfg(test)] mod tests` inside `src/foo.rs`).

-- Function-level, tree-sitter langs only.
CREATE TABLE funcs (
    sha        TEXT NOT NULL REFERENCES commits(sha),
    path       TEXT NOT NULL,
    name       TEXT NOT NULL,
    kind       TEXT NOT NULL,         -- 'fn'|'method'|'test'|...
    start_line INTEGER NOT NULL,
    end_line   INTEGER NOT NULL,
    PRIMARY KEY (sha, path, name, start_line)
);
-- Function-cohort birth key = (path, name, start_line). Moving a function
-- within a file resets the birth. Documented, not fixed in v1 — the
-- alternative (key by name only) breaks on overloads + siblings.
```

Cohort tracking (Slice A of the roadmap) materializes a view:

```sql
CREATE VIEW line_births AS
  -- one row per (sha, path, line-in-current-file) with birth_sha + birth_bucket
  ...;
```

Materialized as a table on `--reindex` for query speed.

## Indexing pipeline

Two phases. Phase 1 is embarrassingly parallel; Phase 2 is sequential
per file (cohort fold) but parallel across files.

Both phases re-open `gix::Repository` per worker (gix `Repository` is
`!Sync`, but opening the same `.git/` from N threads is fine).

### Phase 1 — parallel across commits (rayon)

1. Open repo (`gix::discover`); reject bare + detached HEAD.
2. Load mailmap + user aliases + `.git-blame-ignore-revs`.
3. Walk tags, populate `tags`.
4. Walk full DAG (`rev-list --all`), populate `commits`, `commit_parents`,
   `commit_trailers`.
5. For every non-merge commit in parallel:
   - Compute hunk-level diff vs first parent (`gix::blob_diff` with
     rename detection on) → `hunks` (all four line ranges), `file_churn`.
   - If sampled (last-in-bucket) → tree-sitter LOC + function extraction
     over the in-mem tree → `file_stats`, `funcs`.

### Phase 2 — sequential per file, parallel across files (rayon)

Cohort tracking cannot be parallelized across commits: it requires a
stateful fold on each file's line-array in commit order. It IS parallel
across files.

6. Group `hunks` by final (post-rename-follow) path.
7. For each path, in parallel (rayon):
   - Walk the file's hunks in commit order along the first-parent chain.
     Each merge on the chain folds as its diff against its first parent
     (not stored in `hunks`), so branch lines are born at the merge that
     landed them and line counts match HEAD exactly.
   - Maintain a `Vec<sha>` — birth commit per line. Apply each hunk:
     splice out `old_len` entries at `old_start`, splice in `new_len`
     copies of the current commit's sha at `new_start`.
   - Emit final `(path, line_no, birth_sha, birth_bucket)` rows into
     `line_births`.
   - Write lines added per (sha, path) to `chain_births`: `survival`'s
     `born`, so survivors and births count the same commits.
8. Update `meta.indexed_head_sha`, `meta.indexed_at`.

Progress reported via stderr every 100 ms (phase + percentage + eta).

Incremental reindex: skip SHAs already in `commits`, and purge stored
commits no ref reaches anymore (rebase, amend, deleted branch). Tail bucket may
promote a newly-added commit to sampled — re-run tree-sitter for that
single bucket only. The cohort fold is rebuilt in full on every run
(incremental fold: TASKS, deferred).

## CLI surface

```
git-archaeologist [--repo PATH] <SUBCOMMAND>

SUBCOMMANDS
    index                 build / update the cache (implicit on any query)
    reindex               wipe cache + rebuild
    export <fmt> <dir>    dump every table as parquet | csv | json
    sql "<query>"         raw DuckDB query, stdout tsv/table
    burndown              cumulative LOC series
    cohort                cohort-stacked LOC series
    survival              Kaplan-Meier + optional exp fit
    coupling              top-N file pairs by co-commit
    classify              conventional-commit types over time
    hotspot               top-N funcs by churn (tree-sitter langs only)
    age                   file age histogram
    churn                 churn per module / lang / author over time
    stats                 tokei-style HEAD snapshot (or --rev C | A..B)

COMMON FLAGS
    --from DATE  --to DATE
    --bucket [auto|commit|day|week|month|tag]
    --lang L,L
    --author SUBSTR
    --path PREFIX
    --format [tsv|csv|json|table]
    --by [language|author|module|kind]   (burndown, churn)

SUBCOMMAND-SPECIFIC
    coupling  --max-files-per-commit N   (default 50)
    survival  --fit exp                  (min 100 deletion events)
    hotspot   --top N   --lang L         (--lang required)
    churn     --depth N                  (default 1)
    stats     --by lang|dir|file --depth N [--rev C | --rev A..B]
```

## Perf targets

- First run on 10k-commit repo: < 90 s indexer (rayon-parallel).
- Cached-run query (any subcommand, default filters): < 500 ms.
- Cache size: < 200 MB for 10k-commit polyglot repo (hunks table is the
  hot cost).
- `sql` subcommand warm-start: < 200 ms.

Perf is a v1 requirement, not a v1.1 aspiration. If we're not faster
than theseus + hercules on the same repo, the project has no reason to
exist.

## Test strategy

Four tiers, ordered by cost. CI runs the first two (plus fmt +
clippy) on every push. Everything runs locally through the `justfile`:
`just test` (tiers 0-1.x), `just e2e` (tier 2), `just bench` (tier 3),
`just lint`.

Independent oracles, in `tests/common/truth.rs`: line births per file
against `git`, birth commits against `git blame --first-parent`, code +
blanks per language against `tokei`.

### Tier 0 — unit (fast, `cargo test`)

Per-module. Sub-second.

- `bucket::bucket_key` — day / week (ISO year-wrap) / month / commit / tag.
- `treesitter::count_lines` — one fixture per grammar (Rust, Py, JS, TS, Go, C, C++).
- `treesitter::extract_funcs` — same fixtures, function boundaries.
- `query::apply_view` — cohort dense-fill, cumulative running-sum.
- `mailmap` + trailer parsing — `Co-authored-by:`, `Signed-off-by:`,
  quoted names, unicode.
- Conventional Commit parser — `feat!:`, `fix(scope):`, `revert:`,
  malformed input.
- `.git-blame-ignore-revs` loader — comments, blank lines, missing file.

### Tier 1 — golden repo integration (fast, `cargo test`)

A hand-built tiny repo, built at test time via `tempfile` + system `git`
with pinned identities and dates (stable SHAs).

- 3-author, 7-language, 30-commit fixture with known truths:
  - Includes a rename, a merge, a revert, a `Co-authored-by:` trailer,
    a Conventional Commit `feat!:`, a tag, and a `.git-blame-ignore-revs`
    entry.
- Every subcommand and every filter combination exits 0 with output.
- `export parquet`, then each of the 13 tables must equal its checked-in
  golden in `tests/goldens/` as a multiset (DuckDB `EXCEPT ALL` both
  ways: row order ignored, duplicates count). Drift prints the differing
  rows. `just regen-goldens` rewrites them.

### Tier 1.x — scenarios (fast, `cargo test`)

`tests/scenarios.rs`: one small synthetic repo per invariant, exact
assertions. Covers query outputs (survival, fit, coupling cap, hotspot,
churn depth, burndown by kind, classify edge cases), bucket keys per
size, date filters, mailmap + trailers, `.git-blame-ignore-revs`, odd
files (CRLF, no final newline, empty, unicode paths, case-only rename,
gitlink, file↔dir), unusual history (evil / octopus / criss-cross /
unrelated-root merges, rename on a branch, same-second commits, clock
skew, rewritten history), cache corruption and locking, and bare /
detached / shallow repos.

### Tier 2 — small public repo smoke (opt-in, `cargo test --features e2e`)

Cloned once into `$XDG_CACHE_HOME/git-archaeologist-tests/` and reused.
Correctness bounds are strict but not bit-exact (upstream evolves).

- Fixture: `ratatui-org/ratatui` (small, active, polyglot-ish — Rust
  primarily, ~5k commits, ~300 contributors, actively adds features).
- Assertions:
  - `commits` row count ≥ `git rev-list --count HEAD` (all refs walked).
  - Line births per file equal `git`; ≥ 95% of sampled lines match
    `git blame --first-parent`.
  - Code + blanks for Rust / Python / JSON within 0.5% of `tokei`.
  - Every subcommand exits 0 with non-empty stdout under default filters.
  - Coupling top-1 pair has > 1 co-commit.
  - Surviving lines within 50% of sampled code lines (loose: one counts
    every line, the other code only).
- Perf ceiling: full `index` < 180 s, any query < 5 s (loose: shared
  runners; the §Perf targets are measured by tier 3).

### Tier 3 — mid & mid-large bench (local, `just bench`)

`cargo test --release --features bench-large --test tier3_bench`. Not in
CI. Perf ceilings per class (index 180 s / 600 s / 1800 s, queries
5 s / 10 s / 30 s), RSS < 2 GB. The table below is the target, not the
gate.

| Class     | Fixture repo          | Commits | Perf ceiling (index) | Perf ceiling (query) |
| --------- | --------------------- | ------- | -------------------- | -------------------- |
| small     | `ratatui-org/ratatui` | ~5k     | 30 s                 | 500 ms               |
| mid       | `astral-sh/uv`        | ~15k    | 90 s                 | 500 ms               |
| mid-large | `godotengine/godot`   | ~60k    | 10 min               | 2 s                  |

- Correctness bounds:
  - Small: LOC total within ±1% of `tokei`, commit count exact.
  - Mid: LOC total within ±1%, per-language share within ±0.5 pp.
  - Mid-large: LOC total within ±2%, per-language share within ±1 pp.
    Correctness is secondary; perf is the gate.
- Fixture repos pinned by commit SHA in `benches/fixtures.toml` so
  results are reproducible across runs.
- Memory ceiling: RSS < 2 GB on mid-large during index.
- Cache size ceiling: matches §Perf targets.

### What we do NOT test

- Cross-branch queries — out of scope, `sql` escape hatch covers it.
- Rename tracking correctness beyond one hop — v1 accepts drift.
- Shallow-clone results — warned + degraded, not asserted.
- Windows anything.

## Non-goals / explicit limits (v1)

- No TUI. Users pipe to `youplot` / `duckdb` / `datasette` / anything.
- No plot rendering — no `plotters`, no matplotlib subprocess.
- No blame lens as a first-class metric. Cohort tracking supersedes
  last-touch attribution; per-line blame walk is deferred until a real
  use case shows up.
- No Windows.
- No branch other than HEAD walked for sampling (though the full DAG is
  stored — cross-branch queries via `sql` still work).
- No content-based language detection: extension, plus an exact
  filename map for `Makefile`, `Dockerfile`, `CMakeLists.txt`.

## Config

`$XDG_CONFIG_HOME/git-archaeologist/config.toml`:

```toml
default_bucket = "auto"
```

Deliberately tiny. `--exclude` and `--module-depth` were considered and
cut (§Filters). Anything beyond `default_bucket` = `git-arch sql`.

`aliases.toml` — as above.

## Dependency budget

Justify every crate on the fence.

| Crate                       | Why                                                                 | Alternative rejected                                                      |
| --------------------------- | ------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| `gix`                       | pure-Rust git, no subprocess, gives hunks + rename via `blob_diff`  | shell out to `git` — slower per-commit                                    |
| `duckdb` (unbundled)        | columnar OLAP + parquet native + window fns; new metric = new query | `sqlite` (row-store, slow on aggregations); polars (no persistence story) |
| `tree-sitter` + 21 grammars | required for function-level + exact comment split                   | regex (approximate, no function-level)                                    |
| `rayon`                     | per-commit parallelism                                              | manual threading                                                          |
| `clap`                      | CLI                                                                 | —                                                                         |
| `plotters`                  | —                                                                   | **rejected** — no plotting in v1                                          |
| `ratatui`, `crossterm`      | —                                                                   | **rejected** — no TUI in v1                                               |
| `directories`               | XDG paths                                                           | —                                                                         |
| `time`                      | timestamps + tz                                                     | chrono                                                                    |

**Removed vs prior spec:** ratatui, crossterm, plotters (never added,
now formally out). Tree-sitter grammar set later re-expanded to 20
(§Language stack) after the Core-9 landed cleanly and the +1 MB
per-grammar cost stayed cheap; §Killed entry retired.

**Distribution: prebuilt binaries via `cargo dist` are the primary
install path.** Users download a static-linked binary from GitHub
releases; DuckDB is linked in at build time on our CI runners so end
users need nothing installed. `cargo install git-archaeologist` remains
supported as a fallback and requires `libduckdb` on the system (via
apt/brew/dnf; Arch users hit AUR). Bundled compile-from-source is
dropped — first build no longer takes 5-15 minutes for either path.

## Roadmap post-v1

- More grammars only where functions matter (Zig, Dart, Elixir,
  Objective-C); they already get line counts without one.
- v1.1: `git-arch serve` — unix socket daemon; editor extensions.
- v1.2: full incremental blame (RB-tree, hercules-style) IF a real user
  asks for line-level ownership. Kept out of v1 because cohort covers
  90% of the ownership questions cheaper.
- v1.2: WASM build for a browser demo.
- v2: multi-repo (`git-arch export` outputs merge cleanly on
  `authors`/`bucket_key`; add a merger CLI verb).

## Killed (do not resurrect without a fresh case)

- Any TUI (breakdown table, chart panel, modals, palette).
- `Lens` / `View` / `GroupBy` enums in code — replaced by CLI
  subcommands + `--by` flag.
- Ownership lens as sha × path blame subprocess.
- Delta view as a first-class mode (compute in SQL).
- Language detection heuristics beyond ext + exact filename map.
- Sentiment / DTW / any ML.
- Babelfish / UAST.
- `--exclude` flag / `default_exclude` config — use `sql WHERE`.
- `--module-depth` flag / `default_module_depth` config — module =
  first path segment, no knob.
- `commits.tz_offset_min` column — no v1 metric consumes it.
- `file_stats.is_test` column — misclassifies mixed files; derive from
  `funcs.kind` at query time.
