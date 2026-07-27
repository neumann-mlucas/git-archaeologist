use std::collections::{BTreeMap, HashMap};

use anyhow::{Context, Result};
use duckdb::{Connection, ToSql};

use crate::index::bucket::BucketSize;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupBy {
    Language,
    Author,
    Module,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum View {
    Cumulative,
    Delta,
}

/// Three orthogonal questions the tool answers, exposed as top-level modes.
/// Replaces the old (Metric × GroupBy) matrix which had illegal cells
/// (e.g. "LOC by Author" quietly swapped to net-churn behind the user's back).
///
/// - `Structure`: LOC snapshot. "What exists?" — groupable by Language / Module.
/// - `Activity`:  Churn events.   "What changed?" — groupable by Language / Module / Author.
/// - `Ownership`: Blame-based.    "Who owns it?"   — groupable by Author only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lens {
    Structure,
    Activity,
    Ownership,
}

impl Lens {
    pub fn label(self) -> &'static str {
        match self {
            Lens::Structure => "structure",
            Lens::Activity => "activity",
            Lens::Ownership => "ownership",
        }
    }

    /// Groupings this lens supports. Cycling Tab walks this list; changing
    /// lens snaps `group_by` to the first entry if the current one isn't valid.
    pub fn valid_groups(self) -> &'static [GroupBy] {
        match self {
            Lens::Structure => &[GroupBy::Language, GroupBy::Module],
            Lens::Activity => &[GroupBy::Language, GroupBy::Module, GroupBy::Author],
            Lens::Ownership => &[GroupBy::Author],
        }
    }
}

#[derive(Debug, Clone)]
pub struct Filters {
    pub from: Option<i64>,
    pub to: Option<i64>,
    pub languages: Vec<String>,
    pub author_ids: Vec<i64>,
    pub path_scope: String,
    pub module_depth: u8,
    pub bucket: BucketSize,
    pub group_by: GroupBy,
    pub view: View,
    pub lens: Lens,
}

