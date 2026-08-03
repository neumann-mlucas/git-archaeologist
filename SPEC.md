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
  steal the *analyses*; drop the Babelfish/UAST dep (dead) and the Python
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
- Function-level metrics for the 7 languages with tree-sitter grammars
  linked in.

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
   *birth bucket*. Answers "how much 2020 code is still here?" Requires
   hunk-level attribution.
4. **Survival curve (Kaplan-Meier)** — % of lines from cohort N still
   alive at age T. Optional exponential fit → half-life scalar per repo
   (gated on ≥ 100 deletion events, else NULL + `reason`).
5. **Coupling matrix** — top-N file pairs by co-occurrence in commits.
   Cheap self-join on `(sha, path)`. `--max-files-per-commit N` (default
   50) drops squash/import commits from the count.
6. **Commit classification over time** — from Conventional Commit prefix
   in message (`feat:` / `fix:` / `revert:` / `!`). Emits % bugfix,
   revert rate, breaking-change rate per bucket.
7. **File age histogram** — bucketed `now − first_touched_at` per path in
   scope.
8. **Churn per module** — sum of `+lines`/`-lines` per top-level
   directory per bucket. Module = first path segment; no configurable
   depth in v1.

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
   surface top N.
10. **Function cohort** — birth bucket of each top-level def. Enables
    "% of functions from era X still present".
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
|--------------|----------------|
| < 500        | commit         |
| 500 – 5,000  | day            |
| 5k – 50k     | week           |
| > 50k        | month          |

Overrides: `--bucket [auto|commit|day|week|month|tag]`.

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
- **`.git-blame-ignore-revs`** — read at index time; commits listed are
  excluded from ownership attribution.
- **Commit timezone offset** — stored separately from unix timestamp so
  per-author-local hour-of-day is queryable.
- **File mode + binary detection** — symlinks, submodules, binaries
  skipped in line counting.

## Filters

All filtering happens in SQL. There is no filter UI. The CLI exposes the
common ones as flags:

- `--from YYYY-MM-DD`, `--to YYYY-MM-DD`
- `--lang rust,python,...`
- `--author name-substring` (matches canonical name or email)
- `--path prefix/`

No `--exclude` and no `--module-depth` in v1. Both add configuration
surface for the smallest fraction of users; `sql` covers exclusion
(`WHERE path NOT LIKE 'vendor/%'`), and module = first path segment is
the fixed rule. Anything more exotic → `git-arch sql "SELECT ..."`.

## Author normalization

1. Load `.mailmap` at repo root via `gix::mailmap`.
2. Load user aliases from `$XDG_CONFIG_HOME/git-archaeologist/aliases.toml`.
3. Merge: user aliases override mailmap.
4. Parse `Co-authored-by:` / `Signed-off-by:` trailers; canonicalize
   through same mailmap + alias pipeline.
5. Store canonical identities in `authors`; every raw
   `(name, email)` observed is a row in `author_aliases`.

`aliases.toml` schema unchanged from prior spec.

## Language stack — tree-sitter, 20 grammars

V1 grammars linked in statically:

**Core 9** — full function-level extraction:

- Rust
- Python
- JavaScript
- TypeScript (covers `.ts` + `.tsx`)
- Go
- C (covers `.c` + `.h`)
- C++
- Lua
- Vim Script

**Extended 11** — full comment / LOC extraction; function extraction is
best-effort using conventional node names:

- Java, Ruby, Bash, HTML, CSS, JSON, PHP, OCaml, Scala, Haskell,
  Markdown

Non-negotiables:
- Detection: extension → language. No content sniffing.
- Files whose extension isn't in the registry fall back to an
  extension-map heuristic (single-line comment prefix + block delimiters
  by ext) so burndown-by-language keeps working.

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
7. For each path, in parallel:
   - Walk the file's hunks in commit order along the first-parent chain.
   - Maintain a `Vec<sha>` — birth commit per line. Apply each hunk:
     splice out `old_len` entries at `old_start`, splice in `new_len`
     copies of the current commit's sha at `new_start`.
   - Emit final `(path, line_no, birth_sha, birth_bucket)` rows into
     `line_births`.
8. Update `meta.indexed_head_sha`, `meta.indexed_at`.

Progress reported via stderr every 100 ms (phase + percentage + eta).

Incremental reindex: skip SHAs already in `commits`. Tail bucket may
promote a newly-added commit to sampled — re-run tree-sitter for that
single bucket only. Cohort fold reruns only for files whose hunk set
changed (new commits touched them).

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

COMMON FLAGS
    --from DATE  --to DATE
    --bucket [auto|commit|day|week|month|tag]
    --lang L,L
    --author SUBSTR
    --path PREFIX
    --format [tsv|csv|json|table]
    --by [language|author|module|cohort]

SUBCOMMAND-SPECIFIC
    coupling  --max-files-per-commit N   (default 50)
    survival  --fit exp                  (min 100 deletion events)
    hotspot   --top N   --lang L
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

Four tiers, ordered by cost. CI runs the first two on every push; the
lower two are opt-in / weekly.

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

