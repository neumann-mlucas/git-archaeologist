# git-archaeologist — Task List

Ordered roughly by dependency. Milestone tags in `[brackets]`.

## M0 — Scaffolding
- [x] `cargo init`, commit skeleton
- [ ] Wire CI (fmt + clippy + test) — GitHub Actions
- [x] Add `README.md` stub pointing at `SPEC.md`

## M1 — Repo + cache foundations
- [x] `repo::open` — discover repo via `gix`, reject bare + detached HEAD
- [x] `cache::schema` — SQLite migrations (v1 schema from SPEC)
- [x] `cache::open` — open/create DB at `.git/git-archaeologist/cache.sqlite`
- [x] `config` — load user `config.toml` + `aliases.toml` from XDG dir
- [x] Author normalization: load `.mailmap`, merge with user aliases, persist to `authors` + `author_aliases`

## M2 — Indexer
- [x] `index::walker` — rev-list HEAD reverse, skip merges, stream `CommitInfo`
- [x] `index::bucket` — heuristic + user override → assign `bucket_key`, mark `is_sampled` (last per bucket)
- [x] `index::churn` — parse `git log --numstat` (or gix equivalent), insert rows
- [x] `index::tokei_run` — read tree blobs in-mem via `gix::ObjectDb`, feed to `tokei::Languages`
- [x] Wire full indexer: walk → churn always → tokei on sampled
- [x] Incremental reindex: skip commits already in `commits` table, redo tail bucket
- [x] Progress reporter (mpsc → UI) — channel plumbing exists; UI hookup deferred to M7 progress modal

## M3 — Query layer
- [x] `query::Filters` struct (dates, langs, authors, path, depth, group_by, view)
- [x] `query::build_series_sql` — cumulative + delta LOC per bucket per group
- [x] `query::build_breakdown_sql` — current-scope totals per group
- [x] `query::breakdown_by_module` — grouping honors current path scope + depth
- [ ] Author last-touch attribution SQL (window func over commits per path) — **deferred to v1.1**; v1 uses cumulative net churn per author as a contribution proxy (see SPEC §Attribution)

## M4 — TUI baseline
- [x] `main` — parse CLI (path arg optional, defaults to cwd), start indexer if needed
- [x] `app` — event loop, key dispatch, dirty flag → requery
- [x] `ui::layout` — five-panel split (title, filters, chart, breakdown, footer)
- [x] `ui::chart` — line chart, multi-series, stable palette by group key
- [x] `ui::breakdown` — sortable table, colored row markers matching chart
- [x] `ui::filters` — status bar rendering current filter values

## M5 — Interactivity
- [x] Group-by cycling (Tab)
- [x] Cumulative/delta toggle (d)
- [x] Path drill-down (Enter/Bksp) — scope filter, breadcrumb in title
- [x] Bucket selector modal (b)
- [ ] Date range picker modal (f) — **preset list shipped** (all / 7d / 30d / 90d / 1y); free-form date input → v1.1
- [x] Language filter modal (l) — multi-select
- [x] Author filter modal (a) — multi-select, shows raw→canonical mappings
- [x] Force reindex (r)
- [x] Help modal (?)

## M6 — Author merge UX
- [x] Detect unmerged identities (heuristic: shared email local-part OR same lowercased name)
- [x] "N unmerged identities" badge in title bar
- [x] Alias-edit modal — pick pair, `Enter` merges, writes to user `aliases.toml`
- [x] Trigger author remap without full reindex (in-place UPDATE of `author_aliases` + `commits`)

