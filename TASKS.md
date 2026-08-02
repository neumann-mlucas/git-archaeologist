# git-archaeologist — TASKS (v1 rev 2)

The tree in this repo is a TUI prototype (ratatui + crossterm + 19 tree-sitter
grammars + blame subsystem). SPEC v1 rev 2 collapses the surface to a CLI-only
analyzer with a DuckDB cache. This file is the demolition + rebuild plan.

Order matters: delete before rebuild; rebuild on green tests. Each phase has
a green gate before the next starts.

## Decisions

- **Grammars.** V1 built-in set = 7: Rust, Python, JavaScript,
  TypeScript, Go, C, C++. Other 12 currently in `Cargo.toml` deleted
  outright (not feature-gated) — post-v1 add-back per SPEC §Roadmap.
- **Blame.** Deferred to v1.2. Delete `src/index/blame.rs`, the `blame`
  table, and every query that touches it. No preview gate.
- **Cache migration.** Auto-nuke stale caches on first v1 run; log one
  line. Matches recent commit `2789aeb`. No `schema_version` gymnastics.

## Phase 0 — demolition ✅

Cut everything the CLI-only spec kills. No new features here.

- [x] Delete `src/app.rs`, `src/ui/` (`breakdown.rs`, `chart.rs`,
      `filters.rs`, `modals.rs`, `palette.rs`, `mod.rs`).
- [x] Drop `ratatui`, `crossterm` from `Cargo.toml`.
- [x] Delete `src/index/blame.rs`, `blame` table in `cache/schema.rs`,
      all blame queries in `cache/queries.rs` and `query.rs`.
- [x] Prune tree-sitter grammar deps in `Cargo.toml` to Rust, Python,
      JavaScript, TypeScript, Go, C, C++. Delete the other 12
      (`java`, `ruby`, `bash`, `html`, `css`, `json`, `yaml`, `toml-ng`,
      `md`, `scala`, `haskell`, `zig`) and their registrations in
      `src/index/treesitter.rs`.
- [x] `Cargo.toml`: update `description`.
- [x] Drop `duckdb` `bundled` feature; switch to unbundled + `cargo dist`
      prebuilt binary path (SPEC §Dependency budget, §Distribution).
- [x] Rewrite `src/main.rs`: remove ad-hoc `--export-parquet` flag,
      replace with `clap` subcommand tree.
- [x] Remove stale `tempfile` / `assert_cmd` usage that only exercised
      the TUI harness.

**Green gate:** `cargo build --release` succeeds. Kept unit tests
(`bucket`, `treesitter::count_lines`) still pass. `cargo tree` shows no
direct ratatui/crossterm/plotters (crossterm still appears transitively
via `comfy-table` under `duckdb` — not a v1 concern).

## Bug fixes from Phase 1 audit (2026-08-02)

Round-trip audit of the Phase 1 landing found four SPEC-drift bugs and
two smaller ones. All fixed on top of the existing Phase 1 code:

- [x] `main.rs` `TABLES` const missed `line_births` — `export` dumped
      11 tables instead of 12, silently losing cohort data on parquet
      round-trip. Added.
- [x] Query subcommands ignored `q.bucket` when auto-indexing a cold
      cache. Threaded through a new `pick_bucket()` helper and added
      `config::default_bucket` load path.
- [x] `hotspot` had `--lang` optional at CLI parse time (SPEC says
      required); the query bailed but the error was late. Enforced at
      dispatch.
- [x] `survival --fit` accepted any string silently — typos yielded a
      "no fit" run. Now errors on anything other than `exp`.
- [x] `survival()` plumbed `Filters` but the `births` CTE only used
      `path_filter`; `--from`/`--to` never filtered the cohort births.
      Applied `commit_where()` + author join at the birth CTE.
- [x] SPEC §Scope wanted shallow clones to warn+degrade; `Repo::is_shallow`
      existed but was never called. Wired an eprintln at startup.

## Phase 1 — schema + CLI skeleton ✅

- [x] Rewrite `cache/schema.rs` to match SPEC §Data model exactly. Adds
      `line_births` (schema_version = "2") on top of SPEC's base tables.
- [x] Cache open path: schema-version mismatch wipes the DuckDB file and
      logs `cache: schema drift, wiped <path>`.
- [x] `clap` subcommand tree for `index`, `reindex`, `export`, `sql`,
      `burndown`, `cohort`, `survival`, `coupling`, `classify`, `hotspot`,
      `age`, `churn`.
- [x] Common flags plumbed on every query subcommand: `--from`, `--to`,
      `--bucket`, `--lang`, `--author`, `--path`, `--format`, `--by`.
- [x] Subcommand-specific flags: `coupling --max-files-per-commit`,
      `survival --fit exp`, `hotspot --top --lang`.