impl Default for Filters {
    fn default() -> Self {
        Self {
            from: None,
            to: None,
            languages: vec![],
            author_ids: vec![],
            path_scope: String::new(),
            module_depth: 1,
            bucket: BucketSize::Week,
            group_by: GroupBy::Language,
            view: View::Cumulative,
            lens: Lens::Structure,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SeriesPoint {
    pub bucket: i64,
    pub group: String,
    pub value: i64,
}

#[derive(Debug, Clone)]
pub struct BreakdownRow {
    pub group: String,
    pub total: i64,
    pub delta: i64,
}

// ─── series ──────────────────────────────────────────────────────────────

pub fn series(conn: &Connection, f: &Filters) -> Result<Vec<SeriesPoint>> {
    // Illegal (lens, group) combos are rejected at the UI level (Tab cycles
    // within `Lens::valid_groups`), but harden here too — a config file could
    // ship a stale default and we shouldn't panic on it.
    if !f.lens.valid_groups().contains(&f.group_by) {
        return Ok(vec![]);
    }

    let raw = match (f.lens, f.group_by) {
        (Lens::Structure, GroupBy::Language) => loc_series_by_language(conn, f)?,
        (Lens::Structure, GroupBy::Module) => loc_series_by_module(conn, f)?,

        (Lens::Activity, GroupBy::Language) => churn_series_by_language(conn, f)?,
        (Lens::Activity, GroupBy::Module) => churn_series_by_module(conn, f)?,
        (Lens::Activity, GroupBy::Author) => churn_series_by_author(conn, f)?,

        // Ownership lens is blame-backed and pending Slice 5 (blame cache).
        // Return empty; the chart panel already handles "no data" gracefully.
        (Lens::Ownership, _) => vec![],

        // Guard branches — technically unreachable given the guard above.
        (Lens::Structure, GroupBy::Author) => vec![],
    };

    Ok(apply_view(raw, f))
}

fn loc_series_by_language(conn: &Connection, f: &Filters) -> Result<Vec<SeriesPoint>> {
    let (date_where, date_params) = date_where_clause(f);
    let scope_like = scope_like(&f.path_scope);
    let (lang_where, lang_params) = in_clause_strings(&f.languages, "fs.language");

    let sql = format!(
        "SELECT c.bucket_key, fs.language, SUM(fs.code)
         FROM file_stats fs
         JOIN commits c ON c.sha = fs.sha
         WHERE c.is_sampled = TRUE
           {date_where}
           AND fs.path LIKE ?
           {lang_where}
         GROUP BY c.bucket_key, fs.language
         ORDER BY c.bucket_key"
    );

    let mut params: Vec<Box<dyn ToSql>> = date_params;
    params.push(Box::new(scope_like));
    params.extend(lang_params);

    fetch_series(conn, &sql, &params)
}

fn loc_series_by_module(conn: &Connection, f: &Filters) -> Result<Vec<SeriesPoint>> {
    // Fetch (bucket, path, code); group by module in Rust (respects scope + depth).
    let (date_where, date_params) = date_where_clause(f);
    let scope_like = scope_like(&f.path_scope);
    let (lang_where, lang_params) = in_clause_strings(&f.languages, "fs.language");

    let sql = format!(
        "SELECT c.bucket_key, fs.path, fs.code
         FROM file_stats fs
         JOIN commits c ON c.sha = fs.sha
         WHERE c.is_sampled = TRUE
           {date_where}
           AND fs.path LIKE ?
           {lang_where}
         ORDER BY c.bucket_key"
    );

    let mut params: Vec<Box<dyn ToSql>> = date_params;
    params.push(Box::new(scope_like));
    params.extend(lang_params);

    let stmt_params: Vec<&dyn ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;

    let mut agg: BTreeMap<(i64, String), i64> = BTreeMap::new();
    let mut rows = stmt.query(stmt_params.as_slice())?;
    while let Some(row) = rows.next()? {
        let bucket: i64 = row.get(0)?;
        let path: String = row.get(1)?;
        let code: i64 = row.get(2)?;
        let module = module_key(&path, &f.path_scope, f.module_depth);
        *agg.entry((bucket, module)).or_insert(0) += code;
    }

    Ok(agg
        .into_iter()
        .map(|((bucket, group), value)| SeriesPoint {
            bucket,
            group,
            value,
        })
        .collect())
}

fn churn_series_by_language(conn: &Connection, f: &Filters) -> Result<Vec<SeriesPoint>> {
    // Language attribution for churn = language of the file path via the most
    // recent file_stats snapshot that contains that path. Build the path→lang
    // map once, then aggregate churn rows in memory.
    let path_lang = latest_path_language_map(conn)?;

    let (date_where, date_params) = date_where_clause(f);
    let scope_like = scope_like(&f.path_scope);
    let sql = format!(
        "SELECT c.bucket_key, ch.path,
                ch.added + ch.deleted
         FROM churn ch
         JOIN commits c ON c.sha = ch.sha
         WHERE 1=1
           {date_where}
           AND ch.path LIKE ?
         ORDER BY c.bucket_key"
    );

    let mut params: Vec<Box<dyn ToSql>> = date_params;
    params.push(Box::new(scope_like));

    let stmt_params: Vec<&dyn ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;

    let lang_filter: std::collections::HashSet<String> = f.languages.iter().cloned().collect();
    let use_filter = !lang_filter.is_empty();

    let mut agg: BTreeMap<(i64, String), i64> = BTreeMap::new();
    let mut rows = stmt.query(stmt_params.as_slice())?;
    while let Some(row) = rows.next()? {
        let bucket: i64 = row.get(0)?;
        let path: String = row.get(1)?;
        let val: i64 = row.get(2)?;
        let lang = match path_lang.get(&path) {
            Some(l) => l,
            None => continue, // path never appeared in a sampled snapshot → unknown language
        };
        if use_filter && !lang_filter.contains(lang) {
            continue;
        }
        *agg.entry((bucket, lang.clone())).or_insert(0) += val;
    }

    Ok(agg
        .into_iter()
        .map(|((bucket, group), value)| SeriesPoint {
            bucket,
            group,
            value,
        })
        .collect())
}

fn latest_path_language_map(conn: &Connection) -> Result<HashMap<String, String>> {
    // Walk sampled snapshots oldest → newest and overwrite; final value per
    // path is the most recent language classification for that path.
    let mut stmt = conn.prepare(
        "SELECT fs.path, fs.language
         FROM file_stats fs
         JOIN commits c ON c.sha = fs.sha
         WHERE c.is_sampled = TRUE
         ORDER BY c.committed_at",
    )?;
    let mut rows = stmt.query([])?;
    let mut out: HashMap<String, String> = HashMap::new();
    while let Some(row) = rows.next()? {
        let path: String = row.get(0)?;
        let lang: String = row.get(1)?;
        out.insert(path, lang);
    }
    Ok(out)
}

fn churn_series_by_author(conn: &Connection, f: &Filters) -> Result<Vec<SeriesPoint>> {
    let (date_where, date_params) = date_where_clause(f);
    let scope_like = scope_like(&f.path_scope);
    let (auth_where, auth_params) = in_clause_ints(&f.author_ids, "c.author_id");

    let sql = format!(
        "SELECT c.bucket_key, c.author_id,
                SUM(ch.added) + SUM(ch.deleted)
         FROM churn ch
         JOIN commits c ON c.sha = ch.sha
         WHERE 1=1
           {date_where}
           AND ch.path LIKE ?
           {auth_where}
         GROUP BY c.bucket_key, c.author_id
         ORDER BY c.bucket_key"
    );

    let mut params: Vec<Box<dyn ToSql>> = date_params;
    params.push(Box::new(scope_like));
    params.extend(auth_params);

    let names = author_names(conn)?;
    fetch_series_with_int_group(conn, &sql, &params, &names)
}

fn churn_series_by_module(conn: &Connection, f: &Filters) -> Result<Vec<SeriesPoint>> {
    let (date_where, date_params) = date_where_clause(f);
    let scope_like = scope_like(&f.path_scope);

    let sql = format!(
        "SELECT c.bucket_key, ch.path,
                ch.added + ch.deleted
         FROM churn ch
         JOIN commits c ON c.sha = ch.sha
         WHERE 1=1
           {date_where}
           AND ch.path LIKE ?
         ORDER BY c.bucket_key"
    );

    let mut params: Vec<Box<dyn ToSql>> = date_params;
    params.push(Box::new(scope_like));

    let stmt_params: Vec<&dyn ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(&sql)?;

    let mut agg: BTreeMap<(i64, String), i64> = BTreeMap::new();
    let mut rows = stmt.query(stmt_params.as_slice())?;
    while let Some(row) = rows.next()? {
        let bucket: i64 = row.get(0)?;
        let path: String = row.get(1)?;
        let val: i64 = row.get(2)?;
        let module = module_key(&path, &f.path_scope, f.module_depth);
        *agg.entry((bucket, module)).or_insert(0) += val;
    }

    Ok(agg
        .into_iter()
        .map(|((bucket, group), value)| SeriesPoint {
            bucket,
            group,
            value,
        })
        .collect())
}

// ─── breakdown ───────────────────────────────────────────────────────────

pub fn breakdown(conn: &Connection, f: &Filters) -> Result<Vec<BreakdownRow>> {
    // Two-point comparison: totals at latest sampled bucket in-window minus
    // totals at the earliest sampled bucket in-window → delta.
    let series = series(conn, f)?;
    if series.is_empty() {
        return Ok(vec![]);
    }

    let (first_bucket, last_bucket) = {
        let mut buckets: Vec<i64> = series.iter().map(|p| p.bucket).collect();
        buckets.sort_unstable();
        buckets.dedup();
        (*buckets.first().unwrap(), *buckets.last().unwrap())
    };

    let mut latest: HashMap<String, i64> = HashMap::new();
    let mut earliest: HashMap<String, i64> = HashMap::new();
    for p in &series {
        if p.bucket == last_bucket {
            *latest.entry(p.group.clone()).or_insert(0) += p.value;
        }
        if p.bucket == first_bucket {
            *earliest.entry(p.group.clone()).or_insert(0) += p.value;
        }
    }

    let mut rows: Vec<BreakdownRow> = latest
        .into_iter()
        .map(|(group, total)| {
            let prior = earliest.get(&group).copied().unwrap_or(0);
            BreakdownRow {
                delta: total - prior,
                total,
                group,
            }
        })
        .collect();

    rows.sort_by_key(|r| std::cmp::Reverse(r.total));
    Ok(rows)
}

// ─── subpaths (drill-down) ───────────────────────────────────────────────

#[derive(Debug, Clone, Copy, Default)]
pub struct CacheStats {
    pub commits: i64,
    pub file_stats: i64,
    pub churn: i64,
    pub authors: i64,
}

pub fn cache_stats(conn: &Connection) -> Result<CacheStats> {
    let count = |sql: &str| -> Result<i64> {
        Ok(conn.query_row(sql, [], |r| r.get::<_, i64>(0))?)
    };
    Ok(CacheStats {
        commits: count("SELECT COUNT(*) FROM commits")?,
        file_stats: count("SELECT COUNT(*) FROM file_stats")?,
        churn: count("SELECT COUNT(*) FROM churn")?,
        authors: count("SELECT COUNT(*) FROM authors")?,
    })
}

pub fn list_languages(conn: &Connection) -> Result<Vec<String>> {
    let mut stmt =
        conn.prepare("SELECT DISTINCT language FROM file_stats ORDER BY language")?;
    let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<duckdb::Result<_>>()?)
}

pub fn list_authors(conn: &Connection) -> Result<Vec<(i64, String, String)>> {
    let mut stmt = conn.prepare(
        "SELECT id, canonical_name, canonical_email FROM authors ORDER BY canonical_name",
    )?;
    let rows = stmt.query_map([], |r| {
        Ok((
            r.get::<_, i64>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })?;
    Ok(rows.collect::<duckdb::Result<_>>()?)
}

#[allow(dead_code)] // reserved for v1.1 path-picker modal
pub fn subpaths(conn: &Connection, scope: &str) -> Result<Vec<String>> {
    let scope_norm = normalize_scope(scope);
    let like = format!("{scope_norm}%");

    let latest_sha: Option<String> = conn
        .query_row(
            "SELECT sha FROM commits WHERE is_sampled = TRUE ORDER BY committed_at DESC LIMIT 1",
            [],
            |r| r.get(0),
        )
        .ok();

    let latest_sha = match latest_sha {
        Some(s) => s,
        None => return Ok(vec![]),
    };

    let mut stmt = conn.prepare(
        "SELECT DISTINCT path FROM file_stats WHERE sha = ? AND path LIKE ?",
    )?;
    let mut rows = stmt.query([&latest_sha as &dyn ToSql, &like as &dyn ToSql])?;

    let mut seen: BTreeMap<String, ()> = BTreeMap::new();
    let scope_len = scope_norm.len();
    while let Some(row) = rows.next()? {
        let path: String = row.get(0)?;
        if let Some(rest) = path.get(scope_len..) {
            let seg = rest.split('/').next().unwrap_or("");
            if !seg.is_empty() {
                seen.insert(seg.to_string(), ());
            }
        }
    }
    Ok(seen.into_keys().collect())
}

// ─── helpers ─────────────────────────────────────────────────────────────

fn fetch_series(
    conn: &Connection,
    sql: &str,
    params: &[Box<dyn ToSql>],
) -> Result<Vec<SeriesPoint>> {
    let stmt_params: Vec<&dyn ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(sql).context("preparing series SQL")?;
    let mut rows = stmt.query(stmt_params.as_slice())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        out.push(SeriesPoint {
            bucket: row.get(0)?,
            group: row.get::<_, String>(1)?,
            value: row.get::<_, i64>(2)?,
        });
    }
    Ok(out)
}

fn fetch_series_with_int_group(
    conn: &Connection,
    sql: &str,
    params: &[Box<dyn ToSql>],
    labels: &HashMap<i64, String>,
) -> Result<Vec<SeriesPoint>> {
    let stmt_params: Vec<&dyn ToSql> = params.iter().map(|b| b.as_ref()).collect();
    let mut stmt = conn.prepare(sql).context("preparing series SQL")?;
    let mut rows = stmt.query(stmt_params.as_slice())?;
    let mut out = Vec::new();
    while let Some(row) = rows.next()? {
        let id: i64 = row.get(1)?;
        let label = labels
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("author#{id}"));
        out.push(SeriesPoint {
            bucket: row.get(0)?,
            group: label,
            value: row.get::<_, i64>(2)?,
        });
    }
    Ok(out)
}

fn author_names(conn: &Connection) -> Result<HashMap<i64, String>> {
    let mut stmt = conn.prepare("SELECT id, canonical_name FROM authors")?;
    let rows = stmt.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?)))?;
    let mut out = HashMap::new();
    for r in rows {
        let (id, name) = r?;
        out.insert(id, name);
    }
    Ok(out)
}

