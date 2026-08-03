# git-archaeologist

Your repo has 5, 10, sometimes 20 years of history in it. Almost nobody
looks at it — the tools are either slow (`git log` for anything past a
summary) or single-purpose (`gitstats`, `hercules`, `git-of-theseus` —
one lens each, none of them cheap to run).

`git-archaeologist` walks the DAG once, stashes every derived metric in a
local DuckDB cache, and hands you a dozen time series to pipe wherever —
[`youplot`](https://github.com/red-data-tools/YouPlot),
[`datasette`](https://datasette.io), the [`duckdb` CLI](https://duckdb.org),
a notebook, or the bundled [Claude Code analysis skill](#claude-code-skill).

No charts. Just numbers.

It answers questions like:

- How much of the code written in 2020 is still alive?
- What's the half-life of a line of code in this repo?
- Which files always change together?
- Which functions are churn hotspots?
- How has the language mix shifted per release tag?
- Who wrote what, respecting `.mailmap` + `Co-authored-by:` trailers?

Status: **v1 in development.** See [SPEC.md](SPEC.md) for the specification
and [TASKS.md](TASKS.md) for the roadmap.

## What each metric tells you

Every sample below comes from a real indexing run against
[neovim](https://github.com/neovim/neovim): **40,227 commits, 1,641 authors,
12 years of history.** Indexing took ~4 minutes on a dev laptop; the cache
is ~570 MB. Every subcommand returns in under a second after that.

### `burndown --by language` — how has the language mix shifted?

```
bucket   language     loc
202631   Vim Script   343,785
202631   Lua          331,067
202631   Markdown     2,032
202631   Python       210
```

The Vim-Script-to-Lua migration in progress: Lua closed a 300k-line gap
from a standing start and now sits within 4% of Vim Script, still
climbing. The same query with `--by author` or `--by module` swaps the
group axis.

### `survival --fit exp` — what's the half-life of a line of code?

```
half_life = 580.9 days   (n=650 buckets, 1,968,479 deletion events)
```

Half of any given neovim line is gone in ~19 months. High-churn — compare
against a mature codebase where the same number is 5+ years. Skip
`--fit exp` for the full per-cohort table.

### `coupling --top 5` — which files always change together?

```
path_a                            path_b                              co_commits
runtime/doc/options.txt           src/nvim/options.lua                437
runtime/lua/vim/filetype.lua      test/old/testdir/test_filetype.vim  320
runtime/lua/vim/_meta/options.lua src/nvim/options.lua                270
```

Docs welded to code, tests welded to code. Touch `options.lua` and
`options.txt` moves with it 40% of the time — the classic "docs are code"
signal. If you're planning a refactor, this list is your blast-radius
preview.

### `hotspot --lang c --top 5` — which functions are churn hotspots?

```
bucket   path                       func              churn
202229   src/nvim/edit.c            get_literal       3517
202233   src/nvim/screen.c          fill_foldcolumn   2542
202232   src/nvim/spell.c           ex_spellrepall    2049
202225   src/nvim/getchar.c         vgetorpeek        1978
```

The top rows here are one-time deletions (a bucket-2022-W29 rewrite
pass), not chronic hotspots. The analyze skill's trajectory check
distinguishes "cooling / flat / worse" — a single-shot churn spike
usually means somebody finally deleted dead code.

### `age` — how old is the code sitting on disk today?

```
age_days   files
4543       495
4515       253
...
```

Two big clumps around 4,500 days old (~12.4 years) — the original
vim-import fossil layer, still on disk. Newer work sits above. An
age histogram with two distinct modes = "legacy layer with active
skin", the most common shape in long-lived projects.

### `churn --by module` — which subsystem is most active?

```
bucket   module    added   deleted
202631   runtime   3,196   846
202631   test      2,363   1,113
202631   src       1,774   2,037
```

This week: `runtime/` and `test/` are net-positive (growing), `src/` is
net-negative (shrinking). The C core is being replaced by Lua in
`runtime/` — the migration is visible in a single week of churn.

### `cohort` — how much of era X is still here?

Cohort-stacked LOC per bucket. Reads best as a plot (feed to
`uplot line`): each stripe is the birth-bucket, height is how much of
that cohort survives today. Pairs naturally with `survival --fit exp` —
the fit gives you the scalar, the table gives you the shape.

### `classify` — commit intent breakdown

```
bucket   feat  fix  refactor  breaking  revert  untyped
202631   100   32   6         0         1       25
```

Requires the repo to use Conventional Commits. Untyped commits (25 this
week for neovim) fall through — usually `runtime/` doc-only changes.
`breaking = 0` + `revert = 1` means a stable week; the reverse means
scrolling `git log` is due.

### Others

- **`index` / `reindex`** — build or wipe the cache.
- **`export parquet|csv|json <dir>`** — dump every table for offline
  analysis in duckdb / pandas / whatever.
- **`sql "<query>"`** — raw DuckDB against the cache, for anything not
  covered by the built-ins.

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
git-archaeologist index                # build the cache (idempotent)
git-archaeologist burndown --by language | uplot line --xlabel date --ylabel loc
git-archaeologist survival --fit exp   # scalar half-life
git-archaeologist coupling --top 20 | uplot barplot --title 'file coupling'
```

`index` runs implicitly on the first query — the explicit form just lets
you time the indexer separately.

## Subcommands & flags

```
git-archaeologist [--repo PATH] <SUBCOMMAND>
```

Subcommands: `index`, `reindex`, `export <fmt> <dir>`, `sql "<query>"`,
`burndown`, `cohort`, `survival`, `coupling`, `classify`, `hotspot`,
`age`, `churn`. See [above](#what-each-metric-tells-you) for what each
one shows.

Common flags across every query subcommand:

- `--from YYYY-MM-DD` — inclusive lower bound.
- `--to YYYY-MM-DD` — inclusive upper bound.
- `--lang rust,python` — filter by language (comma list).
- `--author SUBSTR` — canonical-name / email substring.
- `--path PREFIX` — path prefix filter.
- `--bucket auto|day|week|month|tag|commit` — time bucketing.
- `--format tsv|csv|json|table` — default: tsv on pipe, table on TTY.
- `--by language|author|module` — group axis for `burndown` / `churn`.

Subcommand-specific:

- `coupling --max-files-per-commit N` (default 50) — drops squash /
  import commits that would inflate every pair.
- `survival --fit exp` — returns a half-life scalar when there are ≥ 100
  deletion events (else NULL + a `reason` column).
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

For a correct current-snapshot query (join through `commits.bucket_key`
to avoid summing across every sampled snapshot), prefer the built-in
`sql` subcommand — it runs against the live cache without export.

### Raw SQL escape hatch

```sh
git-archaeologist sql "SELECT language, SUM(code)
              FROM   file_stats
              WHERE  bucket_key >= 20240101
              GROUP  BY language
              ORDER  BY 2 DESC"
```

## Claude Code skill

`skills/git-arch-analyze/` ships a [Claude Code](https://claude.com/claude-code)
skill that indexes a repo, runs every subcommand, and writes an
interpreted `ARCHAEOLOGY.md` report — the numbers plus a narrative that
names the health signals, the fossil layers, and where the bodies are
buried. To enable globally:

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

- [`gix`](https://github.com/GitoxideLabs/gitoxide) — pure-Rust git, hunks + rename detection via `blob_diff`.
- [`duckdb`](https://duckdb.org) — columnar OLAP cache, native parquet.
- [`tree-sitter`](https://tree-sitter.github.io) — per-language LOC + function extraction.
- [`imara-diff`](https://github.com/pascalkuthe/imara-diff) — line-hunk producer over blob pairs.
- [`clap`](https://docs.rs/clap) — CLI.
- [`rayon`](https://docs.rs/rayon) — parallel churn walk, cohort fold, and tree-sitter snapshot.

## Prior art we learn from

- [**git-of-theseus**](https://github.com/erikbern/git-of-theseus) — cohort stacks + survival curves. Slow Python; we replace it with Rust + DuckDB.
- [**hercules**](https://github.com/src-d/hercules) — one-pass DAG, coupling matrix, per-author burndown. We reuse the analyses in a smaller dep footprint.
