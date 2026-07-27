# git-archaeologist — Specification (v1)

## Purpose

Interactive TUI to explore evolution of a git repository over time:
LOC, language mix, module composition, author contribution.

Answers questions like:
- "How much has the Python codebase in this monorepo grown, by author?"
- "When did the Rust portion overtake the Go portion?"
- "Which modules exploded in size in Q1 2026?"
- "Who contributed most to `api/` last quarter?"

## Scope (v1)

### In scope
- Single repo, current working directory only
- Read-only on the git repository (never mutates git state or `.mailmap`)
- Current branch (HEAD) only — no cross-branch comparison
- LOC as primary metric (from tokei), churn as secondary (from git numstat)
- Filters: date range, language, author, path (module) with drill-down
- Views: cumulative and delta (delta with moving average is v1.1)
- Group-by: language, author, module
- SQLite cache under `.git/git-archaeologist/cache.sqlite`
- Author normalization via repo `.mailmap` + user-level alias overrides

### Out of scope (v1)
- Multi-repo dashboards
- Bare repos, detached HEADs — **rejected**, tool exits with error
- Non-current branch indexing
- Rename/move tracking (`git log --follow`)
- Complexity metrics (cyclomatic etc.)
- Repo-side writes (mailmap edits, config)
- Export (CSV/JSON) — deferred to v1.1

## Metrics

### Primary — LOC (via tokei)
Per (commit, path):
- language
- code lines
- comment lines
- blank lines

Aggregations: total, by language, by author (via last-touch attribution — see §Attribution), by module.

### Secondary — Churn (via `git log --numstat`)
Per (commit, path): lines added, lines deleted.

Aggregations: by author, by language (inferred from path), by module, over time.

### Attribution model
LOC attribution to authors uses **last-touch blame-lite**:
- Each file's current LOC is credited to the author of the most recent commit touching it (within the current filter window)
- Cheap: no per-line blame, uses only commit history
- Limitation: rewrites and refactors reassign credit — documented, not fixed in v1

Churn attribution is exact (numstat is per-commit-per-author).

## Sampling (bucketing)

Snapshot LOC only at bucket boundaries. Churn is full-resolution.

Bucket sizes:

| Repo commit count | Default bucket |
|-------------------|----------------|
| < 500             | per-commit     |
| 500 – 5,000       | daily          |
| 5,000 – 50,000    | weekly         |
| > 50,000          | monthly        |

User overrides in TUI: `[auto | commit | day | week | month]`.

Sampled commit = **last commit within bucket** (represents "state at end of period").

Merge commits skipped (`--no-merges`).

## Filters

- **Date range**: `from`, `to` (inclusive), defaults to full history
- **Branch**: current HEAD only in v1 (shown, not editable)
- **Language**: multi-select from tokei-detected languages, default all
- **Author**: multi-select from normalized identities, default all
- **Path**: single path prefix, drill-down UI (see §UI)
- **Module depth**: integer 1..N — how many path segments define a "module"

All filters compose. Applied as SQL `WHERE` clauses on cache.

### Path drill-down
- Start at repo root `/`
- Table shows top-level dirs as "modules"
- Enter (or `→`) on a row descends into that dir; `Backspace` (or `←`) ascends
- Chart + breakdown update to reflect scoped path
- Depth setting controls how many segments below the current scope are grouped

## Author normalization

1. Load `.mailmap` from repo root on startup (gitoxide/git2 built-in)
2. Load user aliases from `$XDG_CONFIG_HOME/git-archaeologist/aliases.toml`
3. Merge: user aliases override mailmap
4. Compute canonical (name, email) per raw identity, dedupe
5. If unmerged near-duplicates detected (same name, different email; or vice versa), show badge "N unmerged identities" — modal offers manual merge, writes to user aliases file only

Aliases file format:
```toml
[[alias]]
canonical_name = "Jane Doe"
canonical_email = "jane@example.com"
raw = [
    { name = "jane", email = "jd@old.co" },
    { name = "Jane D.", email = "jane@example.com" },
]
```

## TUI

### Layout
```
┌ git-archaeologist ─ repo: <name> ─ branch: <HEAD> ──────────┐
│ Filters                                                     │
│   From: [YYYY-MM-DD]  To: [YYYY-MM-DD]                      │
│   Bucket: [auto▾]  Metric: [LOC▾]  View: [cumulative▾]      │
│   Group-by: [language▾]                                     │
│   Lang: [all▾]  Author: [all▾]  Path: /api/  Depth: [2]     │
├─────────────────────────────────────────────────────────────┤
│ Chart (stacked area/line, colored by group)                 │
│   legend: ● Python  ● Rust  ● Go  ● TS  ...                 │
├─────────────────────────────────────────────────────────────┤
│ Breakdown (table, sortable, colored by group)               │
│   ● Group        LOC       Δ (window)   % of scope          │
│   ● Python       12,340        +1,200        64%            │
│   ● Rust          4,200          +300        22%            │
│   ...                                                       │
├─────────────────────────────────────────────────────────────┤
│ [Tab] cycle group-by  [Enter] drill in  [Bksp] up  [q] quit │
└─────────────────────────────────────────────────────────────┘
```

