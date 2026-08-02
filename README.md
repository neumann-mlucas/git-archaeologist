# git-archaeologist

Interactive TUI to explore the evolution of a git repository — LOC over time,
language mix, module composition, author contribution.

Three orthogonal lenses answer three questions:

- **Structure** — "What exists?" LOC snapshot, grouped by language or module.
- **Activity** — "What changed?" Churn events, grouped by language, module or author.
- **Ownership** — "Who owns it?" Blame-based per-line attribution (WIP, blocked on Slice 5).

Status: **work in progress**. See [`SPEC.md`](SPEC.md) for the v1 specification
and [`TASKS.md`](TASKS.md) for the roadmap.

## Build

```sh
cargo build --release
```

First build is slow — DuckDB is bundled and compiled from source (5-15 min).
Incremental builds hit sccache.

## Run

```sh
git-archaeologist                    # index + explore the repo in cwd
git-archaeologist path/to/repo
git-archaeologist --reindex          # force full rebuild of the cache
git-archaeologist --bucket week      # override auto bucketing (commit|day|week|month)
git-archaeologist --export-parquet out/   # export cache tables as Parquet, no TUI
```

### Keys

| key       | action                                              |
|-----------|-----------------------------------------------------|
| `q`       | quit                                                |
| `L`       | cycle lens (Structure → Activity → Ownership)       |
| `Tab`     | cycle group-by within the current lens              |
| `d`       | toggle view (Cumulative ↔ Delta)                    |
| `s`       | cycle sort column (Total → Δ → Group)               |
| `↑/↓`     | select row                                          |
| `Enter/→` | drill into module                                   |
| `←/Bksp`  | drill out                                           |
| `b`       | bucket size modal                                   |
| `f`       | date range modal                                    |
| `l`       | language filter                                     |
| `a`       | author filter                                       |
| `,` `.`   | pan date window left / right (25%)                  |
| `-` `=`   | zoom out / in (2× / 0.5×)                           |
| `r`       | reindex                                             |
| `?`       | help                                                |

### Cache

The DuckDB cache lives under the user's XDG data dir, keyed by the canonical
worktree path so multiple checkouts of the same repo don't collide:

```
~/.local/share/git-archaeologist/caches/<repo-name>-<hash>/cache.duckdb
```

`--reindex` wipes it. Delete the directory to fully reset.

### Config

Optional `~/.config/git-archaeologist/config.toml`:

```toml
default_bucket = "auto"       # commit | day | week | month | auto
default_view   = "cumulative" # cumulative | delta
default_group  = "language"   # language | module | author
default_lens   = "structure"  # structure | activity | ownership
palette        = "default"
```

Optional `~/.config/git-archaeologist/aliases.toml` merges identity variants:

```toml
[[alias]]
canonical_name  = "Alice"
canonical_email = "alice@example.com"
raw = [
    { name = "Alice Smith", email = "alice@work.com" },
    { name = "alice",       email = "alice@personal.com" },
]
```

## Stack

- [`ratatui`](https://ratatui.rs) — terminal UI
- [`gix`](https://github.com/GitoxideLabs/gitoxide) — pure-Rust git access
- [`tree-sitter`](https://tree-sitter.github.io) — per-language LOC parsing
- [`duckdb`](https://duckdb.org) — embedded columnar cache (bundled)

### Supported languages

Rust, Python, JavaScript, TypeScript, TSX, Go, C, C++, Java, Ruby, Bash, HTML,
CSS, JSON, YAML, TOML, Markdown, Scala, Haskell, Zig.

Files with an unknown extension are skipped from LOC counts (they still count
toward churn).