A hand-built tiny repo lives at `tests/data/golden/` (or is built at test
time via `tempfile` + system `git`). Bit-exact assertions on every
metric.

- 3-author, 7-language, 30-commit fixture with known truths:
  - Includes a rename, a merge, a revert, a `Co-authored-by:` trailer,
    a Conventional Commit `feat!:`, a tag, and a `.git-blame-ignore-revs`
    entry.
- For every subcommand (`burndown`, `cohort`, `survival`, `coupling`,
  `classify`, `hotspot`, `age`, `churn`, `sql`) — snapshot of stdout
  compared byte-for-byte against a checked-in golden `.tsv`.
- End-to-end: `git-arch index && git-arch export parquet /tmp/x` →
  DuckDB reopens the parquet → row counts match.

### Tier 2 — small public repo smoke (opt-in, `cargo test --features e2e`)

Cloned once into `$XDG_CACHE_HOME/git-archaeologist-tests/` and reused.
Correctness bounds are strict but not bit-exact (upstream evolves).

- Fixture: `ratatui-org/ratatui` (small, active, polyglot-ish — Rust
  primarily, ~5k commits, ~300 contributors, actively adds features).
- Assertions:
  - `git rev-list --count HEAD` == `commits` table row count.
  - Total code lines within ±1% of `tokei --output json .`.
  - Every subcommand exits 0 with non-empty stdout under default filters.
  - Cohort surviving sum at latest bucket == current total code lines
    (within ±0.5%).
  - Coupling top-1 pair matches `git log --name-only` co-occurrence
    hand-check.
- Perf ceiling: full `index` < 30 s, any query < 500 ms.

### Tier 3 — mid & mid-large bench (manual / nightly, `cargo bench --features bench-large`)

Not run in default CI. Runs on a dev box or a nightly GitHub Action with
a beefier runner. Both correctness (looser bounds) and perf (hard
ceilings).

| Class      | Fixture repo         | Commits | Perf ceiling (index) | Perf ceiling (query) |
|------------|----------------------|---------|----------------------|----------------------|
| small      | `ratatui-org/ratatui`| ~5k     | 30 s                 | 500 ms               |
| mid        | `astral-sh/uv`       | ~15k    | 90 s                 | 500 ms               |
| mid-large  | `godotengine/godot`  | ~60k    | 10 min               | 2 s                  |

- Correctness bounds:
  - Small: LOC total within ±1% of `tokei`, commit count exact.
  - Mid: LOC total within ±1%, per-language share within ±0.5 pp.
  - Mid-large: LOC total within ±2%, per-language share within ±1 pp.
    Correctness is secondary; perf is the gate.
- Fixture repos pinned by commit SHA in `benches/fixtures.toml` so
  results are reproducible across runs.
- Comparison rows for the same query on the same repo run against
  `hercules` (if installed) and `git-of-theseus` (if installed) — logged,
  not asserted. We ship a table in the README, not a test failure.
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
- No content-based language detection (extension only).

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

| Crate | Why | Alternative rejected |
|-------|-----|----------------------|
| `gix` | pure-Rust git, no subprocess, gives hunks + rename via `blob_diff` | shell out to `git` — slower per-commit |
| `duckdb` (unbundled) | columnar OLAP + parquet native + window fns; new metric = new query | `sqlite` (row-store, slow on aggregations); polars (no persistence story) |
| `tree-sitter` + 7 grammars | required for function-level + exact comment split | regex (approximate, no function-level) |
| `rayon` | per-commit parallelism | manual threading |
| `clap` | CLI | — |
| `plotters` | — | **rejected** — no plotting in v1 |
| `ratatui`, `crossterm` | — | **rejected** — no TUI in v1 |
| `directories` | XDG paths | — |
| `time` | timestamps + tz | chrono |

**Removed vs prior spec:** ratatui, crossterm, plotters (never added,
now formally out), 12 of the 19 tree-sitter grammars currently linked
(kept: Rust, Python, JS, TS, Go, C, C++).

**Distribution: prebuilt binaries via `cargo dist` are the primary
install path.** Users download a static-linked binary from GitHub
releases; DuckDB is linked in at build time on our CI runners so end
users need nothing installed. `cargo install git-archaeologist` remains
supported as a fallback and requires `libduckdb` on the system (via
apt/brew/dnf; Arch users hit AUR). Bundled compile-from-source is
dropped — first build no longer takes 5-15 minutes for either path.

## Roadmap post-v1

- v1.1: extra grammars behind Cargo features (Java, Ruby, Bash, HTML,
  CSS, JSON, YAML, TOML, Markdown, Scala, Haskell, Zig, ...).
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
- 20-grammar tree-sitter fan-out.
- Language detection heuristics beyond ext map.
- Sentiment / DTW / any ML.
- Babelfish / UAST.
- `--exclude` flag / `default_exclude` config — use `sql WHERE`.
- `--module-depth` flag / `default_module_depth` config — module =
  first path segment, no knob.
- `commits.tz_offset_min` column — no v1 metric consumes it.
- `file_stats.is_test` column — misclassifies mixed files; derive from
  `funcs.kind` at query time.
