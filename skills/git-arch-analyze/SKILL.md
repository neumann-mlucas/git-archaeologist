---
name: git-arch-analyze
description: >
  Run git-archaeologist on a repo and write a narrative markdown report
  interpreting the numbers. Not just "here are the tables" — the report
  reads as an engineering analysis: what's healthy, what's smelly, where
  the bodies are buried. Works on any git repo: OSS health, monorepo
  subsystem drift, team dynamics, AI-era code shifts, composition trends
  (test/config/code balance). Trigger: `/git-arch-analyze [path]`,
  "analyze this repo with git-archaeologist", "run archaeologist +
  interpret", "git-arch report", "monorepo forensics", "AI-impact scan".
---

# git-arch-analyze

Two-step workflow:

1. **Collect** — invoke this skill's `run.sh <repo>` (default: current working directory). Script indexes the repo, runs every subcommand, dumps `.tsv` files into `/tmp/gitarch-<repo>-<ts>/`. Prints the output directory path on its final line.
2. **Interpret** — read the dumped TSVs, then write a markdown report to `<repo>/ARCHAEOLOGY.md` (unless the user specifies otherwise). The report is the point of the skill — do not just paste tables.

## How to invoke

Skill files live at `skills/git-arch-analyze/` in the git-archaeologist
repo; the `~/.claude/skills/git-arch-analyze` entry is a symlink into
that. Either path works:

```bash
bash ~/.claude/skills/git-arch-analyze/run.sh [repo-path]
# or, from the git-archaeologist repo:
bash skills/git-arch-analyze/run.sh [repo-path]
```

If no path given, the script uses `$PWD`. First run indexes; subsequent runs are cached and re-emit in a few seconds.

## Reading the output

The script writes these files under `/tmp/gitarch-<name>-<ts>/`:

| File                      | What it holds                                                                                                                                                                                                                                                                                                                      |
| ------------------------- | ---------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `burndown-lang.tsv`       | cumulative LOC per bucket, per language                                                                                                                                                                                                                                                                                            |
| `burndown-author.tsv`     | running net-contribution per author per bucket                                                                                                                                                                                                                                                                                     |
| `cohort.tsv`              | alive lines per (bucket, birth_bucket) — the theseus stack                                                                                                                                                                                                                                                                         |
| `survival.tsv`            | births / alive / dead / survival ratio per cohort                                                                                                                                                                                                                                                                                  |
| `survival-fit.tsv`        | half-life scalar via exponential fit (or `reason` if <100 deaths)                                                                                                                                                                                                                                                                  |
| `coupling.tsv`            | top-50 file pairs by co-commit                                                                                                                                                                                                                                                                                                     |
| `classify.tsv`            | commits + fix/feat/revert/breaking/untyped per bucket                                                                                                                                                                                                                                                                              |
| `hotspot-<lang>.tsv`      | top-300 (bucket, func) rows by churn, sorted desc (head = top hotspots; the depth feeds the trajectory check), one file per tree-sitter lang present (rust, python, javascript, typescript, tsx, go, c, cpp, lua, vim-script, java, ruby, bash, php, ocaml, scala, haskell, kotlin, csharp, swift); langs with no data are dropped |
| `age.tsv`                 | file age histogram, 7-day bins — columns: `age_days_bucket`, `files`. Bucket into 5 tiers for the report (0-90d / 90d-1yr / 1-2yr / 2-5yr / 5yr+)                                                                                                                                                                                  |
| `churn-module.tsv`        | added/deleted per top-level dir per bucket (`churn --by module --depth 1`)                                                                                                                                                                                                                                                         |
| `churn-module-d2.tsv`     | same at second-level path granularity (`churn --by module --depth 2`) — reach for this when `src`, `runtime`, `test` are the top rows and hide where the churn actually lives                                                                                                                                                      |
| `churn-author.tsv`        | added/deleted per author per bucket                                                                                                                                                                                                                                                                                                |
| `burndown-kind.tsv`       | cumulative LOC per bucket split `code` vs `test` (static test detection: test dirs/file conventions + `#[cfg(test)]`, `def test_*`, `describe(...)`, etc.)                                                                                                                                                                         |
| `stats-lang.tsv`          | HEAD snapshot per language: files, lines, code, comments, blanks, `test_code`, `test_pct`, funcs, `max_func`, `median_age_days`, `owner`, `owner_pct`, `churn_90d`; last row `Total`. `other` = files with no known language (lines only)                                                                                          |
| `stats-dir.tsv`           | same, per top-level dir (`.` = root files). Fastest per-subtree view of test share, age and owner                                                                                                                                                                                                                                  |
| `stats-since-tag.tsv`     | per-language delta from the newest tag before HEAD (`since_tag` in `_meta.txt`) to HEAD; `test_pct` in percentage points. Absent on untagged repos. Grammar-less files are not counted                                                                                                                                             |
| `_meta.txt`               | index metadata (`schema_version`, `bucket_size`, `indexed_head_sha`, `since_tag`)                                                                                                                                                                                                                                                  |
| `_summary.txt`            | quick totals (commits, merges, tags, authors) + the `stats` table at HEAD                                                                                                                                                                                                                                                          |
| `_authors.tsv`            | per-author tenure: commits, first/last commit ts, active_days, days_since_last                                                                                                                                                                                                                                                     |
| `_ownership.tsv`          | files where one author wrote >80% of the lines (HEAD paths only)                                                                                                                                                                                                                                                                   |
| `_birth_death.tsv`        | born vs died lines per bucket, sorted by died — auto-detects big deletion events                                                                                                                                                                                                                                                   |
| `_longlived.tsv`          | day-1 files still in HEAD, with line counts from the first bucket                                                                                                                                                                                                                                                                  |
| `_tag_gaps.tsv`           | tag inter-arrival distribution (under_1h / under_1d / under_1w / over_1w)                                                                                                                                                                                                                                                          |
| `_ai_trailers.tsv`        | per-bucket AI Co-Authored-By count (Claude/Copilot/GPT/Codex/Cursor/Aider). Empty = no AI co-authorship in repo                                                                                                                                                                                                                    |
| `_off_hours.tsv`          | per-author `off_hours_ratio_utc` (weekend or outside 09-18 UTC) + `weekend_ratio_utc`. Min 20 commits. Timezones lost — treat as UTC                                                                                                                                                                                               |
| `_comment_density.tsv`    | per-(bucket, language) `code`, `comments`, `blanks`, `comment_ratio = comments/(code+comments)`                                                                                                                                                                                                                                    |
| `_subtree_ownership.tsv`  | per top-level dir: `head_files`, `single_owner_files` (>80% share), `avg_top_share`, `total_line_births`. Subtree bus-factor rollup                                                                                                                                                                                                |
| `_cross_module_pairs.tsv` | file pairs from different top-level dirs, co-committed ≥3× in commits touching <50 files. Leaky-boundary detector                                                                                                                                                                                                                  |
| `_func_longevity.tsv`     | per (path, name): `first_bucket`, `last_bucket`, `snapshots`, `rewrites` (distinct `funcs.body_hash` count), `min_len`/`max_len`, `alive_at_head`. Top 200 by rewrite count. Cursed-core / fossil-API detector                                                                                                                     |

