//! Query subcommands. One DuckDB query per SPEC §Metrics.
//!
//! Each `pub fn` returns `Table { headers, rows }` — the CLI renders it in
//! whatever `--format` the user asked for. Filter plumbing lives in the
//! `Filters` type so every subcommand accepts the same SPEC §Filters set.

use anyhow::{bail, Context, Result};
use duckdb::types::ValueRef;
use time::format_description::well_known::Iso8601;
use time::{Date, OffsetDateTime, Time};

use crate::cache::Cache;

pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

pub struct Filters {
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub langs: Option<Vec<String>>,
    pub author: Option<String>,
    pub path_prefix: Option<String>,
}

impl Filters {
    pub fn parse(
        from: Option<&str>,
        to: Option<&str>,
        lang: Option<&str>,
        author: Option<&str>,
        path: Option<&str>,
    ) -> Result<Self> {
        Ok(Self {
            from: from.map(parse_from_date).transpose()?,
            to: to.map(parse_to_date).transpose()?,
            langs: lang.map(|s| {
                s.split(',')
                    .map(|t| t.trim().to_ascii_lowercase())
                    .filter(|t| !t.is_empty())
                    .collect()
            }),
            author: author.map(|s| s.to_string()),
            path_prefix: path.map(|s| s.to_string()),
        })
    }

    /// WHERE-fragment on the `commits` table. Includes leading " AND " if
    /// any clauses apply; empty otherwise. Author filter joins via
    /// `authors` — caller must include that join.
    pub fn commit_where(&self) -> String {
        let mut parts: Vec<String> = Vec::new();
        if let Some(f) = self.from {
            parts.push(format!("c.authored_at >= {f}"));
        }
        if let Some(t) = self.to {
            parts.push(format!("c.authored_at < {t}"));
        }
        if let Some(a) = &self.author {
            let a = sql_escape(a).to_ascii_lowercase();
            parts.push(format!(
                "(LOWER(a.canonical_name) LIKE '%{a}%' OR LOWER(a.canonical_email) LIKE '%{a}%')"
            ));
        }
        if parts.is_empty() {
            String::new()
        } else {
            format!(" AND {}", parts.join(" AND "))
        }
    }

    pub fn path_where(&self, path_col: &str) -> String {
        match &self.path_prefix {
            Some(p) => {
                let p = sql_escape(p);
                format!(" AND {path_col} LIKE '{p}%'")
            }
            None => String::new(),
        }
    }

    pub fn lang_where(&self, lang_col: &str) -> String {
        match &self.langs {
            Some(ls) if !ls.is_empty() => {
                let list: Vec<String> = ls
                    .iter()
                    .map(|l| format!("'{}'", sql_escape(l)))
                    .collect();
                format!(" AND LOWER({lang_col}) IN ({})", list.join(","))
            }
            _ => String::new(),
        }
    }

    /// Author filter needs an `authors a` join to work. `commits` table
    /// aliased `c`. Returns the JOIN clause (or empty).
    pub fn author_join(&self) -> &'static str {
        if self.author.is_some() {
            " JOIN authors a ON a.id = c.author_id"
        } else {
            ""
        }
    }
}

fn parse_from_date(s: &str) -> Result<i64> {
    let d = parse_ymd(s)?;
    Ok(OffsetDateTime::new_utc(d, Time::MIDNIGHT).unix_timestamp())
}

fn parse_to_date(s: &str) -> Result<i64> {
    // `--to` is inclusive of the named day → use midnight of next day.
    let d = parse_ymd(s)?;
    let next = d.next_day().context("--to date has no next day")?;
    Ok(OffsetDateTime::new_utc(next, Time::MIDNIGHT).unix_timestamp())
}

fn parse_ymd(s: &str) -> Result<Date> {
    Date::parse(s, &Iso8601::DATE).with_context(|| format!("invalid date {s:?}, expected YYYY-MM-DD"))
}

fn sql_escape(s: &str) -> String {
    s.replace('\'', "''")
}

// -------- subcommand queries --------

