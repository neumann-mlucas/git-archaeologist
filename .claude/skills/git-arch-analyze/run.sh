#!/usr/bin/env bash
# git-arch-analyze/run.sh — dump every git-archaeologist metric for a repo
# into /tmp/gitarch-<name>-<ts>/, so the caller (a Claude skill) can read
# them and write a narrative report.
#
# Usage: run.sh [repo-path]  (default: $PWD)
#
# ponytail: no config, no flags beyond the repo path. If a real need shows
# up (custom tmp dir, subset of subcommands), promote to a flag then.

set -euo pipefail

BIN="${GIT_ARCH_BIN:-$HOME/.local/bin/git-archaeologist}"
if [[ ! -x "$BIN" ]]; then
  echo "error: git-archaeologist not found at $BIN" >&2
  echo "hint: cargo install --path <repo> --root ~/.local (or set GIT_ARCH_BIN)" >&2
  exit 1
fi

REPO="${1:-$PWD}"
REPO="$(realpath "$REPO")"

if [[ ! -d "$REPO/.git" && ! -f "$REPO/.git" ]]; then
  # gix::discover walks up; try the walk before rejecting.
  if ! git -C "$REPO" rev-parse --git-dir >/dev/null 2>&1; then
    echo "error: not a git repository: $REPO" >&2
    exit 2
  fi
fi

REPO_NAME="$(basename "$REPO")"
TS="$(date +%Y%m%d-%H%M%S)"
OUT="/tmp/gitarch-${REPO_NAME}-${TS}"
mkdir -p "$OUT"

log() { printf '[git-arch-analyze] %s\n' "$*" >&2; }

log "repo: $REPO"
log "out:  $OUT"

# Force TSV format regardless of TTY.
run() {
  local name="$1"; shift
  log "→ $name"
  if ! "$BIN" --repo "$REPO" "$@" --format tsv >"$OUT/${name}.tsv" 2>"$OUT/${name}.err"; then
    log "  (subcommand failed — see ${name}.err)"
  fi
  # Empty .err → drop it to keep the output dir tidy.
  [[ -s "$OUT/${name}.err" ]] || rm -f "$OUT/${name}.err"
}

# One implicit index run happens on the first subcommand. Prefer an
# explicit one so timing shows up clearly.
log "indexing (first run may take a while)…"
if ! "$BIN" --repo "$REPO" index >"$OUT/_index.log" 2>&1; then
  log "index failed; full log at $OUT/_index.log"
  tail -20 "$OUT/_index.log" >&2
  exit 3
fi
tail -3 "$OUT/_index.log" >&2

run burndown-lang    burndown --by language
run burndown-author  burndown --by author
run cohort           cohort
run survival         survival
# survival-fit is a separate call — --fit exp changes the output shape.
"$BIN" --repo "$REPO" survival --fit exp --format tsv \
  >"$OUT/survival-fit.tsv" 2>"$OUT/survival-fit.err" || true
[[ -s "$OUT/survival-fit.err" ]] || rm -f "$OUT/survival-fit.err"

run coupling         coupling --top 50
run classify         classify
run age              age
run churn-module     churn --by module
run churn-author     churn --by author

# hotspot needs --lang; run for each of the 7 built-in tree-sitter langs
# and skip languages with no rows.
for lang in rust python javascript typescript go c cpp; do
  out="$OUT/hotspot-${lang}.tsv"
  "$BIN" --repo "$REPO" hotspot --lang "$lang" --top 30 --format tsv \
    >"$out" 2>/dev/null || { rm -f "$out"; continue; }
  # Header-only file (< 2 lines) means no data for this lang — drop.
  if [[ "$(wc -l <"$out")" -lt 2 ]]; then
    rm -f "$out"
  fi
done

# --- summary sidecars ---

{
  "$BIN" --repo "$REPO" sql "SELECT * FROM meta" --format tsv
} >"$OUT/_meta.txt" 2>/dev/null || true

# --- SQL extractions (see SKILL.md § "SQL extractions") ---

sql_dump() {
  local name="$1"; shift
  local query="$1"
  "$BIN" --repo "$REPO" sql "$query" --format tsv \
    >"$OUT/${name}.tsv" 2>"$OUT/${name}.err" || log "  ($name query failed — see ${name}.err)"
  [[ -s "$OUT/${name}.err" ]] || rm -f "$OUT/${name}.err"
}

log "→ _authors (per-author tenure + last-touch)"
sql_dump _authors "SELECT a.canonical_name AS author,
  COUNT(*) AS commits,
  MIN(c.authored_at) AS first_ts,
  MAX(c.authored_at) AS last_ts,
  date_diff('day', to_timestamp(MIN(c.authored_at)), to_timestamp(MAX(c.authored_at))) AS active_days,
  date_diff('day', to_timestamp(MAX(c.authored_at)), current_timestamp) AS days_since_last