## M7 — Polish + perf
- [x] Palette configs (default / colorblind / mono) — reads `config.toml` `palette`
- [x] Progress splash during first index — consumes `Progress` mpsc, drawn to stderr before TUI takes over
- [x] Threaded reindex (`r`) — alt-screen swap + progress splash + Instant-based status msg expiry
- [x] Cache size + row count in help modal
- [x] Shallow-clone detection + warning (title-bar badge)
- [ ] Bench harness on public repo (e.g. Linux subset) — hit 2-min budget
- [x] Corrupt-cache detection at open — `PRAGMA integrity_check` + wipe-and-rebuild
- [x] Sortable breakdown table columns (`s` cycles Total → Δ → Group)
- [x] Legend / group-color key in chart panel (right-side sidebar ≥60 cols)
- [x] Lazygit-style panel chrome — rounded borders, focus-aware accent, colored titles
- [x] `M` toggles LOC ↔ churn metric (was UI dead-end)
- [x] Config-driven `default_view` / `default_group` / `default_bucket`
- [x] Delta view: use `abs(total)` for share denominator so signs don't invert percentages
- [x] Time axis: HH:MM when commit-bucket span < 24h
- [x] Modal-apply flows reset `selected_row`

## M8 — Release prep
- [x] `--version`, `--help` (via clap derive)
- [ ] `cargo dist` or manual release binary build (Linux + macOS)
- [ ] Screencast for README
- [ ] Tag v1.0.0

## v1.1 backlog
- [ ] Free-form date range picker (text input widget)
- [ ] Delta moving-average window
- [ ] CSV/JSON export current view
- [ ] Cross-branch compare
- [ ] Rename tracking behind flag
- [x] Churn-by-language series (path→lang map from latest snapshot)
- [ ] Path-picker modal using `query::subpaths`
- [ ] Stacked-area chart (youplot-style) — needs custom widget, ratatui Chart is line-only
- [ ] Bench harness on Linux kernel subset — validate 2-min budget

## v2 — architectural rewrite (sliced)

- [x] **Slice 1** Lens reframe: replace `Metric` with `Lens { Structure | Activity | Ownership }`;
      valid group_by set per lens; delete unmerged-heuristic + AliasMerge modal;
      delete cumulative-net-churn-as-author-LOC hack; delete apply_view churn-Delta special-case.
- [x] **Slice 2** Kill `git log --numstat` subprocess. Compute churn in-process via `gix` diff.
      Numstat parity verified against real git for HEAD of this repo.
- [x] **Slice 3** Drop SQLite → DuckDB embedded (bundled, no cmake needed).
      Native columnar storage; Parquet export is a one-liner (`COPY ... TO 'x.parquet'`).
      No schema-version tracking / migrations — tool is single-version,
      cache is disposable, `rm .git/git-archaeologist/*` rebuilds. MSRV
      bumped to 1.85. Trade-off: first build ~15 min (libduckdb C++ via cc);
      binary size grows meaningfully; sub-500-commit repos see no query
      speedup vs sqlite (only justified at kernel scale).
- [ ] **Slice 4** Drop tokei. Semantic LOC via tree-sitter grammars.
      - [x] Step A: tree-sitter core + Rust grammar. Line-classifier
            replicates tokei's blank / comment / code semantics with byte-range
            filtering (trailing `// comment` counts as code). Blanks + code
            match tokei exact; comment count includes `///` doc-content
            (tokei bucketizes those into a nested-Markdown sub-count).
      - [x] Step B: tokei deleted from deps + `tokei_run.rs` removed.
      - [ ] Step C: add grammars for Python, JS, TS, Go, C, C++, Java, and
            the other ~15 popular languages.
      - [ ] Step D: extend `file_stats` schema with `functions` / `types` /
            `imports` / `test_lines`, backed by per-language `.scm` queries.
      - [ ] Step E: `GroupBy::Function` / `GroupBy::NodeKind` in UI.
      - [ ] Step F: test-vector-based parity assertions.
- [ ] **Slice 5** Real Ownership lens: `git blame --incremental` cache, per-line author.
- [ ] **Slice 6** UX: command palette (`:`), `/` fuzzy filter in modals, sparkline column
      in Breakdown, interactive x-axis zoom/pan, diff-plot mode (two ranges side-by-side).
- [ ] **Slice 7** Ownership first-run wizard (replaces the deleted heuristic-auto-detect).