/// Conventional-commit type shares per bucket + breaking/revert rates.
pub fn classify(cache: &Cache, filters: &Filters) -> Result<Table> {
    let filter = filters.commit_where();
    let join = filters.author_join();
    let sql = format!(
        "SELECT
             c.bucket_key                                             AS bucket,
             COUNT(*)                                                 AS commits,
             SUM(CASE WHEN c.msg_type = 'fix'    THEN 1 ELSE 0 END)   AS fixes,
             SUM(CASE WHEN c.msg_type = 'feat'   THEN 1 ELSE 0 END)   AS feats,
             SUM(CASE WHEN c.is_revert           THEN 1 ELSE 0 END)   AS reverts,
             SUM(CASE WHEN c.is_breaking         THEN 1 ELSE 0 END)   AS breaking,
             SUM(CASE WHEN c.msg_type IS NULL    THEN 1 ELSE 0 END)   AS untyped
         FROM commits c
         {join}
         WHERE c.is_merge = FALSE {filter}
         GROUP BY bucket
         ORDER BY bucket",
    );
    run_sql(cache, &sql)
}

/// File age histogram — days since first touch, bucketed.
pub fn age(cache: &Cache, filters: &Filters) -> Result<Table> {
    // First-touched = MIN(authored_at) for a path across file_churn.
    // Age in days = (now - first_touched) / 86400. Bucket into 7-day bins.
    let path_filter = filters.path_where("fc.path");
    let now = OffsetDateTime::now_utc().unix_timestamp();
    let sql = format!(
        "WITH first_touch AS (
             SELECT fc.path              AS path,
                    MIN(c.authored_at)   AS first_at
             FROM   file_churn fc
             JOIN   commits    c ON c.sha = fc.sha
             WHERE  1=1 {path_filter}
             GROUP  BY fc.path
         )
         SELECT CAST((( {now} - first_at ) / 86400 / 7) AS BIGINT) * 7 AS age_days_bucket,
                COUNT(*)                                                AS files
         FROM first_touch
         GROUP BY age_days_bucket
         ORDER BY age_days_bucket",
    );
    run_sql(cache, &sql)
}

/// Churn per module | lang | author over the whole cache (bucketed).
pub fn churn(cache: &Cache, filters: &Filters, by: &str) -> Result<Table> {
    let filter = filters.commit_where();
    let path_filter = filters.path_where("fc.path");
    let lang_filter = filters.lang_where("fs.language");
    let join = filters.author_join();

    let sql = match by {
        "module" => format!(
            "SELECT c.bucket_key                                    AS bucket,
                    split_part(fc.path, '/', 1)                     AS module,
                    SUM(fc.added)                                   AS added,
                    SUM(fc.deleted)                                 AS deleted
             FROM   file_churn fc
             JOIN   commits    c ON c.sha = fc.sha
             {join}
             WHERE  c.is_merge = FALSE {filter} {path_filter}
             GROUP  BY bucket, module
             ORDER  BY bucket, added DESC",
        ),
        "lang" | "language" => format!(
            // Language attribution is only known for sampled commits (via
            // file_stats). Join fc → fs on (sha,path); rows without a
            // language row are grouped as 'unknown' so they still show up.
            "SELECT c.bucket_key                                    AS bucket,
                    COALESCE(fs.language, 'unknown')                AS language,
                    SUM(fc.added)                                   AS added,
                    SUM(fc.deleted)                                 AS deleted
             FROM   file_churn fc
             LEFT   JOIN file_stats fs ON fs.sha = fc.sha AND fs.path = fc.path
             JOIN   commits    c ON c.sha = fc.sha
             {join}
             WHERE  c.is_merge = FALSE {filter} {path_filter} {lang_filter}
             GROUP  BY bucket, language
             ORDER  BY bucket, added DESC",
        ),
        "author" => format!(
            "SELECT c.bucket_key                                    AS bucket,
                    au.canonical_name                               AS author,
                    SUM(fc.added)                                   AS added,
                    SUM(fc.deleted)                                 AS deleted
             FROM   file_churn fc
             JOIN   commits    c  ON c.sha = fc.sha
             JOIN   authors    au ON au.id = c.author_id
             {}
             WHERE  c.is_merge = FALSE {filter} {path_filter}
             GROUP  BY bucket, author
             ORDER  BY bucket, added DESC",
            // If the filter join also aliased `a`, we'd double-alias. Keep
            // it simple: reuse the same `authors a` alias so filters
            // matching by name still work.
            if filters.author.is_some() { "" } else { "" }
        ),
        other => bail!("--by must be one of module|lang|author, got {other:?}"),
    };
    run_sql(cache, &sql)
}