- [x] `config.toml` shrunk to `default_bucket` only.
- [x] `--format` default resolver: `tsv` on pipe, `table` on TTY (via
      `std::io::IsTerminal`).

## Phase 2 — indexer (SPEC §Indexing pipeline, Phase 1) ✅

- [x] Full DAG walk from every ref tip → `commits`, `commit_parents`
      (with `parent_idx`).
- [x] Committer identity into `commits.committer_id` (separate from author).
- [x] Tag walk (`refs/tags/*`) with tagger date → `tags`.
- [x] Trailer parser (`Co-authored-by:`, `Signed-off-by:`, quoted names,
      unicode) canonicalized through mailmap + aliases →
      `commit_trailers(sha, author_id, role)`.
- [x] Conventional Commit parser (`feat!:`, `fix(scope):`, `revert:`,
      malformed input) → `commits.msg_type`, `is_breaking`, `is_revert`.
- [x] `.git-blame-ignore-revs` loader (comments, blank lines, missing
      file OK) → `commits.ignored_blame`.
- [x] Per non-merge commit: `gix::blob_diff` with rename detection →
      four-line-range `hunks` + numstat rollup in `file_churn`.
      (Serial — parallel via rayon deferred to v1.x perf polish.)
- [x] Sampled commits only: tree-sitter LOC + function extraction →
      `file_stats`, `funcs`. Symlinks / submodules / binaries skipped.
- [x] `bucket::bucket_key` gains a `tag` variant (boundary = each
      reachable tag, sorted by tagger date).
- [x] Progress reported to stderr every 100 ms (done/total/rate/ETA).
- [x] Incremental reindex: skip SHAs already in `commits`.
      Tail-bucket promotion when a sampled commit changes is deferred
      to v1.x — cache wipe + full reindex covers all use cases today.

## Phase 3 — cohort fold (SPEC §Indexing pipeline, Phase 2) ✅

- [x] Group `hunks` by post-rename final path (rename chain via
      `prev_path`, cycle-guarded resolver).
- [x] Per-file fold: `Vec<(sha, bucket)>` per line, splice via the
      standard `old_file → new_file` merge (copy pre-hunk region, skip
      `old_len`, emit `new_len` of current sha). Serial — rayon
      parallelism across files deferred to v1.x perf polish.
- [x] Materialize `line_births(path, line_no, birth_sha, birth_bucket)`
      as a table; rebuilt on every `index` run.
- [ ] Incremental: rerun fold only for files whose hunk set changed.
      Deferred to v1.x — full-rebuild is fast enough at v1 scale.

## Phase 4 — query subcommands (SPEC §Metrics) ✅

- [x] `burndown` — `--by language` exact, `--by author` approximate
      (running net-contribution from `file_churn`; true cohort attribution
      would need line-birth × author join).
- [x] `cohort` — cohort-stacked surviving LOC per bucket, driven by
      `line_births`.
- [x] `survival` — per-cohort born/alive/dead + survival ratio.
      `--fit exp` fits `ln(survival) = -λ·age` via linear regression,
      returns half-life scalar when ≥ 100 deletion events (else NULL +
      `reason` column).
- [x] `coupling` — top-N file pairs by co-commit,
      `--max-files-per-commit N` (default 50).
- [x] `classify` — commits + fixes/feats/reverts/breaking/untyped per
      bucket.
- [x] `hotspot` — top-N funcs by churn (hunks attributed to funcs at the
      latest-sampled snapshot per path); `--lang` required.
- [x] `age` — file age histogram bucketed at 7-day granularity.
- [x] `churn` — `--by module|lang|author`.
- [x] `sql "<query>"` — raw DuckDB, stdout tsv/csv/json/table.
- [x] `export {parquet|csv|json} <dir>` — one file per table (11 tables).

## Phase 5 — tests (SPEC §Test strategy, four tiers)

CI runs Tier 0 + 1 on every push; Tier 2 opt-in via feature flag;
Tier 3 nightly / manual.

### Tier 0 — unit ✅

- [x] `bucket::bucket_key` day / week (ISO wrap) / month / commit.
- [x] `bucket::tag_bucket_key` — before-first-tag, at-and-after, empty list.
- [x] `treesitter::count_lines` — Rust, Python, Go, C++ fixtures (JS/TS/C
      exercised via `extract_funcs` tests + tier-1 golden run).
- [x] `treesitter::extract_funcs` — Rust (`#[test]`, `#[cfg(test)] mod`),
      Python (`test_*`), Go (`Test*`), JS/TS (function + method), C
      (function). C++ + call-site test detection covered by path
      fallback + tier-1 golden.
- [x] `cohort::fold_file` — addition-only births, modification updates
      only new lines, full delete leaves empty state, multiple hunks in
      one commit apply left-to-right, rename chain resolution (1 hop, 2
      hops, cycle guard).