Read each with `Read` — TSVs, so first line is headers. For big tables (>200 rows), read the head + tail rather than the whole file.

## Writing the report

Target: `ARCHAEOLOGY.md` at the repo root. Structure below. **Iron rule: every section ends with a "so what" line — one sentence stating the finding a reader should walk away with. If you can't write one, delete the section.**

````markdown
# {repo name} — code archaeology

_Generated by git-archaeologist on {YYYY-MM-DD}. Indexed HEAD: {short-sha}._

## TL;DR

- 3–5 bullets. Include repo shape one-liner (commits, authors, window, primary lang) as first bullet — no separate "Repo shape" section.
- Auto-flag any of these when present: bus factor ≤2 (>60% net-line share by #1); cohort year with survival <30% AND ≥500 lines born; hot function with commit in last 30 days; last-90d contributor count ≥2× rolling 12-mo median.

## Cadence

- Sparkline commits/month from `classify.tsv`. Answers "alive, bursty, or dead" in one glyph row.
- **Flag dead periods**: any consecutive ≥30-day span with zero commits. Name start/end dates. Don't guess causes (illness, job change, burnout) — just report the gap.
- **Trailing dormancy**: compute `today - last_commit_ts` from `_summary.txt`. If >90 days, this is the headline — put it in TL;DR too.
- **Merge ratio** one-liner: `merges / commits` from `_summary.txt`. <5% = linear-history solo shop; >20% = PR-gated. Cite the number and the regime.
- **Tag rhythm** one-liner: `tags / months_active`. Use `_tag_gaps.tsv` to distinguish human-release cadence from automation — if `under_1h` + `under_1d` buckets dominate, tags are bot-driven; note it explicitly instead of quoting raw tags/month.
- **Deletion events**: from `_birth_death.tsv` top rows, name any bucket where `died` > 5× the median. Usually a rewrite, package removal, or vendored-code purge. Cross-reference `git log <bucket-date>` if the commit message helps.
- **Spike diagnosis is mandatory.** For every month over 2.5× the median commit count, `git log --oneline --since=YYYY-MM-01 --before=YYYY-MM+1-01 | head -30` and name the driver in one clause (release-push, refactor sprint, vendored-code drop, mass rebase, or "no single driver — broad activity"). Never leave a spike unlabeled in the sparkline caption.
- **File-age histogram from `age.tsv`.** One horizontal bar chart bucketed into ~5 age tiers (0-90d, 90d-1yr, 1-2yr, 2-5yr, 5yr+) showing what fraction of HEAD files land in each. Reveals whether the working tree is mostly recent code, or a fossil layer with a thin recent skin. Recipe:
  ```bash
  python3 -c '
  from collections import defaultdict
  tiers = [(90,"0-90d"),(365,"90d-1yr"),(730,"1-2yr"),(1825,"2-5yr"),(10**9,"5yr+")]
  buckets = defaultdict(int)
  for l in open("age.tsv").readlines()[1:]:
      p = l.rstrip().split("\t")
      # age.tsv columns: bin_days_old, files (or similar — inspect header)
      d = int(p[0]); n = int(p[-1])
      for lim, name in tiers:
          if d < lim: buckets[name] += n; break
  m = max(buckets.values()) or 1
  for _, name in tiers:
      v = buckets[name]
      print(f"{name:<10} {chr(9608)*int(v/m*30):<30} {v}")'
  ```

Read the header of `age.tsv` first — column names vary by schema version.

- So-what: name the regime (steady / bursty / declining / dead) with the last-90d rate vs full-history median.

## Growth

- **Total LOC over time** (sum `burndown-lang.tsv` across languages per bucket). Sparkline. Answers "is the codebase growing, plateauing, or shrinking?" Cite start-LOC, end-LOC, and CAGR across the window.
- **Net LOC/month** from `_birth_death.tsv` — mean of `net` column, plus recent-90d mean. If recent is <20% of historical, growth has stalled; if >2×, expansion phase.
- **Contributor-count trajectory** — unique authors per year from `churn-author.tsv` (bot-filtered). Sparkline or short table. Growing/flat/shrinking community is a leading indicator that commit-rate doesn't catch (a stable commit rate carried by fewer people = concentrating risk).
- **Regime label**: `expanding` (LOC + contribs both up), `maturing` (LOC up, contribs flat), `consolidating` (LOC flat/down, contribs steady — rewrites/refactors dominate), `declining` (both down), `frozen` (both near zero).
- Recipe:
  ```bash
  # total LOC per bucket
  python3 -c '
  from collections import defaultdict
  tot = defaultdict(int)
  for l in open("burndown-lang.tsv").readlines()[1:]:
      b, _, loc = l.rstrip().split("\t")
      tot[b] += int(loc)
  xs = [tot[b] for b in sorted(tot)]
  s = "▁▂▃▄▅▆▇█"
  lo, hi = min(xs), max(xs); r = (hi-lo) or 1
  print("total LOC:", "".join(s[min(7,int((x-lo)/r*7))] for x in xs))
  print(f"start={xs[0]}  end={xs[-1]}  peak={hi}  net_delta={xs[-1]-xs[0]:+}")'

  # unique contributors per year, bot-filtered
  python3 -c '
  import re
  from collections import defaultdict
  bot = re.compile(r"(bot|dependabot|renovate|runner|-ci|weave|flux|action|noreply)", re.I)
  by_yr = defaultdict(set)
  for l in open("churn-author.tsv").readlines()[1:]:
      b, a, *_ = l.rstrip().split("\t")
      if bot.search(a): continue
      by_yr[b[:4]].add(a)
  for y in sorted(by_yr): print(f"{y}\t{len(by_yr[y])}")'
  ```
- **Since last release** — `stats-since-tag.tsv` (when present): per-language code and test-code delta since `since_tag`. One line: "since v0.24: +1.2k Rust, +400 test lines, test share +2pp".
- So-what: name the regime + one sentence on whether growth is coming from more people or the same people writing more.

## Cohort survival

- Interpret `cohort.tsv` + `survival.tsv`. Quote half-life from `survival-fit.tsv` **only if** `reason` is `ok` AND the value is less than 10× the repo's total age in days — the exponential fit blows up when a recent giant cohort hasn't had time to decay. Otherwise ignore the number and just describe the yearly table.
- Yearly-survival bar chart. Call out outliers (fast decay or unusual persistence).
- **Decay-shape callout from `cohort.tsv`** — pick 2-3 birth-buckets (oldest, one mid-life, most recent stable) and describe the shape of the decay: "steep first year then flat" (initial-scaffolding decay), "linear grind" (steady rewrite pressure), "cliff at bucket X" (a single rewrite killed the cohort). One sentence; don't paste the curve. Use this recipe:
  ```bash
  # b = a cohort key copied from survival.tsv (same format as bucket_size)
  awk -F'\t' -v b=20161009 '$2==b {print $1"\t"$3}' cohort.tsv | tail -30
  ```
- **Caveat when repo is dormant** (see Cadence): a latest-year cohort at >85% survival is a freeze artifact, not durability. Say so.
- Add a one-line "day-1 survivors" callout from `_longlived.tsv` — the top file(s) with the most first-bucket lines still in HEAD are either the stable core or the ossified boilerplate; name them.
- So-what: does this repo keep code or churn it?

## Language mix

- **Skip section entirely** if the repo is single-language AND no language ever rose or fell past another. One line under TL;DR instead.
- Include full section if `burndown-lang.tsv` shows ≥2 languages AND any pair crossed (one overtook another in LOC) OR any language grew/shrank >2× over the window. A "crossover" is a real story — name it (which lang rose, when, at whose expense). Bar chart of current LOC + one-line trend per lang.
- Recipe:
  ```bash
  python3 -c '
  from collections import defaultdict
  by_bucket = defaultdict(dict)
  for l in open("burndown-lang.tsv").readlines()[1:]:
      b, lang, loc = l.rstrip().split("\t")
      by_bucket[b][lang] = int(loc)
  buckets = sorted(by_bucket)
  langs = sorted({l for v in by_bucket.values() for l in v}, key=lambda x:-by_bucket[buckets[-1]].get(x,0))[:5]
  print("bucket\t"+"\t".join(langs))
  for b in buckets[::max(1,len(buckets)//15)]:
      print(b+"\t"+"\t".join(str(by_bucket[b].get(l,0)) for l in langs))'
  ```
  Look for crossover rows; that's your headline.
- **Current mix and test share** — `stats-lang.tsv` at HEAD. Quote `test_pct` for the main language(s); an `other` row with many lines usually means lockfiles, data or vendored assets, not code.

## Contributors & bus factor

- `burndown-author.tsv` + `churn-author.tsv`. Net-lines bar chart, top 10. **Peak net-LOC is a proxy** — an author whose code was fully deleted still shows up big. Cross-check with `_ownership.tsv` (files where a single author wrote >80%) for who owns the live code today, not who once wrote code.
- **Author timelines from `burndown-author.tsv`** — for the top 3-5 contributors by net LOC, describe the shape over time: "linear climb", "peak-then-flat" (built something, moved on), "peak-then-decay" (their code got rewritten by others), "late arrival" (joined recently and rising). Do NOT just quote the final value. This is where you catch founder-scaffolding traps and rising owners the tenure table alone misses. Recipe:
  ```bash
  python3 -c '
  from collections import defaultdict
  ts = defaultdict(list)
  for l in open("burndown-author.tsv").readlines()[1:]:
      b, a, loc = l.rstrip().split("\t")
      ts[a].append((b, int(loc)))
  # top 5 by peak
  top = sorted(ts.items(), key=lambda x:-max(v for _,v in x[1]))[:5]
  s = "▁▂▃▄▅▆▇█"
  for a, series in top:
      xs = [v for _,v in series]
      lo, hi = min(xs), max(xs); r = (hi-lo) or 1
      spark = "".join(s[min(7,int((x-lo)/r*7))] for x in xs)
      print(f"{a:<25} peak={hi:>7} final={xs[-1]:>7} {spark}")'
  ```
  Spark shape tells the story: `▁▂▃▅▇█` = still climbing; `▁▄▇█▇▆` = peak-then-decay; `▁▇███████` = built-then-flat.
- **Tenure table from `_authors.tsv`**: for the top 10 by commits, show `commits, first, last, days_since_last`. This surfaces departed contributors (`days_since_last > 180`) without external HR data — often explains dormancy or silo formation directly.
- **Bot-filter verification, not assertion.** Before claiming "no bots in top 20," grep for it:
  ```bash
  grep -Ei '(bot|dependabot|renovate|runner|-ci|weave|flux|action|noreply)' _authors.tsv | head
  ```
  Cite the count found (e.g. "3 bot identities present but all outside top 20") — never claim clean without running the check.
- **Per-file concentration from `_ownership.tsv`** replaces the old "run git blame on top hotspots" step. Show top 5–10 rows with `share > 0.8`. Skip generated code (`*.pb.go`, `*_gen.py`, etc.) — high concentration there is trivial.
- **Per-dir owner at HEAD** — `stats-dir.tsv` `owner` + `owner_pct` (share of surviving lines). Quick bus-factor read per subtree before reaching for `_subtree_ownership.tsv`.
- If a large contributor's net went flat OR their `days_since_last` is >90d, name them + when.
- So-what: name the bus factor and the person(s) it depends on. If several top authors have high `days_since_last`, connect that to any dormancy flagged in Cadence.

## Change type mix

- **Skip section entirely** if <30% of commits are typed (Conventional Commits). Replace with one line: "N reverts / M commits in span; prefix convention unused" — done.
- Include full section only if typing rate justifies it (fix/feat/revert rates, spike windows).

## Coupling & hotspots

- Top 5–10 pairs from `coupling.tsv`. **For the top cross-module pair (not module↔test), run `git log --oneline -- <fileA> <fileB> | head -5` and cite the resulting commits — or state "no ripple trail" honestly.** No hand-waving "these are coupled."
- Function-level hotspots from `hotspot-<lang>.tsv` if non-empty, top 5.
- **Trajectory check is mandatory, not optional.** For each top-3 hotspot function that has ≥2 touches in the file, compute whether it's cooling. Recipe:
  ```bash
  awk -F'\t' '{print $2"::"$3"\t"$1"\t"$8}' hotspot-c.tsv | tail -n+2 | \
    python3 -c '
  import sys
  from collections import defaultdict
  t = defaultdict(list)
  for l in sys.stdin:
      k, b, c = l.rstrip().split("\t")
      t[k].append((b, int(c)))
  for k, rows in sorted(t.items(), key=lambda x:-sum(c for _,c in x[1]))[:5]:
      rows.sort()
      if len(rows) < 2:
          print(f"{k}\tSINGLE_SHOT churn={rows[0][1]}"); continue
      h = len(rows)//2 or 1
      a, b = sum(c for _,c in rows[:h])/h, sum(c for _,c in rows[h:])/max(1,len(rows)-h)
      verdict = "cooling" if b < a*0.7 else ("worse" if b > a*1.3 else "flat")
      print(f"{k}\ttouches={len(rows)}\t{verdict}\tfirst_half_avg={a:.0f}\tsecond_half_avg={b:.0f}")'
  ```
  A single-shot big-churn row usually means a one-time rewrite/delete, not a hotspot — say so. Repeated appearances of the _same_ function across buckets = real chronic hotspot.
- So-what: is architecture clean, or is there a god-file / hidden dependency?

## Module churn

- `churn-module.tsv` for the coarse regime: bar chart top 8 top-level dirs. Note dead dirs (no recent churn) and migrated ones (added ≈ deleted).
- **When top-level dirs are too broad to be informative** (e.g. neovim: `src`, `runtime`, `test` each dominate but say nothing), switch to `churn-module-d2.tsv`. Second-level paths (`src/nvim`, `test/functional`, `runtime/lua`) show which subsystem is actually moving. Include a milestone-year table for the top-4 depth-2 modules (net = added − deleted per year, plus lifetime churn = added + deleted). If d2 rows still look thin, run `git-archaeologist churn --by module --depth 3` ad-hoc.
- Balance check: for each top-4 module, if `|net| < 5% of churn` call it "balanced — ongoing maintenance, not growth or removal". If `net ≈ +churn` it's greenfield; if `net ≈ -churn` it's a removal wave.
- So-what: where is engineering effort going, and where has it stopped?

## Risk flags

- Cross-reference findings: silo (churn + author concentration), instability (recent reverts/fixes spike), architecture debt (unexpected coupling with ripple evidence).
- Each flag: one line, cite the section it came from.

## Follow-ups

- **Max 3 items.** Each MUST reference an unresolved finding from a section above by name. No speculative queries.
- Example: "Contributors section says 9 new committers in 30d — check merge latency to test whether backlog is growing" — ties to a real section.
````

## Optional lenses

Beyond the core sections above. Each lens has a trigger condition — include when repo shape justifies it, otherwise skip. All lenses use existing dumps + raw `sql` or `git log`; no re-index needed. Same "so what" discipline applies.

### Monorepo subtree health

Trigger: ≥3 top-level dirs each holding ≥5% of HEAD LOC (or ≥3 subtrees in `_subtree_ownership.tsv` with `head_files ≥ 5`).

- **Per-subtree bus factor** — `_subtree_ownership.tsv` directly. Any subtree with `single_owner_files / head_files > 0.5` = silo risk.
- **Cross-subtree coupling** — `_cross_module_pairs.tsv`. Name the top pair; run `git log --oneline -- <fileA> <fileB> | head -5` for the ripple trail (or state "no ripple trail" if no informative commits).
- **Per-subtree cadence** — commits/month per top-level dir from `churn-module.tsv`. Dormant subtree next to a hot one = ossifying subsystem, not "the whole repo is healthy".
- So-what: which subtree carries silo risk; which boundary leaks.

### AI-era shift

Trigger: `_ai_trailers.tsv` non-empty AND ≥10 AI-tagged commits total. Or repo spans across 2023-01-01 even without trailers (adoption can be silent).

- **AI adoption curve (trailer path)** — `_ai_trailers.tsv` cumulative `ai_commits` per bucket. First bucket with non-zero rows = adoption start. Ratio of AI-tagged commits to total commits in last 90d = current mix.
- **AI mention path** — some commits mention AI in the message body without a trailer. `commits.message` is in the schema — union with trailers for the full picture:
  ```bash
  git-archaeologist --repo "$REPO" sql "SELECT bucket_key, COUNT(*) AS mentions
    FROM commits WHERE regexp_matches(lower(message), '(claude|copilot|chatgpt|cursor|codex|aider)')
    GROUP BY bucket_key ORDER BY bucket_key" --format tsv
  ```
- **Cohort survival split** — post-adoption birth-buckets from `cohort.tsv` / `survival.tsv` at matched age vs pre-adoption cohorts. Faster decay = AI output revised more (correlation, not causation).
- **Comment-density shift** — `_comment_density.tsv` `comment_ratio` per language across the adoption boundary. AI often inflates comment density; call the delta.
- **Commit-size shift** — median LOC/commit pre vs post-adoption via `sql` over `file_churn`. Rising size + rising reverts = "AI drafts, humans miss bugs".
- **Function rewrite bursts** — `_func_longevity.tsv` `rewrites` per function post-adoption. Cursed-core hotspots that keep getting AI-rewritten point at churn-prone areas.
- So-what: is AI adoption visible in the churn/comment/survival curves? Name the bucket it started, name what changed — or "adoption present but no measurable shift" if flat.

### Human patterns

Trigger: `_off_hours.tsv` has ≥5 rows (dump filters authors with <20 commits).

- **Off-hours ratio** — `_off_hours.tsv` `off_hours_ratio_utc`. Sustained >30% for one author = contractor pattern, timezone-shifted teammate, or firefighting. Do NOT diagnose which — cite the number and let the reader decide.
- **Timezone caveat** — `authored_at` is normalized without tz preservation in this schema; ratios are UTC-based. A team commuting during Asia business hours will look "off-hours" from a UTC lens. State this in the section header when the top author is >50%.
- **Weekend ratio** separately from `weekend_ratio_utc` — weekend-heavy but 9-18 within = "works weekends but daytime" (side project); weekend-light but off-hours = "shifts start early/end late" (different tz).
- **Tenure distribution** — from `_authors.tsv`, histogram of `active_days` per author (bucket <30 / 30-180 / 180d-1y / 1-3y / 3y+). Fat short-tenure bucket = drive-by contribs; fat long-tail = stable core.
- So-what: name any author >40% off-hours; name the team's working-hours regime; connect dormant-author flags from Contributors section.

### Function longevity

Trigger: `_func_longevity.tsv` non-empty (tree-sitter parsed at least one snapshot).

- **Cursed core** — top rows by `rewrites` where `alive_at_head=true` AND `snapshots >= 10`. Same function, many bodies, still shipping. Name 3-5. These are load-bearing legacy where every change lands.
- **Rewrite treadmill** — `rewrites / snapshots > 0.3` = body cycled every ~3 snapshots. Design instability.
- **Fossil API** — `alive_at_head=true` AND `rewrites <= 1` AND `snapshots > 20`. Written once, never touched, still exported. Either stable primitive or dead public surface.
- **Fast burners** — `alive_at_head=false` AND `snapshots >= 5` AND `rewrites >= 3`. Born, churned, killed. Design failures worth naming (what was tried, what replaced it).
- **Bloat drift** — `max_len / min_len > 3`. Function grew unbounded. Refactor candidates.
- Cross-reference the top-3 cursed-core functions against `hotspot-<lang>.tsv` — they should agree. If a function is high-churn in `_func_longevity` but absent from hotspot table, the file itself is churn-heavy while the function is stable (surrounding code moves around it).
- So-what: name the top cursed-core function; name any rewrite-treadmill instability; name a dead fast-burner if the story explains a design pivot.

### Composition trend

Trigger: always safe. Especially load-bearing for repos where "is this well-tested" or "is this a config platform" matters.

- **Comment density per language over time** — `_comment_density.tsv`. Trend `comment_ratio` per top-3 languages across buckets. Rising = doc culture / AI inflation; falling = code outpacing docs. Compare across languages: Rust/Go idiomatic doc-comments should sit higher than JS/Python inline habits.
- **Test share per top-level dir** — `stats-dir.tsv` `test_code` / `test_pct`. Counts inline tests too (`#[cfg(test)]`, `def test_*`, `describe(...)`), not just test files. Any prod dir with `test_pct = 0` and substantial code = red flag.
- **Test share over time** — `burndown-kind.tsv`: `test / (code + test)` per milestone year. Rising = test culture improving; falling = code outpacing tests.
- **Config-vs-code ratio** — sum `_comment_density.tsv` `code` where `language IN ('YAML','JSON','TOML','HCL','Dockerfile')` vs everything else. Rising config share past 30% = platform drift (k8s/terraform/pipelines eating the codebase).
- **Generated-code split** — flag `vendor/`, `node_modules/`, `dist/`, `*.pb.go`, `*_gen.*`, `*_pb2.py`. Report handwritten LOC separately in TL;DR when generated exceeds 20% of HEAD LOC. Cleans every other signal that treats machine output as human work.
- So-what: any prod dir with zero tests; any repo where config >30% of LOC (name it as platform, not app); comment-ratio trend direction per language.

## SQL extractions (design notes)

The `sql` subcommand hits the DuckDB index directly. `run.sh` dumps eleven `_*.tsv` files that the built-in subcommands don't cover; use them per the section instructions above. Key tables (schema v7; check `_meta.txt`):

- `commits` — sha, author_id, committer_id, authored_at, is_merge, is_sampled, bucket_key, msg_type, is_breaking, is_revert, ignored_blame, message
- `commit_parents` — sha, parent_sha, parent_idx (0 = first parent)
- `commit_trailers` — sha, author_id, role (lowercased, e.g. `coauthor`)
- `authors` — id, canonical_name, canonical_email
- `author_aliases` — author_id, raw_name, raw_email
- `line_births` — path, line_no, birth_sha, birth_bucket: one row per line alive at HEAD, so `DISTINCT path` = HEAD's file set (no death_sha — deaths are derived from `file_churn.deleted` per bucket)
- `file_churn` — sha, path, added, deleted, language
- `file_stats` — sha, path, language, code, comments, blanks, test_code (snapshot per sampled commit; one sampled commit per bucket)
- `hunks` — sha, path, prev_path, old_start/len, new_start/len (rename hops here)
- `tags` — name, sha, tagged_at
- `funcs` — sha, path, name, kind, start_line, end_line, body_hash
- `meta` — key, value (`schema_version`, `bucket_size`, `indexed_head_sha`)

Gotchas:

- Timestamp math: `authored_at` is UNIX epoch (BIGINT). Wrap with `to_timestamp(...)` before `date_diff` / `current_timestamp`, or the parser mis-binds to `age(BIGINT, BIGINT)` and errors out.
- HEAD-alive filtering: for files, use `SELECT DISTINCT path FROM line_births`. For a HEAD `file_stats` / `funcs` snapshot, walk HEAD's first-parent chain from `meta.indexed_head_sha` and take the newest sampled commit (the `$HEAD_SNAP` CTE in `run.sh`). Do **not** use "newest sampled commit": every ref is walked, so it can be on a branch ahead of HEAD. For current totals prefer `stats-lang.tsv`, which is exact.
- No death→author link. `line_births` has no `death_sha`. Deletion attribution ("who deleted whose code") would need custom fold logic — not available via SQL. Don't promise it in reports.

If you want an ad-hoc extraction not covered by the dumps, run `git-archaeologist --repo <repo> sql "<query>" --format tsv` directly.

## Style rules

- Interpret, don't dump. If you paste a 200-row table, you failed. Pull
  representative slices (top 5, notable outliers) into inline tables and
  reference the raw TSV in `/tmp/gitarch-...` for the full data.
- Cite the number. "The Rust cohort born in 2023 has ~40% surviving"
  beats "cohort survival is decent."
- Flag uncertainty. If cohort is dominated by a `line_births` heuristic
  (rename beyond one hop drops the origin), say so.
- Never invent. If a table is empty (e.g., `hotspot-rust.tsv` on a Python
  repo), say why and skip the section — don't hallucinate hotspots.

## ASCII plots

**Preferred: milestone-year tables.** Pick 4-7 years across the repo's
span (e.g. 2014, 2016, 2018, 2020, 2022, 2024, 2026) and print one
column per group. Renders identically in every markdown viewer, holds
exact numbers, no glyphs to break on mobile or in non-monospace
contexts.

```bash
# Milestone-year snapshot from any bucket-keyed TSV
python3 -c '
from collections import defaultdict
years = ["2014","2016","2018","2020","2022","2024","2026"]
by_bkt = defaultdict(dict)
for l in open("burndown-lang.tsv").readlines()[1:]:
    b, k, v = l.rstrip().split("\t")
    by_bkt[int(b)][k] = int(v)
last_of = {}
for b in sorted(by_bkt): last_of[str(b)[:4]] = b
top = sorted(by_bkt[max(by_bkt)], key=lambda k:-by_bkt[max(by_bkt)][k])[:5]
print("year".ljust(6) + "".join(f"{k:>14}" for k in top))
for y in years:
    if y not in last_of: continue
    r = by_bkt[last_of[y]]
    print(f"{y:<6}" + "".join(f"{r.get(k,0):>14,}" for k in top))'
```

**Horizontal bars** (ranked, no time axis — cohort survival by year,
top-N authors, age tiers):

```bash
python3 -c 'import sys; rows=[l.rstrip().split("\t") for l in sys.stdin if l.strip()]; m=max(float(r[-1]) for r in rows); [print(f"{r[0]:<18} {chr(9608)*int(float(r[-1])/m*40):<40} {r[-1]}") for r in rows]' < top.tsv
```

**Sparklines** are a fallback only — when a milestone table would
be too sparse (< 4 milestone hits) or the story is a shape that only
survives at high resolution (dead-period detector). Braille glyphs
render inconsistently in editors and mobile viewers, so default to
the table.

Rules for plots in the report:

- Wrap in `` ```text `` fences so tables stay left-aligned.
- One plot per section max. Charts are a caption for the number, not
  the headline.
- Skip when N < 4 milestone rows — a table with two rows says nothing
  the sentence beside it couldn't.
- Always precede with the exact command that produced them (so the
  user can re-run against updated data).
- The plot-worthy set: cohort survival by year (bars), module churn
  net + magnitude per year at depth 1 AND depth 2 (milestone table),
  contributor net-lines top-10 (bars), LOC-burndown per top-5 lang
  (milestone table), file-age histogram (bars), commit cadence when
  spikes exist (milestone table of commits/year).

## Reach for `sql`

The canned dumps cover 90% of questions. Anything else (function
authored by whom, files added by cohort X still on disk, revert
patterns per module, etc.) goes through the raw `sql` subcommand:

```bash
git-archaeologist --repo "$REPO" sql "<query>" --format tsv
```

Schema is documented at the end of this file (§SQL extractions). Do
not paste made-up SQL — verify column names against the schema
section first, then run.

## Commit cadence (how to compute)

`classify.tsv` first column is bucket key. **Check `_meta.txt` for `bucket_size`** before slicing — `Day` = `YYYYMMDD`, `Week` = ISO week, `Month` = `YYYYMM`. For daily buckets:

```bash
awk -F'\t' 'NR>1 {m=substr($1,1,6); c[m]+=$2} END {for(k in c) print k"\t"c[k]}' classify.tsv | sort > /tmp/_cad.tsv
python3 -c 'import sys; s="▁▂▃▄▅▆▇█"; xs=[int(l.split()[-1]) for l in open("/tmp/_cad.tsv")]; lo,hi=min(xs),max(xs); r=(hi-lo)or 1; print("".join(s[min(7,int((x-lo)/r*7))] for x in xs)); print(f"months: {len(xs)}, min={lo}, max={hi}, last90d≈{sum(xs[-3:])}")'
```

Dead-period detector (≥30-day gaps in daily buckets):

```bash
python3 -c '
from datetime import date
prev=None
for l in open("classify.tsv").readlines()[1:]:
    d=l.split("\t")[0]; cur=date(int(d[:4]),int(d[4:6]),int(d[6:8]))
    if prev and (cur-prev).days>=30:
        print(f"GAP {(cur-prev).days}d  {prev} → {cur}")
    prev=cur'
```

Report the regime, not the numbers: "Steady ~5 commits/mo for 3 yr, spiked to 40 in June 2026. One 92-day gap May–Aug 2025."

## Failure modes

- Script exits non-zero on: not a git repo, bare repo, detached HEAD.
  Propagate the exact stderr line to the user and stop.
- Indexer memory is bounded (< 500 MB on ~3k-commit polyglot repos) via
  the DuckDB Appender path. Churn / cohort fold / tree-sitter run
  rayon-parallel; expect roughly 10 s per 1k commits on a warm 8-core
  dev box.

## Overrides

Trigger examples:

- `/git-arch-analyze` → analyze cwd
- `/git-arch-analyze /path/to/repo` → analyze that repo
- `/git-arch-analyze --report /tmp/report.md` → write report elsewhere
  (default is `<repo>/ARCHAEOLOGY.md`)