/// Cumulative LOC per bucket, grouped by language or author.
///
/// `by = language`: exact snapshot — `SUM(file_stats.code)` at each
/// sampled commit, grouped by `fs.language`. Reflects the on-disk state at
/// the sampling point.
///
/// `by = author`: approximate — running `SUM(added - deleted)` per author
/// per bucket, computed from `file_churn`. Not the same as "lines still
/// alive that author X wrote", which requires the cohort fold (Phase 3).
/// The approximation is a legitimate per-author net-contribution series.
pub fn burndown(cache: &Cache, filters: &Filters, by: &str) -> Result<Table> {
    let filter = filters.commit_where();
    let path_filter = filters.path_where("fs.path");
    let lang_filter = filters.lang_where("fs.language");
    let author_join = filters.author_join();

    let sql = match by {
        "language" | "lang" => format!(
            "SELECT c.bucket_key       AS bucket,
                    fs.language        AS language,
                    SUM(fs.code)       AS loc
             FROM   commits c
             JOIN   file_stats fs ON fs.sha = c.sha
             {author_join}
             WHERE  c.is_sampled = TRUE {filter} {path_filter} {lang_filter}
             GROUP  BY bucket, language
             ORDER  BY bucket, language",
        ),
        "author" => {
            let path_fc = filters.path_where("fc.path");
            format!(
                "WITH per_bucket AS (
                     SELECT c.bucket_key         AS bucket,
                            au.canonical_name    AS author,
                            SUM(fc.added) - SUM(fc.deleted) AS net
                     FROM   file_churn fc
                     JOIN   commits c  ON c.sha = fc.sha
                     JOIN   authors au ON au.id = c.author_id
                     WHERE  c.is_merge = FALSE {filter} {path_fc}
                     GROUP  BY bucket, author
                 )
                 SELECT bucket,
                        author,
                        SUM(net) OVER (PARTITION BY author ORDER BY bucket
                                       ROWS BETWEEN UNBOUNDED PRECEDING AND CURRENT ROW) AS loc
                 FROM   per_bucket
                 ORDER  BY bucket, author",
            )
        }
        other => bail!("--by must be one of language|author, got {other:?}"),
    };
    run_sql(cache, &sql)
}

/// Top-N functions by hunk-attributed churn per bucket.
///
/// v1 approximation: for each (path, function), use the **latest sampled**
/// commit's function map — hunks touching lines in `[start_line, end_line]`
/// are attributed to that function. Doesn't chase line drift over time; a
/// function that moved within its file collapses its history under the new
/// location.
///
/// `--lang` is required and restricts the hunk set to files of that
/// language at the sampled commit.
pub fn hotspot(cache: &Cache, filters: &Filters, top: usize) -> Result<Table> {
    if filters.langs.is_none() {
        bail!("hotspot requires --lang <L>");
    }
    let filter = filters.commit_where();
    let path_filter = filters.path_where("h.path");
    let lang_filter = filters.lang_where("fs.language");
    let author_join = filters.author_join();

    let sql = format!(
        "WITH ranked_snapshots AS (
             SELECT fs.path,
                    fs.sha,
                    fs.language,
                    c.bucket_key,
                    ROW_NUMBER() OVER (PARTITION BY fs.path ORDER BY c.bucket_key DESC) AS rn
             FROM   file_stats fs
             JOIN   commits c ON c.sha = fs.sha
             WHERE  1=1 {lang_filter}
         ),
         latest AS (
             SELECT rs.path,
                    rs.sha,
                    rs.language,
                    f.name,
                    f.kind,
                    f.start_line,
                    f.end_line
             FROM   ranked_snapshots rs
             JOIN   funcs f ON f.sha = rs.sha AND f.path = rs.path
             WHERE  rs.rn = 1
         )
         SELECT c.bucket_key                                AS bucket,
                latest.path                                 AS path,
                latest.name                                 AS func,
                latest.kind                                 AS kind,
                latest.language                             AS language,
                SUM(h.new_len)                              AS added,
                SUM(h.old_len)                              AS deleted,
                SUM(h.new_len + h.old_len)                  AS churn
         FROM   hunks h
         JOIN   commits c    ON c.sha = h.sha
         JOIN   latest       ON latest.path = h.path
                             AND h.new_start >= latest.start_line
                             AND h.new_start <= latest.end_line
         {author_join}
         WHERE  c.is_merge = FALSE {filter} {path_filter}
         GROUP  BY c.bucket_key, latest.path, latest.name, latest.kind, latest.language
         ORDER  BY churn DESC
         LIMIT  {top}",
    );
    run_sql(cache, &sql)
}
///
/// A self-join on `(sha, path)` blows up on squash/import commits; the
/// `--max-files-per-commit` cap drops those from the count.
pub fn coupling(
    cache: &Cache,
    filters: &Filters,
    top: usize,
    max_files_per_commit: usize,
) -> Result<Table> {
    let filter = filters.commit_where();
    let path_filter_a = filters.path_where("a.path");
    let path_filter_b = filters.path_where("b.path");
    let join = filters.author_join();
    let sql = format!(
        "WITH kept AS (
             SELECT c.sha, fc.path
             FROM   file_churn fc
             JOIN   commits    c ON c.sha = fc.sha
             {join}
             WHERE  c.is_merge = FALSE {filter}
         ),
         sized AS (
             SELECT sha
             FROM   kept
             GROUP  BY sha
             HAVING COUNT(*) <= {max_files_per_commit}
         )
         SELECT a.path                            AS path_a,
                b.path                            AS path_b,
                COUNT(*)                          AS co_commits
         FROM   kept a
         JOIN   kept b ON a.sha = b.sha AND a.path < b.path
         JOIN   sized s ON s.sha = a.sha
         WHERE  1=1 {path_filter_a} {path_filter_b}
         GROUP  BY path_a, path_b
         ORDER  BY co_commits DESC, path_a, path_b
         LIMIT  {top}",
    );
    run_sql(cache, &sql)
}