fn date_where_clause(f: &Filters) -> (String, Vec<Box<dyn ToSql>>) {
    let mut s = String::new();
    let mut p: Vec<Box<dyn ToSql>> = Vec::new();
    if let Some(from) = f.from {
        s.push_str(" AND c.committed_at >= ?");
        p.push(Box::new(from));
    }
    if let Some(to) = f.to {
        s.push_str(" AND c.committed_at <= ?");
        p.push(Box::new(to));
    }
    (s, p)
}

fn in_clause_strings(vals: &[String], col: &str) -> (String, Vec<Box<dyn ToSql>>) {
    if vals.is_empty() {
        return (String::new(), vec![]);
    }
    let placeholders: Vec<&str> = vals.iter().map(|_| "?").collect();
    let clause = format!(" AND {col} IN ({})", placeholders.join(","));
    let params: Vec<Box<dyn ToSql>> =
        vals.iter().map(|v| Box::new(v.clone()) as Box<dyn ToSql>).collect();
    (clause, params)
}

fn in_clause_ints(vals: &[i64], col: &str) -> (String, Vec<Box<dyn ToSql>>) {
    if vals.is_empty() {
        return (String::new(), vec![]);
    }
    let placeholders: Vec<&str> = vals.iter().map(|_| "?").collect();
    let clause = format!(" AND {col} IN ({})", placeholders.join(","));
    let params: Vec<Box<dyn ToSql>> =
        vals.iter().map(|v| Box::new(*v) as Box<dyn ToSql>).collect();
    (clause, params)
}