Chart series colors and breakdown row colors share the same palette,
keyed by group value. Deterministic colorizer (hash of group name → palette
index) so switching group-by keeps colors stable per session.

### Keybindings

| Key         | Action                              |
|-------------|-------------------------------------|
| `q`         | quit                                |
| `Tab`       | cycle group-by (lang → author → module) |
| `Enter`/`→` | drill into selected row (path only) |
| `Bksp`/`←`  | ascend one path segment             |
| `↑`/`↓`     | move table selection                |
| `d`         | toggle cumulative / delta view      |
| `b`         | open bucket selector                |
| `f`         | open date range picker              |
| `l`         | open language filter                |
| `a`         | open author filter                  |
| `r`         | force reindex                       |
| `?`         | help modal                          |

## Data model

SQLite at `<repo>/.git/git-archaeologist/cache.sqlite`.

```sql
CREATE TABLE meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);
-- meta rows: schema_version, indexed_head_sha, indexed_at

CREATE TABLE authors (
    id              INTEGER PRIMARY KEY,
    canonical_name  TEXT NOT NULL,
    canonical_email TEXT NOT NULL,
    UNIQUE(canonical_name, canonical_email)
);

CREATE TABLE author_aliases (
    author_id INTEGER NOT NULL REFERENCES authors(id),
    raw_name  TEXT NOT NULL,
    raw_email TEXT NOT NULL,
    PRIMARY KEY (raw_name, raw_email)
);

CREATE TABLE commits (
    sha          TEXT PRIMARY KEY,
    parent_sha   TEXT,
    author_id    INTEGER NOT NULL REFERENCES authors(id),
    committed_at INTEGER NOT NULL,     -- unix seconds
    is_merge     INTEGER NOT NULL,
    is_sampled   INTEGER NOT NULL,     -- 1 if LOC snapshot taken
    bucket_key   INTEGER NOT NULL      -- YYYYMMDD or similar
);
CREATE INDEX idx_commits_ts     ON commits(committed_at);
CREATE INDEX idx_commits_bucket ON commits(bucket_key);

CREATE TABLE file_stats (
    sha      TEXT NOT NULL REFERENCES commits(sha),
    path     TEXT NOT NULL,
    language TEXT NOT NULL,
    code     INTEGER NOT NULL,
    comments INTEGER NOT NULL,
    blanks   INTEGER NOT NULL,
    PRIMARY KEY (sha, path)
);
CREATE INDEX idx_file_stats_lang ON file_stats(language);
CREATE INDEX idx_file_stats_path ON file_stats(path);

CREATE TABLE churn (
    sha     TEXT NOT NULL REFERENCES commits(sha),
    path    TEXT NOT NULL,
    added   INTEGER NOT NULL,
    deleted INTEGER NOT NULL,
    PRIMARY KEY (sha, path)
);
CREATE INDEX idx_churn_path ON churn(path);
```

## Indexing pipeline

1. Open repo (`gix::discover`); reject bare + detached HEAD
2. Load mailmap + user aliases → populate `authors` + `author_aliases`
3. Compute bucketing plan from total commit count
4. Walk `HEAD` reverse: `rev-list --reverse --no-merges`
5. For each commit:
   - Insert `commits` row
   - Parse numstat, insert `churn` rows
   - If commit is last in its bucket → `is_sampled = 1`, run tokei on in-mem tree, insert `file_stats` rows
6. Update `meta.indexed_head_sha`

**Incremental reindex**: on relaunch, walk only commits reachable from new HEAD but not in cache. Re-evaluate last-in-bucket for the tail bucket (may promote/demote).

Progress reported to TUI via mpsc channel; blocking modal shown on first index.

## Perf targets

- First-paint on cached repo: < 2s
- Filter change (no reindex): < 200ms
- Initial index (10k commits, weekly bucket ≈ 500 samples): < 2 min on mid laptop
- Cache size: < 100MB for 10k-commit polyglot repo

## Non-goals / explicit limits

- No line-level blame → author LOC attribution is last-touch approximation
- No rename tracking → moved files reset stats
- No support for shallow clones (index will be incomplete; warn on startup)
- No Windows support in v1 (Linux + macOS only)

## Config

`$XDG_CONFIG_HOME/git-archaeologist/config.toml` (all optional):
```toml
default_bucket = "auto"        # auto|commit|day|week|month
default_view   = "cumulative"  # cumulative|delta
default_group  = "language"    # language|author|module
palette        = "default"     # default|colorblind|mono
```

`aliases.toml` — see §Author normalization.

## Future (v1.1+)

- Delta moving average window (configurable N buckets)
- CSV/JSON export of current view
- Cross-branch comparison
- Complexity metrics (via tree-sitter)
- Multi-repo dashboard
- Rename tracking behind flag