/// Cohort-stacked LOC series. For each bucket B, sum surviving lines
/// whose `birth_bucket <= B`, grouped by birth_bucket.
pub fn cohort(cache: &Cache, filters: &Filters) -> Result<Table> {
    let path_filter = filters.path_where("lb.path");
    // birth_bucket per line is fixed at index-time; --from/--to filter by
    // the birth timestamp of that cohort. Convert bucket_key back to a
    // birth authored_at via the commits table.
    let mut where_parts = vec!["1=1".to_string()];
    if let Some(f) = filters.from {
        where_parts.push(format!("c_birth.authored_at >= {f}"));
    }
    if let Some(t) = filters.to {
        where_parts.push(format!("c_birth.authored_at < {t}"));
    }
    let cohort_where = where_parts.join(" AND ");

    // Skip the commits join when there's no date filter — otherwise
    // we drag 800k line_births rows through a join that filters nothing.
    let has_date_filter = filters.from.is_some() || filters.to.is_some();
    let filtered_lb_sql = if has_date_filter {
        format!(
            "SELECT lb.birth_bucket
             FROM   line_births lb
             JOIN   commits c_birth ON c_birth.sha = lb.birth_sha
             WHERE  {cohort_where} {path_filter}"
        )
    } else {
        format!(
            "SELECT lb.birth_bucket
             FROM   line_births lb
             WHERE  1=1 {path_filter}"
        )
    };

    let sql = format!(
        // `line_births` only holds surviving lines at HEAD, so a line
        // with birth_bucket X is alive at every bucket B >= X. Pre-
        // aggregate to (cohort, alive) — one row per cohort, then cross
        // join with the bucket universe. Was: 800k×N range join +
        // GROUP BY at the end (~2.4 s on ratatui). Is: 100×100 cross
        // join (< 200 ms).
        "WITH filtered_lb AS (
             {filtered_lb_sql}
         ),
         per_cohort AS (
             SELECT birth_bucket AS cohort, COUNT(*) AS alive
             FROM   filtered_lb
             GROUP  BY birth_bucket
         ),
         buckets AS (
             SELECT DISTINCT bucket_key AS bucket
             FROM   commits
             WHERE  is_sampled = TRUE
         )
         SELECT b.bucket        AS bucket,
                pc.cohort       AS cohort,
                pc.alive        AS alive_lines
         FROM   buckets b
         JOIN   per_cohort pc ON pc.cohort <= b.bucket
         ORDER  BY b.bucket, pc.cohort",
    );
    run_sql(cache, &sql)
}