fn scope_like(scope: &str) -> String {
    let s = normalize_scope(scope);
    format!("{s}%")
}

fn normalize_scope(scope: &str) -> String {
    let trimmed = scope.trim_matches('/');
    if trimmed.is_empty() {
        String::new()
    } else {
        format!("{trimmed}/")
    }
}

/// Extract the module label for `path` given the current scope + depth.
/// Depth counts segments below the scope.
fn module_key(path: &str, scope: &str, depth: u8) -> String {
    let scope_norm = normalize_scope(scope);
    let rest = path.strip_prefix(&scope_norm).unwrap_or(path);
    let mut segs = rest.split('/');
    let mut collected = Vec::with_capacity(depth as usize);
    for _ in 0..depth.max(1) {
        match segs.next() {
            Some(s) if !s.is_empty() => collected.push(s),
            _ => break,
        }
    }
    if collected.is_empty() {
        "(root)".to_string()
    } else {
        collected.join("/")
    }
}

fn apply_view(points: Vec<SeriesPoint>, f: &Filters) -> Vec<SeriesPoint> {
    // Semantics per lens:
    //   Structure  Cumulative → raw snapshot (already correct)
    //              Delta      → snapshot[t] - snapshot[t-1] (dense-fill gaps)
    //   Activity   Delta      → per-bucket churn (native from query)
    //              Cumulative → running sum of churn up to bucket
    //   Ownership  either     → raw (blame cache TBD, currently empty)
    match (f.lens, f.view) {
        (Lens::Structure, View::Cumulative) => points,
        (Lens::Structure, View::Delta) => dense_fill_then_subtract(points),
        (Lens::Activity, View::Delta) => points,
        (Lens::Activity, View::Cumulative) => running_sum_per_group(points),
        (Lens::Ownership, _) => points,
    }
}

