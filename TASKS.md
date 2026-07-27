# git-archaeologist — Tasks

## Now — ship blockers

- [ ] **Tests.** Zero coverage after 5 refactor slices. Minimum set:
      - `treesitter::count_lines` golden files per language (Rust, Python,
        Go, C++, TOML — 5 fixtures, ~30 lines each with known counts).
      - `bucket::bucket_key` unit — day / week (ISO year) / month / commit.
      - `query::apply_view` — Structure Delta dense-fill, Activity
        Cumulative running-sum.
      - end-to-end smoke — `tempfile` a tiny git repo, index, assert
        row counts across `commits` / `churn` / `file_stats`.
- [ ] **TUI regression check.** Never launched interactively since the Lens
      reframe landed. Boot each lens × view, drill in, apply filters,
      screencap.
- [ ] Fix `Cargo.toml` `repository` URL (currently unverified).
- [ ] README refresh — Lens reframe, XDG cache path, DuckDB, tree-sitter
      grammar list.
- [ ] Coarse progress throttle (every 64 commits) → time-based (every
      100 ms) so the bar doesn't look stalled on slow diffs.
- [ ] `--export parquet <dir>` CLI verb (DuckDB `COPY … TO … FORMAT
      PARQUET` — ~15 LOC).

## Slice 4 (continued) — LOC engine

- [x] Step A: tree-sitter core + Rust grammar.
- [x] Step B: tokei deleted.
- [x] Step C: 19 grammars (Python, JS/TS, Go, C/C++, Java, Ruby, Bash,
      HTML, CSS, JSON, YAML, TOML, Markdown, Scala, Haskell, Zig).
- [ ] **Step D** — extend `file_stats` with `functions`, `types`,
      `imports`, `test_lines`; per-language `.scm` queries.
- [ ] **Step E** — `GroupBy::Function` + `GroupBy::NodeKind` under
      Structure lens.
- [ ] **Step F** — test vectors for D+E parity.

*Deferred until Slice 5+6 ship. Semantic queries are a smaller user win
than blame-based ownership.*

## Slice 5 — Ownership lens (blame cache)

Real per-line author attribution. Unblocks the empty Ownership panel;
required by Slice 7 wizard.

- [ ] Blame implementation — likely `git blame --incremental` subprocess
      per (path, sampled sha) tuple, cached in a new `blame` table:
      `(sha, path, author_id, line_count)`.
- [ ] Cache growth guard — bound by (files × sampled buckets); prune
      LRU or wipe on `--reindex`.
- [ ] `Lens::Ownership` series/breakdown queries land off this table.
- [ ] Streaming progress reporter — blame is slow, needs a bar.

## Slice 6 — UX bundle

Independent tweaks. Ship as micro-PRs.

- [ ] Command palette (`:`) — searchable actions replace `b`/`f`/`l`/`a`
      single-key modals for the discoverable path.
- [ ] `/` fuzzy filter inside author + language checklists.
- [ ] Sparkline column in Breakdown table (per-row trend, tiny).
- [ ] Interactive x-axis zoom / pan (`h/l/H/L`).
- [ ] Diff-plot side-by-side (two date ranges or two refs).

## Slice 7 — Ownership wizard

Depends on Slice 5.

- [ ] First-run flow that walks the user through `.mailmap` + user
      aliases when unmerged identities are detected. Replaces the
      deleted `unmerged_candidates` heuristic.

## Release

- [ ] `cargo dist` or manual release binaries (Linux + macOS).
- [ ] Screencast for README.
- [ ] Tag `v1.0.0` — after Slice 5 minimum.

## Cleanup

- [ ] Delete `query::subpaths` (currently `#[allow(dead_code)]`) — either
      wire into path-picker (Slice 6) or drop.
- [ ] Delete `config::merge_authors` if Slice 7 wizard slips —
      re-introduce alongside if resumed.
- [ ] Drop `#[allow(dead_code)]` on `index::Progress` variants — they
      *are* consumed by the splash bar; comment is stale.
- [ ] Delete orphaned `cache.sqlite` in `.git/git-archaeologist/` at
      first Ownership indexer run (opt-in prompt).

## Killed (do not resurrect without a fresh case)

- ~~Delta moving-average window~~ (nobody asked)
- ~~CSV/JSON export current view~~ (Parquet export supersedes)
- ~~Cross-branch compare as a separate mode~~ (Slice 6 diff-plot subsumes)
- ~~Rename tracking behind flag~~ (only if Slice 5 needs it)
- ~~Path-picker modal via `query::subpaths`~~ (either wire or delete
  `subpaths` — see Cleanup)
- ~~Stacked-area chart~~ (ratatui Chart is line-only; custom widget is
  a huge cost for marginal win)
- ~~Free-form date range picker~~ (Slice 6 command palette subsumes)
- ~~`unmerged_candidates` heuristic + AliasMerge modal~~ (Slice 7
  wizard replaces)