- [x] Trailer parser — `Co-authored-by:`, `Signed-off-by:`, quoted names,
      unicode, subject-line ignored.
- [x] Conventional Commit parser — `feat!:`, `fix(scope):`, `revert:`,
      `refactor(core)!:`, git-generated `Revert "..."`, malformed input.
- [x] `.git-blame-ignore-revs` loader — comments, blank lines, missing
      file.
- [x] `query::Filters` — date parsing (from midnight, to exclusive next
      day), WHERE-clause composition, sql-escape, empty defaults.

### Tier 1 — golden repo integration ✅ (invariant-based, not byte-exact)

- [x] `tests/golden.rs` — `tempfile` + system `git`. 3 authors, 7 langs,
      30+ commits. Includes rename, merge, revert, `Co-authored-by:`
      trailer, `feat!:` breaking, annotated tag,
      `.git-blame-ignore-revs`, multi-file commits (for coupling).
- [x] Row-invariant assertions per subcommand (`commits >= 30`, `tags = 1`,
      `is_breaking = 1`, `>= 1 trailer`, `>= 1 ignored_blame`,
      `>= 1 rename hunk`).
- [x] Every subcommand exits 0 with non-empty stdout (`burndown ×2`,
      `classify`, `churn ×2`, `age`, `coupling`, `hotspot`, `cohort`,
      `survival`).
- [x] Round-trip: `index && export parquet <dir>` writes all 12 tables
      (including `line_births`).
- [ ] Byte-exact goldens + `xtask/regenerate-goldens` — deferred.
      Timestamps + DuckDB output formatting drift; invariant tests catch
      regressions with less maintenance cost.

### Tier 2 — small public repo smoke (`--features e2e`) ✅

Passes end-to-end after the indexer moved to all-tables DuckDB Appender
(see §Indexer perf below). ratatui v0.25.0 fixture, 2.6k commits (walker
follows all refs), 800k `line_births` rows, whole test (index + every
subcommand + assertions) in ~100s on a dev box.

- [x] Cargo feature `e2e` gates the module (`tests/tier2_smoke.rs`).
- [x] Fixture: `ratatui-org/ratatui` pinned to tag `v0.25.0`, checked
      out as a real branch (`pin-v0.25.0`) because git-archaeologist
      rejects detached HEAD. Full clone (no `--depth`) to avoid
      shallow-mark side-effect. Skips cleanly when no network + no
      cached clone.
- [x] `commits` row count ≥ `git rev-list --count HEAD` (indexer walks
      every ref per SPEC, so `>=` not `==`).
- [x] Every subcommand exits 0 with non-empty stdout under default
      filters.
- [x] Cohort surviving sum ≈ current sampled code within 50% (loose;
      SPEC 0.5% needs cohort language-classified `code`-only lines,
      out for v1).
- [x] Coupling top-1 pair `co_commits > 1` invariant (loose vs
      hand-check; upgrade to exact hand-check when fixture SHA-pinned
      in Tier 3).
- [x] Perf ceilings: index ≤ 180 s, any query ≤ 5 s. Real observed on
      dev box: index ~90 s, cohort ~2.4 s. SPEC targets (30 s / 500 ms)
      remain a v1.x aspiration; documented `ponytail:` in the test.
- [ ] Total code lines within ±1% of `tokei --output json .` — tokei
      cut from the dep tree in Phase 0. Re-add as `have_tokei()`
      conditional if a real user needs it.

### Indexer perf — first pass ✅

Landed on the Appender path. Move every write-heavy table (`commits`,
`commit_parents`, `commit_trailers`, `hunks`, `file_churn`,
`file_stats`, `funcs`) to `Connection::appender(...)`. Removed
`Connection::transaction()` (needs `&mut`, blocks appender borrow) and
the tx-chunk bookkeeping. Cost: dropped `ON CONFLICT DO UPDATE` — safe
because `wipe_data()` runs before `force_full` and the incremental
path filters SHAs via `already: HashSet<String>`, so we never insert a
duplicate.

Numbers on ratatui (2589 commits reached via all-refs walk):

| Path                        | Wall     | Peak RSS |
|-----------------------------|----------|----------|
| pre-fix (single long tx)    | OOM at 4-8 GB | 4-8 GB |
| Appender + 50-chunk tx      | ~25 min, degrading rate | 1.5 GB |
| **Appender everywhere**     | **~94 s, flat 42/s**  | **< 500 MB** |

Still on the v1.x list:

- Rayon-parallelize the churn walk (SPEC §Indexing Phase 1). Serial
  gix::blob_diff is now the dominant cost.
- Stream `churn::batch_all` iterator instead of collecting a full
  `HashMap<String, CommitDiff>` up front (peak RSS win on big repos).