/// Kaplan-Meier survival on line lifetimes.
///
/// A line "dies" when a hunk deletes it. Since `line_births` is the final
/// state at HEAD, dead lines are implicit — total `(added, deleted)` from
/// `file_churn` gives us birth + death counts per bucket, but true KM needs
/// per-line lifetimes. v1 approximation: per birth_bucket B, deletion events
/// = total deletes from commits after B affecting files with any B-born
/// line; alive at HEAD = COUNT of line_births rows born in B. `--fit exp`
/// returns a scalar half-life when >= 100 deletion events, else NULL.
pub fn survival(cache: &Cache, filters: &Filters, fit_exp: bool) -> Result<Table> {
    let path_filter = filters.path_where("lb.path");
    let birth_path_filter = filters.path_where("h.path");
    let commit_filter = filters.commit_where();
    let author_join = filters.author_join();

    // Total lines ever born per cohort = lines still alive + all deletes
    // that touched files after cohort birth. Approximation: use total
    // per-bucket added (from hunks new_len sum where new_len > 0) as
    // "births" and per-bucket deleted as "deaths". Filters restrict the
    // cohort universe when supplied.
    let sql = format!(
        "WITH births AS (
             SELECT c.bucket_key AS cohort, SUM(h.new_len) AS born
             FROM   hunks h
             JOIN   commits c ON c.sha = h.sha
             {author_join}
             WHERE  h.new_len > 0 {commit_filter} {birth_path_filter}
             GROUP  BY c.bucket_key
         ),
         alive_now AS (
             SELECT lb.birth_bucket AS cohort, COUNT(*) AS alive
             FROM   line_births lb
             WHERE  1=1 {path_filter}
             GROUP  BY lb.birth_bucket
         )
         SELECT b.cohort                                    AS birth_bucket,
                b.born                                      AS born,
                COALESCE(a.alive, 0)                        AS alive,
                b.born - COALESCE(a.alive, 0)               AS dead,
                CAST(COALESCE(a.alive, 0) AS DOUBLE) / NULLIF(b.born, 0) AS survival
         FROM   births b
         LEFT   JOIN alive_now a ON a.cohort = b.cohort
         ORDER  BY birth_bucket",
    );

    let mut table = run_sql(cache, &sql)?;

    if fit_exp {
        table = fit_half_life(&table)?;
    }

    Ok(table)
}

/// Take the survival table (birth_bucket, born, alive, dead, survival)
/// and fit an exponential decay ln(survival) = -λ * age to derive a
/// half-life scalar. Gated on ≥ 100 total deletion events per SPEC.
fn fit_half_life(table: &Table) -> Result<Table> {
    let dead_col = table
        .headers
        .iter()
        .position(|h| h == "dead")
        .context("survival table missing 'dead' column")?;
    let survival_col = table
        .headers
        .iter()
        .position(|h| h == "survival")
        .context("survival table missing 'survival' column")?;
    let bucket_col = table
        .headers
        .iter()
        .position(|h| h == "birth_bucket")
        .context("survival table missing 'birth_bucket' column")?;

    let mut total_deaths: i64 = 0;
    let mut xs: Vec<f64> = Vec::new();
    let mut ys: Vec<f64> = Vec::new();
    let mut max_bucket: f64 = f64::NEG_INFINITY;

    for row in &table.rows {
        let dead: i64 = row.get(dead_col).and_then(|s| s.parse().ok()).unwrap_or(0);
        total_deaths += dead;
        let s: f64 = row
            .get(survival_col)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        let b: f64 = row
            .get(bucket_col)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0.0);
        if s > 0.0 && s < 1.0 {
            xs.push(b);
            ys.push(s.ln());
        }
        if b > max_bucket {
            max_bucket = b;
        }
    }

    let headers = vec!["metric".to_string(), "value".to_string(), "reason".to_string()];
    let mut rows: Vec<Vec<String>> = Vec::new();

    if total_deaths < 100 || xs.len() < 2 {
        rows.push(vec![
            "half_life".to_string(),
            String::new(),
            format!("insufficient data: {total_deaths} deletion events (need >= 100)"),
        ]);
        return Ok(Table { headers, rows });
    }

    // Simple linear regression on (x = max_bucket - birth_bucket, y = ln(survival)):
    // slope = Σ(x - x̄)(y - ȳ) / Σ(x - x̄)². Half-life = ln(2) / -slope.
    let n = xs.len() as f64;
    let ages: Vec<f64> = xs.iter().map(|b| max_bucket - b).collect();
    let x_mean: f64 = ages.iter().sum::<f64>() / n;
    let y_mean: f64 = ys.iter().sum::<f64>() / n;
    let num: f64 = ages
        .iter()
        .zip(ys.iter())
        .map(|(x, y)| (x - x_mean) * (y - y_mean))
        .sum();
    let den: f64 = ages.iter().map(|x| (x - x_mean).powi(2)).sum();
    if den == 0.0 {
        rows.push(vec![
            "half_life".to_string(),
            String::new(),
            "degenerate fit (all ages equal)".to_string(),
        ]);
        return Ok(Table { headers, rows });
    }
    let slope = num / den;
    if slope >= 0.0 {
        rows.push(vec![
            "half_life".to_string(),
            String::new(),
            "positive slope (survival not decaying)".to_string(),
        ]);
        return Ok(Table { headers, rows });
    }
    let half_life = std::f64::consts::LN_2 / (-slope);
    rows.push(vec![
        "half_life".to_string(),
        format!("{half_life:.3}"),
        format!("ok (n={n}, deaths={total_deaths})"),
    ]);
    Ok(Table { headers, rows })
}