/// Dense-fill missing (group, bucket) pairs with 0 before subtracting the
/// prior bucket's value. A group that vanishes and reappears registers a
/// correct negative-then-positive swing at the gap instead of silently
/// carrying its last-seen value forward.
fn dense_fill_then_subtract(points: Vec<SeriesPoint>) -> Vec<SeriesPoint> {
    let mut buckets: std::collections::BTreeSet<i64> = std::collections::BTreeSet::new();
    for p in &points {
        buckets.insert(p.bucket);
    }
    let mut nested: HashMap<String, HashMap<i64, i64>> = HashMap::new();
    for p in points {
        nested.entry(p.group).or_default().insert(p.bucket, p.value);
    }
    let mut out = Vec::with_capacity(nested.len() * buckets.len());
    for (group, per_bucket) in &nested {
        let mut prev = 0i64;
        for &b in &buckets {
            let v = per_bucket.get(&b).copied().unwrap_or(0);
            out.push(SeriesPoint {
                bucket: b,
                group: group.clone(),
                value: v - prev,
            });
            prev = v;
        }
    }
    out
}

/// Running sum per group over ascending buckets. Churn rows come out of SQL
/// as per-bucket deltas; cumulative view is derived here.
fn running_sum_per_group(mut points: Vec<SeriesPoint>) -> Vec<SeriesPoint> {
    points.sort_by(|a, b| a.group.cmp(&b.group).then(a.bucket.cmp(&b.bucket)));
    let mut running: HashMap<String, i64> = HashMap::new();
    for p in points.iter_mut() {
        let acc = running.entry(p.group.clone()).or_insert(0);
        *acc += p.value;
        p.value = *acc;
    }
    points
}