FROM commits c JOIN authors a ON a.id = c.author_id
WHERE NOT c.is_merge
GROUP BY a.canonical_name
ORDER BY commits DESC"

log "→ _ownership (per-file author concentration, HEAD paths only)"
sql_dump _ownership "WITH head_paths AS (
  SELECT DISTINCT path FROM file_stats
  WHERE sha = (SELECT sha FROM commits WHERE is_sampled ORDER BY authored_at DESC LIMIT 1)
), per_file AS (
  SELECT lb.path, a.canonical_name AS author, COUNT(*) AS births
  FROM line_births lb
  JOIN commits c ON c.sha = lb.birth_sha
  JOIN authors a ON a.id = c.author_id
  WHERE lb.path IN (SELECT path FROM head_paths)
  GROUP BY lb.path, a.canonical_name
), totals AS (
  SELECT path, SUM(births) AS tot FROM per_file GROUP BY path
)
SELECT p.path, p.author, p.births, ROUND(p.births * 1.0 / t.tot, 3) AS share
FROM per_file p JOIN totals t ON t.path = p.path
WHERE p.births * 1.0 / t.tot > 0.8 AND t.tot >= 50
ORDER BY t.tot DESC LIMIT 50"

log "→ _birth_death (net line churn per bucket, deletion-event detector)"
sql_dump _birth_death "WITH b AS (
  SELECT birth_bucket AS bkt, COUNT(*) AS born FROM line_births GROUP BY birth_bucket
), d AS (
  SELECT c.bucket_key AS bkt, SUM(fc.deleted) AS died
  FROM file_churn fc JOIN commits c ON c.sha = fc.sha
  GROUP BY c.bucket_key
)
SELECT COALESCE(b.bkt, d.bkt) AS bucket,
  COALESCE(b.born, 0) AS born,
  COALESCE(d.died, 0) AS died,
  COALESCE(b.born, 0) - COALESCE(d.died, 0) AS net
FROM b FULL OUTER JOIN d ON b.bkt = d.bkt
ORDER BY died DESC LIMIT 30"

log "→ _longlived (day-1 code still in HEAD)"
sql_dump _longlived "WITH head_paths AS (
  SELECT DISTINCT path FROM file_stats
  WHERE sha = (SELECT sha FROM commits WHERE is_sampled ORDER BY authored_at DESC LIMIT 1)
), first_bucket AS (
  SELECT MIN(birth_bucket) AS b FROM line_births
)
SELECT lb.path, MIN(lb.birth_bucket) AS earliest, COUNT(*) AS lines_from_that_day
FROM line_births lb, first_bucket fb
WHERE lb.path IN (SELECT path FROM head_paths)
  AND lb.birth_bucket = fb.b
GROUP BY lb.path
ORDER BY lines_from_that_day DESC LIMIT 20"

log "→ _tag_gaps (tag inter-arrival distribution)"
sql_dump _tag_gaps "WITH gaps AS (
  SELECT tagged_at, LAG(tagged_at) OVER (ORDER BY tagged_at) AS prev
  FROM tags WHERE tagged_at IS NOT NULL
)
SELECT
  CASE
    WHEN (tagged_at - prev) < 3600   THEN 'under_1h'
    WHEN (tagged_at - prev) < 86400  THEN 'under_1d'
    WHEN (tagged_at - prev) < 604800 THEN 'under_1w'
    ELSE 'over_1w'
  END AS gap_bucket,
  COUNT(*) AS n
FROM gaps WHERE prev IS NOT NULL
GROUP BY gap_bucket ORDER BY n DESC"

{
  echo "# summary — $REPO_NAME @ $TS"
  echo
  "$BIN" --repo "$REPO" sql "SELECT
    (SELECT COUNT(*) FROM commits)        AS commits,
    (SELECT COUNT(*) FROM commits WHERE is_merge)     AS merges,
    (SELECT COUNT(*) FROM tags)           AS tags,
    (SELECT COUNT(DISTINCT author_id) FROM commits) AS authors,
    (SELECT COUNT(*) FROM authors)        AS canonical_authors,
    (SELECT MIN(authored_at) FROM commits) AS first_commit_unix,
    (SELECT MAX(authored_at) FROM commits) AS last_commit_unix,
    (SELECT COUNT(*) FROM hunks)          AS hunk_rows,
    (SELECT COUNT(*) FROM line_births)    AS line_births_rows" --format table
  echo
  echo "-- languages by current LOC --"
  "$BIN" --repo "$REPO" sql "SELECT fs.language, SUM(fs.code) AS code
    FROM file_stats fs
    JOIN commits c ON c.sha = fs.sha
    WHERE c.bucket_key = (SELECT MAX(bucket_key) FROM commits WHERE is_sampled)
    GROUP BY fs.language
    ORDER BY code DESC" --format table
} >"$OUT/_summary.txt" 2>/dev/null || true

log "done → $OUT"
# Machine-readable last line — callers can grep for OUT=.
echo "OUT=$OUT"
