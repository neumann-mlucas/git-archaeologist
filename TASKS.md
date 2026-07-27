# git-archaeologist — Task List

Ordered roughly by dependency. Milestone tags in `[brackets]`.

## M0 — Scaffolding
- [ ] `cargo init`, commit skeleton
- [ ] Wire CI (fmt + clippy + test) — GitHub Actions
- [ ] Add `README.md` stub pointing at `SPEC.md`

## M1 — Repo + cache foundations
- [ ] `repo::open` — discover repo via `gix`, reject bare + detached HEAD
- [ ] `cache::schema` — SQLite migrations (v1 schema from SPEC)
- [ ] `cache::open` — open/create DB at `.git/git-archaeologist/cache.sqlite`
- [ ] `config` — load user `config.toml` + `aliases.toml` from XDG dir
- [ ] Author normalization: load `.mailmap`, merge with user aliases, persist to `authors` + `author_aliases`

## M2 — Indexer
- [ ] `index::walker` — rev-list HEAD reverse, skip merges, stream `CommitInfo`
- [ ] `index::bucket` — heuristic + user override → assign `bucket_key`, mark `is_sampled` (last per bucket)
- [ ] `index::churn` — parse `git log --numstat` (or gix equivalent), insert rows
- [ ] `index::tokei_run` — read tree blobs in-mem via `gix::ObjectDb`, feed to `tokei::Languages`
- [ ] Wire full indexer: walk → churn always → tokei on sampled
- [ ] Incremental reindex: skip commits already in `commits` table, redo tail bucket
- [ ] Progress reporter (mpsc → UI)

## M3 — Query layer
- [ ] `query::Filters` struct (dates, langs, authors, path, depth, group_by, view)
- [ ] `query::build_series_sql` — cumulative + delta LOC per bucket per group
- [ ] `query::build_breakdown_sql` — current-scope totals per group
- [ ] `query::breakdown_by_module` — grouping honors current path scope + depth
- [ ] Author last-touch attribution SQL (window func over commits per path)

## M4 — TUI baseline
- [ ] `main` — parse CLI (path arg optional, defaults to cwd), start indexer if needed
- [ ] `app` — event loop, key dispatch, dirty flag → requery
- [ ] `ui::layout` — five-panel split (title, filters, chart, breakdown, footer)
- [ ] `ui::chart` — line/area chart, multi-series, stable palette by group key
- [ ] `ui::breakdown` — sortable table, colored row markers matching chart
- [ ] `ui::filters` — status bar rendering current filter values

## M5 — Interactivity
- [ ] Group-by cycling (Tab)
- [ ] Cumulative/delta toggle (d)
- [ ] Path drill-down (Enter/Bksp) — scope filter, breadcrumb in title
- [ ] Bucket selector modal (b)
- [ ] Date range picker modal (f)
- [ ] Language filter modal (l) — multi-select
- [ ] Author filter modal (a) — multi-select, shows raw→canonical mappings
- [ ] Force reindex (r)
- [ ] Help modal (?)

## M6 — Author merge UX
- [ ] Detect unmerged identities (heuristic: shared name-prefix or email local-part)
- [ ] "N unmerged identities" badge in title bar
- [ ] Alias-edit modal — pick group, assign canonical, write to user `aliases.toml`
- [ ] Trigger author remap without full reindex

## M7 — Polish + perf
- [ ] Palette configs (default / colorblind / mono)
- [ ] Progress modal during first index
- [ ] Cache size + row count in help modal
- [ ] Shallow-clone detection + warning
- [ ] Bench harness on public repo (e.g. Linux subset) — hit 2-min budget
- [ ] Error surfaces (missing repo, permission errors, corrupt cache → prompt to rebuild)

## M8 — Release prep
- [ ] `--version`, `--help`
- [ ] `cargo dist` or manual release binary build (Linux + macOS)
- [ ] Screencast for README
- [ ] Tag v1.0.0

## v1.1 backlog
- [ ] Delta moving-average window
- [ ] CSV/JSON export current view
- [ ] Cross-branch compare
- [ ] Rename tracking behind flag