- ~~"Author last-touch attribution" as a v1.1 backlog item~~ (subsumed
  into Slice 5 blame)

---

# Possible improvements (unranked idea pool)

Fresh directions worth exploring. Not planned — capture, revisit.

## Perf

- Rayon-parallelize the churn walk. `gix::Repository` is `!Sync`; needs
  per-thread `gix::open()` on the same `.git/` dir. Expected 3-5× on
  fast disks.
- Rayon-parallelize tree-sitter parse across sampled commits (same
  per-thread repo trick).
- Skip `find_object` when we already have a blob cached — currently
  we still hit gix even for a cache hit.
- Incremental Parquet writes — `INSERT INTO ext_table SELECT * FROM
  new_rows` + `COPY TO`. Would enable "append-only, never rewrite"
  storage.
- Bench harness on a real large repo (Linux kernel subset, gecko-dev,
  chromium). Validate whether the DuckDB swap was actually justified.
- `PRAGMA memory_limit`, `PRAGMA threads` for DuckDB. Currently
  default.

## Insights (needs Slice 4 D and/or Slice 5)

- **PR-impact CLI mode** — `--diff main..HEAD --output json` prints
  LOC delta by lang / module / author. GitHub Action integration.
- **Bus-factor rollup** — files with lowest author count in current
  scope. Ownership-lens query.
- **Test-vs-prod ratio over time** — Slice 4 Step D + `test_lines`
  column.
- **File age distribution** — histogram of "first-touched date" per
  path across current scope.
- **Longest-lived files** — sort by (now − first-touched) descending;
  pin as "core" files.
- **Contribution heatmap** — hour-of-day × day-of-week, activity
  intensity. Fun for one-person repos, useful for team ones.
- **Author retention** — for each author, first-and-last commit date;
  gaps > N days = churn.
- **Language lifecycle** — birth/death dates per language in the
  repo (first appearance, last non-zero LOC).
- **Config-change detection** — flag commits where a config file
  edit correlates with a churn spike elsewhere.
- **Deleted-code archaeology** — search "what was in file X before
  Y" without checking out. Needs history walk, not the sampled cache.
- **Function-level delta** — Slice 4 Step E unlock; "biggest single-
  function LOC delta this month".

## Ergonomics

- `.git-archaeologist.toml` **at repo root** — teams / layer maps,
  overrides `default_lens`, ignore-glob patterns. Distinct from user
  `~/.config/…/config.toml`.
- **Config-driven semantic groups** — `[group.team]` sections that
  turn `GroupBy` into a plugin surface (backend / frontend / QA
  buckets over paths).
- **First-run tour** — 5-line onboarding pop-up covering `L`, Tab,
  Enter, `?`.
- **Cache prune command** — `git-archaeologist prune --older-than 30d`
  cleans stale entries from `~/.local/share/…/caches/*`.
- **Multi-repo dashboard** — pass N paths; each becomes a tab / pane.
- **File-tree side panel** — under path scope, show a mini file tree
  with per-node LOC totals.

## Distribution

- **WASM build** — compile core to wasm, browser demo that clones a
  repo via `isomorphic-git`. Zero-install README demo.
- **Editor extensions** — nvim / VS Code plugins that query a
  headless daemon (`git-archaeologist serve`) over unix socket.
- **Library split** — `git-archaeologist-core` (indexer + query) as
  a crate; the TUI binary depends on it. Enables downstream tools
  (LSP-style code lens, PR bot).

## Data model

- **Packed bucket keys** — `(scheme_tag: u8, ordinal: u32)` in one
  `i64` so old + new scheme rows can coexist safely. Kills a class
  of "force wipe on scheme change" bugs (currently already gone,
  but the model is fragile).
- **Full commit DAG** — `commit_parents` table with parent-num. Enables
  branch/tag support, cross-branch compare, first-parent-only view
  as a query flag not an indexer flag.
- **Rename tracking behind flag** — gix diff already computes it;
  wire and gate.

## Grammars (extend Slice 4 Step C)

- PHP, Kotlin, Swift, Elm, Nim, Elixir, Erlang, Clojure, R, Julia,
  Solidity, GraphQL, Dart, OCaml, Lua.
- Group-membership metadata: mark each language "systems", "web",
  "config", "shell" so a "systems-vs-web" lens becomes cheap.

## Meta

- **Golden repo fixture** — a small git repo shipped in `tests/data/`
  with known counts + a snapshot of expected series/breakdown for
  end-to-end regressions.
- **Property tests** (`proptest`) for `apply_view`: assert Σ Δ ==
  final − initial across all groups and buckets.
- **Fuzz** the tree-sitter classifier with `cargo-fuzz` on random
  UTF-8 inputs. Cheap crash detection.
