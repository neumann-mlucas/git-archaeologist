# git-archaeologist

Rust CLI that computes derived time series from a git repository — LOC by
language / author / module, cohort survival, file coupling, function-level
churn, commit classification, file age — and emits them as
TSV / CSV / JSON / Parquet. One DuckDB cache per repo; every query returns
in under a second after the initial index.

Backed by `gix` (pure-Rust git), `duckdb` (columnar cache), and
`tree-sitter` (20 grammars). No plotting, no UI, no daemon.

Status: **v1 in development.** See [SPEC.md](SPEC.md) and [TASKS.md](TASKS.md).

## Metrics

Every sample below is real output from a run against the
[neovim](https://github.com/neovim/neovim) repository: **40,227 commits,
1,641 authors, 12 years of history.** Indexer wall time on a dev laptop:
~4 min. Cache size: ~570 MB. Query latency: < 1 s.

### `burndown`

Cumulative LOC per bucket. `--by language | author | module`.

```
bucket   language     loc
202631   Vim Script   343,785
202631   Lua          331,067
202631   Markdown       2,032
202631   Python           210
```

### `survival`

Kaplan-Meier survival of committed lines per birth-cohort. `--fit exp`
fits `ln(survival) = -λ·age` and returns a half-life scalar.

```
half_life = 580.9 days   (n=650 buckets, deaths=1,968,479)
```

### `coupling`

Top-N file pairs by co-occurrence in commits. `--max-files-per-commit N`
(default 50) drops squash / import commits.

```
path_a                             path_b                              co_commits
runtime/doc/options.txt            src/nvim/options.lua                437
runtime/lua/vim/filetype.lua       test/old/testdir/test_filetype.vim  320
runtime/lua/vim/_meta/options.lua  src/nvim/options.lua                270
```

### `hotspot`

Top-N functions by churn per bucket. `--lang <L>` required. Columns:
`bucket, path, func, kind, language, added, deleted, churn`.

```
bucket   path                       func              churn
202229   src/nvim/edit.c            get_literal       3,517
202233   src/nvim/screen.c          fill_foldcolumn   2,542
202225   src/nvim/getchar.c         vgetorpeek        1,978
```

### `age`

File-age histogram, 7-day bins.

```
age_days   files
4,543      495
4,515      253
```

### `churn`

Added / deleted per group per bucket. `--by module | lang | author`.

```
bucket   module    added   deleted
202631   runtime   3,196     846
202631   test      2,363   1,113
202631   src       1,774   2,037
```

### `classify`

Conventional Commit type counts per bucket. Unparseable subjects fall
into `untyped`.

```
bucket   feat  fix  refactor  breaking  revert  untyped
202631   100   32   6         0         1       25
```

### `cohort`

Cohort-stacked surviving LOC per bucket. Complements `survival` — one
returns the scalar half-life, the other the full per-cohort shape.

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

- `--from YYYY-MM-DD` / `--to YYYY-MM-DD` — inclusive date bounds.
- `--lang rust,python` — comma-list language filter.
- `--author SUBSTR` — canonical-name / email substring.
- `--path PREFIX` — path prefix filter.
- `--bucket auto|day|week|month|tag|commit` — bucketing.
- `--format tsv|csv|json|table` — default: `tsv` on pipe, `table` on TTY.
- `--by language|author|module` — group axis for `burndown` / `churn`.

Subcommand-specific:

- `coupling --max-files-per-commit N` (default 50).
- `survival --fit exp` — half-life scalar; returns NULL + `reason`
  column when < 100 deletion events.
- `hotspot --top N --lang L` — `--lang` required.

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

```sh
git-archaeologist sql "SELECT language, SUM(code)
                       FROM   file_stats
                       WHERE  bucket_key >= 20240101
                       GROUP  BY language
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

20 tree-sitter grammars built in.

- **Core 9** (full function extraction): Rust, Python, JavaScript,
  TypeScript (`.ts` + `.tsx`), Go, C, C++, Lua, Vim Script.
- **Extended 11** (LOC + comment split; function extraction best-effort
  by conventional node names): Java, Ruby, Bash, HTML, CSS, JSON, PHP,
  OCaml, Scala, Haskell, Markdown.

Line-level metrics work on any file the diff engine sees. Function-level
metrics (hotspot, function-cohort, test / code split) need `fn_kinds` —
all Core 9 plus Java, Ruby, Bash, PHP, OCaml, Scala, Haskell.

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