fn run_sql(cache: &Cache, sql: &str) -> Result<Table> {
    let mut stmt = cache.conn.prepare(sql)?;
    let mut rows_iter = stmt.query([])?;
    let col_count = rows_iter.as_ref().map(|r| r.column_count()).unwrap_or(0);
    let mut headers: Vec<String> = Vec::with_capacity(col_count);
    for i in 0..col_count {
        let name = rows_iter
            .as_ref()
            .and_then(|r| r.column_name(i).ok())
            .cloned()
            .unwrap_or_default();
        headers.push(name);
    }
    let mut rows: Vec<Vec<String>> = Vec::new();
    while let Some(row) = rows_iter.next()? {
        let mut r = Vec::with_capacity(col_count);
        for i in 0..col_count {
            r.push(value_to_string(row.get_ref(i)?));
        }
        rows.push(r);
    }
    Ok(Table { headers, rows })
}

pub fn value_to_string(v: ValueRef) -> String {
    match v {
        ValueRef::Null => String::new(),
        ValueRef::Boolean(b) => b.to_string(),
        ValueRef::TinyInt(i) => i.to_string(),
        ValueRef::SmallInt(i) => i.to_string(),
        ValueRef::Int(i) => i.to_string(),
        ValueRef::BigInt(i) => i.to_string(),
        ValueRef::HugeInt(i) => i.to_string(),
        ValueRef::UTinyInt(i) => i.to_string(),
        ValueRef::USmallInt(i) => i.to_string(),
        ValueRef::UInt(i) => i.to_string(),
        ValueRef::UBigInt(i) => i.to_string(),
        ValueRef::Float(f) => f.to_string(),
        ValueRef::Double(f) => f.to_string(),
        ValueRef::Decimal(d) => d.to_string(),
        ValueRef::Timestamp(_, t) => t.to_string(),
        ValueRef::Text(bytes) => String::from_utf8_lossy(bytes).into_owned(),
        ValueRef::Blob(bytes) => format!("<{} bytes>", bytes.len()),
        ValueRef::Date32(d) => d.to_string(),
        ValueRef::Time64(_, t) => t.to_string(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn from_date_is_midnight_utc() {
        assert_eq!(parse_from_date("2025-01-15").unwrap(), 1_736_899_200);
    }

    #[test]
    fn to_date_is_exclusive_next_day() {
        // --to 2025-01-15 → strictly less than 2025-01-16 00:00.
        assert_eq!(parse_to_date("2025-01-15").unwrap(), 1_736_985_600);
    }

    #[test]
    fn bad_date_errors() {
        assert!(parse_from_date("nope").is_err());
    }

    #[test]
    fn filter_where_empty_by_default() {
        let f = Filters::parse(None, None, None, None, None).unwrap();
        assert_eq!(f.commit_where(), "");
        assert_eq!(f.path_where("fc.path"), "");
        assert_eq!(f.lang_where("fs.language"), "");
        assert_eq!(f.author_join(), "");
    }

    #[test]
    fn filter_where_builds_and_clauses() {
        let f = Filters::parse(
            Some("2025-01-01"),
            Some("2025-12-31"),
            Some("Rust,python"),
            Some("Ada"),
            Some("src/"),
        )
        .unwrap();
        let w = f.commit_where();
        assert!(w.starts_with(" AND"));
        assert!(w.contains("c.authored_at >="));
        assert!(w.contains("c.authored_at <"));
        assert!(w.contains("LOWER(a.canonical_name) LIKE '%ada%'"));
        assert_eq!(f.path_where("fc.path"), " AND fc.path LIKE 'src/%'");
        assert_eq!(
            f.lang_where("fs.language"),
            " AND LOWER(fs.language) IN ('rust','python')"
        );
        assert_eq!(f.author_join(), " JOIN authors a ON a.id = c.author_id");
    }

    #[test]
    fn sql_escape_doubles_single_quotes() {
        assert_eq!(sql_escape("O'Reilly"), "O''Reilly");
    }
}
