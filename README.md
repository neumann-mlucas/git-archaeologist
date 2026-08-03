# git-archaeologist

Fast single-binary Rust CLI that extracts every interesting time series out of
a git repository — LOC over time, language mix, per-author burndown, cohort
survival, file coupling, commit classification, function-level churn — and
dumps them as TSV / CSV / JSON / Parquet for downstream tooling to plot.

`git-archaeologist` doesn't draw. It produces numbers. Pipe them to
[`youplot`](https://github.com/red-data-tools/YouPlot), [`datasette`](https://datasette.io),
the [`duckdb` CLI](https://duckdb.org), a notebook — whatever fits.

Answers questions like:

- How much of the code written in 2020 is still alive?
- What's the half-life of a line of code in this repo?
- Which files always change together?
- Which functions are churn hotspots?
- How has the language mix shifted per release tag?
- Who wrote what, respecting `.mailmap` + `Co-authored-by:` trailers?

Status: **v1 in development.** See [SPEC.md](SPEC.md) for the specification
and [TASKS.md](TASKS.md) for the roadmap.

## Install

Prebuilt binaries via `cargo dist` are the intended install path (Linux +
macOS). Until they land in a GitHub Release, build from source:

```sh
cargo install --path .
```

The `duckdb` crate is unbundled — it links against `libduckdb` on your
system. Install it first:

- Arch: `pacman -S duckdb` (or AUR)
- Debian/Ubuntu: `apt install libduckdb-dev`
- macOS: `brew install duckdb`

The binary installs as `git-archaeologist`. If you want the shorter
`git-arch`, add an alias:

```sh
alias git-arch=git-archaeologist
```

## Quick start

```sh
cd my-repo
git-archaeologist index                # build the cache
git-archaeologist burndown --by language | uplot line --xlabel date --ylabel loc
git-archaeologist survival --fit exp   # scalar half-life
git-archaeologist coupling --top 20 | uplot barplot --title 'file coupling'
```

`index` runs implicitly on first query — the explicit form just lets you time
the indexer separately.

## Subcommands

```
git-archaeologist [--repo PATH] <SUBCOMMAND>
```

- **`index`** — build / update the cache (implicit on any query).
- **`reindex`** — wipe cache + rebuild from scratch.
- **`export <fmt> <dir>`** — dump every table as `parquet`, `csv`, or `json`.
- **`sql "<query>"`** — raw DuckDB query, stdout tsv/table.
- **`burndown`** — cumulative LOC series. `--by language|author|module`.
- **`cohort`** — cohort-stacked LOC series — how much of era X is still here.
- **`survival`** — Kaplan-Meier survival; `--fit exp` returns a half-life scalar.
- **`coupling`** — top-N file pairs by co-occurrence in commits.
- **`classify`** — Conventional Commit type shares per bucket.
- **`hotspot`** — top-N funcs by churn per bucket. `--lang <L>` required.
- **`age`** — file age histogram (`now − first_touched_at`).
- **`churn`** — churn per module.

### Common flags

- `--from YYYY-MM-DD` — inclusive lower bound.
- `--to YYYY-MM-DD` — inclusive upper bound.
- `--lang rust,python` — filter by language (comma list).
- `--author SUBSTR` — canonical-name/email substring.
- `--path PREFIX` — path prefix filter.
- `--bucket auto|day|week|month|tag|commit` — time bucketing.
- `--format tsv|csv|json|table` — default: tsv on pipe, table on TTY.
- `--by language|author|module` — group axis for `burndown` / `churn`.

Subcommand-specific:

- `coupling --max-files-per-commit N` (default 50) — drops squash/import commits.
- `survival --fit exp` — returns a half-life scalar when ≥ 100 deletion events.
- `hotspot --top N --lang L` — `--lang` is required.

## Recipes

### youplot

```sh
# stacked LOC per language
git-archaeologist burndown --by language | uplot line --xlabel date --ylabel loc

# per-author net contribution
git-archaeologist burndown --by author   | uplot line --xlabel date

# top-20 coupled file pairs
git-archaeologist coupling --top 20      | uplot barplot --title 'file coupling'

# half-life scalar, no plot
git-archaeologist survival --fit exp
```

### DuckDB CLI

Dump the tables and query them directly:

```sh
git-archaeologist export parquet /tmp/repo/
duckdb -c "SELECT * FROM read_parquet('/tmp/repo/commits.parquet') LIMIT 5"
```

For a correct current-snapshot query (join through `commits.bucket_key` to
avoid summing across every sampled snapshot), prefer the built-in `sql`
subcommand — it runs against the live cache without export.

### Raw SQL escape hatch

```sh
git-archaeologist sql "SELECT language, SUM(code)
              FROM   file_stats
              WHERE  bucket_key >= 20240101
              GROUP  BY language
              ORDER  BY 2 DESC"
```

## Claude Code skill

`skills/git-arch-analyze/` ships a Claude Code skill that indexes a repo,
runs every subcommand, and writes an interpreted `ARCHAEOLOGY.md`
report. To enable globally:

```sh
mkdir -p ~/.claude/skills
ln -sf "$PWD/skills/git-arch-analyze" ~/.claude/skills/git-arch-analyze
```

Then invoke from any repo:

```
/git-arch-analyze [repo-path]
```

## Cache

DuckDB file under XDG data, keyed by canonical worktree path so multiple
checkouts don't collide:

```
~/.local/share/git-archaeologist/caches/<repo>-<hash>/cache.duckdb
```

`reindex` wipes it. Schema-version drift auto-wipes on next open with a
single line to stderr.

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

## Supported languages

Tree-sitter is linked in for **20 languages** in v1:

- **Core 9** (full function-level extraction): Rust, Python, JavaScript,
  TypeScript (`.ts` + `.tsx`), Go, C, C++, Lua, Vim Script.
- **Extended 11** (LOC + comment split; function extraction is best-effort
  by conventional node names): Java, Ruby, Bash, HTML, CSS, JSON, PHP,
  OCaml, Scala, Haskell, Markdown.

Line-level metrics (burndown, churn, coupling, age, cohort, survival) work
for any file the diff engine sees. Function-level metrics (hotspot,
function-cohort, test-vs-code split) rely on tree-sitter and only cover
langs with `fn_kinds` set (all Core 9 + Java, Ruby, Bash, PHP, OCaml,
Scala, Haskell).

## Stack

- [`gix`](https://github.com/GitoxideLabs/gitoxide) — pure-Rust git, hunks + rename detection via `blob_diff`
- [`duckdb`](https://duckdb.org) — columnar OLAP cache, native parquet
- [`tree-sitter`](https://tree-sitter.github.io) — per-language LOC + function extraction
- [`imara-diff`](https://github.com/pascalkuthe/imara-diff) — line-hunk producer over blob pairs
- [`clap`](https://docs.rs/clap) — CLI
- [`rayon`](https://docs.rs/rayon) — parallel churn walk, cohort fold, and tree-sitter snapshot

## Prior art we learn from

- [**git-of-theseus**](https://github.com/erikbern/git-of-theseus) — cohort stacks + survival curves. Slow Python; we replace it with Rust + DuckDB.
- [**hercules**](https://github.com/src-d/hercules) — one-pass DAG, coupling matrix, per-author burndown. We reuse the analyses in a smaller dep footprint.