- Bring cohort/survival queries under the SPEC 500 ms bar
  (materialized view for the (bucket, cohort) grid).

### Tier 3 — mid & mid-large bench (`--features bench-large`, nightly)

- [ ] Cargo feature `bench-large` gates the module.
- [ ] `benches/fixtures.toml` — pinned SHAs for `ratatui-org/ratatui`
      (small, ~5k), `astral-sh/uv` (mid, ~15k), `godotengine/godot`
      (mid-large, ~60k).
- [ ] `benches/bench.rs` harness — index each fixture, run every
      subcommand, assert perf table + loose correctness bounds.
- [ ] RSS ceiling < 2 GB on mid-large — track via `getrusage`.
- [ ] Optional side-by-side rows: if `hercules` / `git-of-theseus` on
      `$PATH`, log their timings; do not gate.
- [ ] `.github/workflows/bench.yml` — larger runner, pushes result JSON
      to a `benches/results/` branch.

## Phase 6 — release

- [ ] `cargo dist init` + workflow — deferred; user action at release
      cut. Prebuilt targets: `x86_64-unknown-linux-gnu`,
      `aarch64-apple-darwin`, `x86_64-apple-darwin`.
- [x] README rewrite — CLI usage, subcommand table, common flags,
      `youplot` / `duckdb` recipes, cache + config docs.
- [x] `Cargo.toml` package metadata — description, keywords,
      categories, homepage, authors, MSRV, explicit `[[bin]]`.
- [ ] Tag `v1.0.0` — user action.

## Killed (do not resurrect without a fresh case)

All items below are killed by SPEC v1 rev 2. See SPEC §Killed for scope
reasoning.

- TUI in every form — chart, breakdown, modals, palette, sparklines,
  filter checklists, interactive zoom / pan, first-run wizard.
- Slice 5 blame table + `Lens::Ownership` queries — deferred v1.2.
- Slice 6 UX bundle — command palette, sparkline column, x-axis
  zoom / pan, diff-plot side-by-side.
- Slice 7 ownership wizard.
- `Lens` / `View` / `GroupBy` enums — replaced by CLI subcommands + `--by`.
- `--exclude` flag / `default_exclude` config — `sql WHERE` covers it.
- `--module-depth` flag / `default_module_depth` — module = first path
  segment, no knob.
- Bundled DuckDB compile path — `cargo dist` prebuilt binaries instead.
- 19-grammar built-in fan-out — SPEC 7 built in (Rust, Py, JS, TS, Go,
  C, C++), rest post-v1 Cargo features.
- `unmerged_candidates` heuristic + AliasMerge modal — static
  `aliases.toml` covers it.
- `commits.tz_offset_min` column — no v1 metric consumes it.
- `file_stats.is_test` column — derive from `funcs.kind = 'test'` at
  query time.

## Possible improvements (unranked, post-v1)

Kept from prior TASKS for future reference. Not planned; capture, revisit.

### Perf
- Rayon-parallelize churn walk (per-thread `gix::open()` on same `.git/`).
- Skip `find_object` when the blob is already in the tree-sitter cache.
- Incremental Parquet writes (`INSERT INTO ext_table SELECT * FROM
  new_rows` + `COPY TO`).
- `PRAGMA memory_limit`, `PRAGMA threads` for DuckDB.
- Bench on a Linux kernel / gecko-dev / chromium subset to validate
  the DuckDB swap.

### Insights (need Phase 3 or v1.2 blame)
- PR-impact CLI: `--diff main..HEAD --output json`, GitHub Action integration.
- Bus-factor rollup (needs blame).
- Contribution heatmap (hour-of-day × day-of-week).
- Author retention, language lifecycle, deleted-code archaeology.
- Function-level delta ("biggest single-function LOC delta this month").
- Config-change correlation with churn spikes.

### Ergonomics
- `.git-archaeologist.toml` at repo root — team / layer maps distinct
  from user config.
- `git-arch prune --older-than 30d` — cache cleanup verb.
- Multi-repo `git-arch export` merge tooling.

### Distribution
- WASM build; browser demo via `isomorphic-git`.
- `git-arch serve` unix-socket daemon; editor extensions.
- Library split: `git-archaeologist-core` crate + CLI binary.

### Grammars (v1.1 re-add + new)
- Re-add (deleted in Phase 0): Java, Ruby, Bash, HTML, CSS, JSON, YAML,
  TOML, Markdown, Scala, Haskell, Zig.
- New candidates: PHP, Kotlin, Swift, Elm, Nim, Elixir, Erlang, Clojure,
  R, Julia, Solidity, GraphQL, Dart, OCaml, Lua.

### Meta
- Property tests (`proptest`) on `apply_view`: Σ Δ == final − initial.
- Fuzz the tree-sitter classifier with `cargo-fuzz`.
