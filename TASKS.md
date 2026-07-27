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
- [ ] Palette configs (default / colorblind / mono) — plumb `config.toml` `palette` field
- [ ] Progress modal during first index — consume `Progress` mpsc channel
- [ ] Cache size + row count in help modal
- [ ] Shallow-clone detection + warning
- [ ] Bench harness on public repo (e.g. Linux subset) — hit 2-min budget
- [ ] Error surfaces (missing repo, permission errors, corrupt cache → prompt to rebuild)
- [ ] Sortable breakdown table columns
- [ ] Legend / group-color key in chart panel

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
- [ ] Churn-by-language series (needs path→language mapping join)
- [ ] LOC-by-author per-bucket last-touch attribution (replace cumulative-net-churn proxy)
- [ ] Path-picker modal using `query::subpaths`
- [ ] Consume `config.toml` defaults (`default_view` / `default_group` / `palette`)
