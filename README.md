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
1,641 authors, 12 years of history (2014-W05 → 2026-W31, 651 weekly
buckets).** Indexer wall time on a dev laptop: ~4 min. Cache size:
~570 MB. Query latency: < 1 s.

Sparklines below use 8-level braille (`▁▂▃▄▅▆▇█`) — each column is one
week, decimated to fit 65 columns. Bars are `█` at 1× / 40-column scale.

### Overview: commits per week

```
▁▅▁▄▄▂▄▂▃▁▃▁▃▂▂▅▄▃▂▂▂▃▂▂▃▂▁▁▆▆▃▃▁▁▂▃▂▂▂▁▃▄▄▅▄▅▆▅▇▃▆▅▄▆▃▃▄▅█▆▄▄▄▇▆
min=2  median=46  max=181  buckets=651
first = 2014W05   last = 2026W31
```

Produced by `git-archaeologist sql "SELECT bucket_key, COUNT(*) FROM
commits WHERE NOT is_merge GROUP BY 1 ORDER BY 1"` piped to a sparkline.

### `burndown`

Cumulative LOC per bucket. `--by language | author | module`.

```
Vim Script  ▁▁▁▃▃▃▃▃▃▃▃▃▃▃▃▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▅▅▅▅▅▅▅▅▅▆▆▆▆▆▆▆▆▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇▇█▇  peak=415,692  now=343,785
Lua         ▁▁▁▁▁▁▁▁▁▁▁▁▂▂▂▂▂▂▂▂▂▂▂▂▃▃▃▃▃▃▃▃▃▃▃▃▃▄▄▄▄▄▄▄▅▅▅▅▆▆▆▆▆▆▆▆▇▇▇▇▇▇▇▇█  peak=331,067  now=331,067
C           ▁▁▁▁▁▁▁▁▁▁▁▁▁▁▂▂▂▂▂▂▂▃▃▃▃▃▃▃▄▄▄▄▄▄▄▄▄▅▅▅▆▆▆▆▆▆▆▆▆▆▆▆▇▇▇▇▇▇▇▇▇▇▇▇█  peak=517,063  now=276,382
Markdown    ▁▂▂▂▂▂▃▃▃▃▃▃▃▃▃▃▃▃▄▄▄▄▄▄▄▄▄▄▄▄▄▅▅▄▅▅▅▅▅▅▅▅▅▅▅▅▅▅▅▅▅▅▅▅▅▇▇▇▇▇█▅▆▆▆  peak=  3,171  now=  2,032
Bash        ▁▁▁▁▁▁▁▁▂▂▂▂▃▃▃▄▅▅▅▆▆▆▆▆▆▆▆▇▇▇▇▇▇▇▇▇▇█▇▇▇▇▇▆▆▆▅▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄▄  peak=  2,183  now=  1,301
```

Vim Script and Lua converge at ~340k lines. C peaked at 517k in ~2018
and shrank by nearly half. Bash rose then fell — a script-consolidation
episode.

### `survival`

Kaplan-Meier survival per birth cohort. `--fit exp` fits
`ln(survival) = −λ·age` and returns a scalar half-life.

```
half_life = 580.9 days   (n=650 buckets, 1,968,479 deletion events)
```

Aggregated to yearly cohorts (bar = survival %):

```
year       born      alive       dead   survival%
2014  1,330,801    451,480    879,321      33.9%   ██████
2015    142,164     28,896    113,268      20.3%   ████
2016    137,024     61,791     75,233      45.1%   █████████
2017    215,687     95,684    120,003      44.4%   ████████
2018    105,815     58,300     47,515      55.1%   ███████████
2019    147,699     49,031     98,668      33.2%   ██████
2020    100,337     35,452     64,885      35.3%   ███████
2021    199,871    101,603     98,268      50.8%   ██████████
2022    350,690    183,411    167,279      52.3%   ██████████
2023    327,165    194,350    132,815      59.4%   ███████████
2024    282,593    182,034    100,559      64.4%   ████████████
2025    226,775    175,174     51,601      77.2%   ███████████████
2026    170,408    151,344     19,064      88.8%   █████████████████
```

Monotonic ramp from 34% (2014) to 89% (2026) is exactly what the
`survival --fit exp` scalar fits.

### `coupling`

Top-N file pairs by co-occurrence in commits. `--max-files-per-commit N`
(default 50) drops squash / import commits.

```
path_a                              path_b                              co_commits
runtime/doc/options.txt             src/nvim/options.lua                        437
runtime/lua/vim/filetype.lua        test/old/testdir/test_filetype.vim          320
runtime/lua/vim/_meta/options.lua   src/nvim/options.lua                        270
runtime/doc/options.txt             runtime/lua/vim/_meta/options.lua           268
runtime/lua/vim/_meta/vimfn.lua     src/nvim/eval.lua                           248
src/nvim/eval.c                     src/nvim/version.c                          246
runtime/doc/eval.txt                src/nvim/eval.c                             237
src/nvim/ex_cmds.c                  src/nvim/ex_docmd.c                         216
src/nvim/eval.c                     src/nvim/ex_docmd.c                         213
src/nvim/ex_docmd.c                 src/nvim/ex_getln.c                         207
```

