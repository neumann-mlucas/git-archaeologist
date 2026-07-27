# git-archaeologist

Interactive TUI to explore the evolution of a git repository — LOC over time,
language mix, module composition, author contribution.

Status: **scaffold / work in progress**. See [`SPEC.md`](SPEC.md) for the v1
specification and [`TASKS.md`](TASKS.md) for the implementation roadmap.

## Build

```sh
cargo build --release
```

## Run

```sh
git-archaeologist                    # index + explore the repo in cwd
git-archaeologist path/to/repo
git-archaeologist --reindex          # force full rebuild of the cache
git-archaeologist --bucket week      # override auto bucketing
```

## Stack

- [`ratatui`](https://ratatui.rs) — terminal UI
- [`gix`](https://github.com/GitoxideLabs/gitoxide) — pure-Rust git access
- [`tokei`](https://github.com/XAMPPRocky/tokei) — language stats (in-process)
- [`rusqlite`](https://github.com/rusqlite/rusqlite) — commit-keyed cache
