# git-archaeologist — Tasks

## Now — ship blockers

- [x] **Tests (legacy baseline).** `treesitter::count_lines`,
      `bucket::bucket_key`, `query::apply_view`, tempdir end-to-end smoke.
- [x] Fix `Cargo.toml` `repository` URL.
- [x] README refresh.
- [x] Coarse progress throttle (every 64 commits) → time-based (100 ms).
- [x] `--export-parquet <dir>` CLI verb.
- [ ] ~~TUI regression check.~~ Killed with the TUI (see SPEC rev 2).

## Test strategy — implementation (matches SPEC §Test strategy)

Four tiers. CI runs 0 + 1 on every push; 2 opt-in via feature flag;
3 nightly / manual.

### Tier 0 — unit

- [x] `bucket::bucket_key` — day / week (ISO wrap) / month / commit.
- [ ] `bucket::bucket_key` — `tag` variant.
- [x] `treesitter::count_lines` — Rust, Python, Go fixtures.
- [ ] `treesitter::count_lines` — JS, TS fixtures added.
- [ ] `treesitter::extract_funcs` — one fixture per grammar, function +
      test-attribute recognition (`#[test]`, `def test_*`, `@Test`, etc.).
- [x] `query::apply_view` — cumulative running-sum, dense-fill.
- [ ] `query::apply_view` — cohort dense-fill (new metric).
- [ ] Trailer parser — `Co-authored-by:`, `Signed-off-by:`, quoted names,
      unicode.
- [ ] Conventional Commit parser — `feat!:`, `fix(scope):`, `revert:`,
      malformed input.
- [ ] `.git-blame-ignore-revs` loader — comments, blanks, missing file.

### Tier 1 — golden repo integration

- [ ] Build fixture generator `tests/support/golden.rs` — `tempfile` +
      system `git`. 3 authors, 5 langs, 30 commits. Must include: rename,
      merge, revert, `Co-authored-by:` trailer, `feat!:` breaking commit,
      tag, `.git-blame-ignore-revs` entry.
- [ ] Golden snapshots under `tests/data/golden/` — one `.tsv` per
      subcommand output. Regenerate script `xtask/regenerate-goldens`.
- [ ] Test per subcommand: `burndown`, `cohort`, `survival`, `coupling`,
      `classify`, `hotspot`, `age`, `churn`, `sql` — byte-for-byte
      against golden.
- [ ] Round-trip test: `index && export parquet /tmp/x` → reopen parquet
      via DuckDB → row counts match.

### Tier 2 — small public repo smoke (`--features e2e`)

- [ ] Cargo feature `e2e` gates the module.
- [ ] Fixture clone helper — clone `ratatui-org/ratatui` at a pinned SHA
      into `$XDG_CACHE_HOME/git-archaeologist-tests/`. Reuse on rerun.
- [ ] Assertions:
      - `git rev-list --count` == `commits` rows (exact).
      - Total code lines within ±1% of `tokei --output json`.
      - Every subcommand exits 0, non-empty stdout, default filters.
      - Cohort latest-bucket surviving sum ≈ current total code (±0.5%).
      - Coupling top-1 pair matches hand-checked `git log --name-only`.
- [ ] Perf ceilings enforced: index < 30 s, any query < 500 ms.

### Tier 3 — mid & mid-large bench (`--features bench-large`)

- [ ] Cargo feature `bench-large` gates the module.
- [ ] `benches/fixtures.toml` — pinned SHAs for `ratatui-org/ratatui`
      (small), `astral-sh/uv` (mid, ~15k), `godotengine/godot`
      (mid-large, ~60k).
- [ ] Bench harness (`benches/bench.rs`) — indexes each fixture, runs
      every subcommand, asserts perf ceilings + loose correctness bounds
      (§SPEC table).
- [ ] RSS ceiling < 2 GB on mid-large — track via `getrusage` in harness.
- [ ] Optional side-by-side rows: if `hercules` / `git-of-theseus` on
      `$PATH`, log their timings; do not gate on them.
- [ ] Nightly GitHub Action `.github/workflows/bench.yml` — larger runner,
      pushes result JSON to a `benches/results/` branch.

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

- [x] Blame implementation — `git blame --incremental -w` subprocess per
      (latest sampled sha, path) tuple, aggregated into the new `blame`
      table `(sha, path, author_id, line_count)`. Parser handles the
      "commit metadata once, header-only for repeats" streaming format
      (see `parse_incremental` + tests).
- [x] `Lens::Ownership` series/breakdown queries land off this table
      (`ownership_series_by_author` in query.rs).
- [x] Streaming progress reporter — reuses the splash bar; a `Progress::Blame`
      variant emits every 100ms during the blame pass.
- [x] Cache wipe on `--reindex` extended to the `blame` table.
- [x] Historical blame — walk every sampled sha; skip shas already
      populated in the `blame` table (idempotent re-runs). Ownership
      series now varies across buckets, not just the latest snapshot.

## Slice 6 — UX bundle

Independent tweaks. Ship as micro-PRs.

- [ ] Command palette (`:`) — searchable actions replace `b`/`f`/`l`/`a`
      single-key modals for the discoverable path.
- [x] Substring filter inside author + language checklists (type to
      narrow, Backspace pops, Esc clears, `C` clears selection).
- [x] Sparkline column in Breakdown table (unicode blocks, tail of the
      current series per group).
- [x] Interactive x-axis zoom / pan — `,` / `.` pan by 25%, `-` / `=`
      zoom out / in (keys chosen to avoid the `L` = lens and `l` = language
      modal clashes in the original `h/l/H/L` spec).
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

- [x] Deleted `query::subpaths` — reintroduce alongside Slice 6 path-picker.
- [x] Deleted `config::merge_authors` — reintroduce alongside Slice 7 wizard.
- [x] Dropped stale `#[allow(dead_code)]` on `index::Progress`.
- [x] Delete orphaned `cache.sqlite` in `.git/git-archaeologist/` on
      `repo::open` (silent — the cache lives in XDG data now).

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