### `hotspot`

Top-N functions by churn per bucket. `--lang <L>` required. Columns:
`bucket, path, func, kind, language, added, deleted, churn`.

```
bucket   path                                 func              added  deleted  churn
202229   src/nvim/edit.c                      get_literal           0    3,517  3,517
202233   src/nvim/screen.c                    fill_foldcolumn       0    2,542  2,542
202232   src/nvim/spell.c                     ex_spellrepall        3    2,046  2,049
202225   src/nvim/getchar.c                   vgetorpeek            0    1,978  1,978
202137   src/nvim/viml/parser/expressions.c   viml_pexpr_parse    942    1,014  1,956
202234   src/nvim/search.c                    current_search        2    1,804  1,806
202210   src/nvim/regexp.c                    getoctchrs            6    1,798  1,804
201932   src/nvim/change.c                    open_line         1,101      626  1,727
```

`--lang lua` on the same repo top-heavy on `test/functional/` — the Lua
churn story lives in the test suite, not the core.

### `age`

File-age histogram, 7-day bins in raw output. Rolled up to 5 tiers
here:

```
0-90d                                              94
90d-1yr   █                                       204
1-2yr     ███                                     403
2-5yr     ███████████                           1,431
5yr+      ████████████████████████████████████  4,797
```

70% of files on disk today are older than 5 years — a fossil-heavy
codebase with a thin recent skin.

### `churn`

Added / deleted per group per bucket. `--by module | lang | author`.
When `--by module`, `--depth N` (default `1`) controls path-segment
granularity — `1` = top-level dir, `2` = one level deeper, and so on.

`--by module --depth 1`:

```
src          ▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▃▁▁▁▁▁▁█▁▁▁▁▁▁▁▁▁▁▁▁  +896,256  -796,079
runtime      ▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▄▁▁▁▄▁▁▁▁▁▁▁▁▁▁▁▁▁▃▁▁▁█▁▁▁▁▁▁▁▁▁▁▁▁▂▁▁▁▁▁▁  +795,115  -344,704
test         ▁▁▁▁▁▁▁▁▁▁▁▃▁▁▁▁▁▂▁▁▁▁▁▁▁▁▁▁▁▂▁▁▁▁▂▁▁▂▂▂▁▂▁▅▁▂▃▅▃▃▂▂▂▄▄▅▇▁▅█▁▂▃▃▂  +507,430  -193,185
third-party  ▁▂▁▁▁▁▁▁▁▁▁▁█▁▁▁▁▁▁▁▁▁▁▂▁▁▁▁▁▁▄▁▁▁▁▁▁▁▁▄▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁  +118,919  -184,243
```

Same query at `--depth 2` — the story sharpens: `src/nvim` is the C
core, `test/functional` is where the Lua-migration test churn lives,
and `src/po` was a one-shot i18n import in the early weeks.

```
src/nvim         ▁▁▁▁▂▁▁█▆▁▁▃▁▁▂▆▂▁▁▁▁▂▂▁▁▂▁▁▁▁▁▁▂▁▁▁▁▁▁▁▁▂▅▃▁▁▁▃▁▁▁▁▃▁▁▁▁▁▁▁ +1,212,341  -955,535
test/functional  ▁▁▁▁▃▁▁▁█▁▁▆▁▄▁▃▁▁▁▂▁▁▁▁▃▁▂▁▁▁▁▁▁▁▁▁▁▁▁▂▁▃▄▁▁▁▁▁▃▃▁▂▂▂▂▁▂▁▂▂   +490,894  -211,383
runtime/doc      ▁▁▁▁▁▁▂▁▆▁▁▂▁▁█▃▁▁▅▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▂▁▁▁▂▁▁▂▁▁▁▁▁▁▂▄▂▃▁▁▁▂   +322,074  -178,099
runtime/syntax   ▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁█▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁   +220,002   -61,600
src/po           █▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁▁   +232,931    -4,885
```

Files with fewer than `depth` segments group under their full path
(root-level files stay as-is).

### `classify`

Conventional Commit type counts per bucket. Unparseable subjects fall
into `untyped`. Last 10 weeks:

```
bucket   commits  fixes  feats  reverts  breaking  untyped
202622        51     12      1        0         0       28
202623        47     15      2        0         1       18
202624        88     25      8        0         1       29
202625        90     30      3        0         0       38
202626        88     29      6        0         1       35
202627       110     34     11        0         0       36
202628        84     35      6        0         4        4
202629        99     31     11        0         1       13
202630       104     31      7        0         0       31
202631       100     32      6        0         1       25
```

Weekly commit volume ~50 → ~100 through mid-2026; `feats` and `fixes`
roughly track each other; `breaking` mostly zero with occasional 1-4
weeks. `untyped` skews heavy some weeks (usually doc-only commits
under `runtime/`).

### `cohort`

Cohort-stacked surviving LOC per bucket. Pairs with `survival` — the
scalar half-life (`survival --fit exp`) is the shape's decay parameter;
`cohort` gives you the per-bucket stacked series to plot as a full
Theseus-style stream.

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
