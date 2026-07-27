use anyhow::Result;
use rusqlite::Connection;

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Metric {
    Loc,
    Churn,
}

#[derive(Debug, Clone)]
pub struct Filters {
    pub from: Option<i64>,       // unix seconds
    pub to: Option<i64>,         // unix seconds
    pub languages: Vec<String>,  // empty = all
    pub author_ids: Vec<i64>,    // empty = all
    pub path_scope: String,      // "" = repo root
    pub module_depth: u8,        // segments below scope to group by
    pub bucket: BucketSize,
    pub group_by: GroupBy,
    pub view: View,
    pub metric: Metric,
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
            metric: Metric::Loc,
        }
    }
}

/// One point in a time series: (bucket_key, group_label, value).
#[derive(Debug, Clone)]
pub struct SeriesPoint {
    pub bucket: i64,
    pub group: String,
    pub value: i64,
}

/// One row in the breakdown table: (group_label, total, delta_within_window, share).
#[derive(Debug, Clone)]
pub struct BreakdownRow {
    pub group: String,
    pub total: i64,
    pub delta: i64,
    pub share: f64,
}

/// Fetch time series for the chart.
pub fn series(_conn: &Connection, _f: &Filters) -> Result<Vec<SeriesPoint>> {
    // Build dynamic SQL:
    //   base: file_stats (LOC) or churn (Churn) joined to commits (sampled + in date range)
    //   filter: language IN (...), author_id IN (...), path LIKE scope||'%'
    //   group: bucket_key, <group_col>
    //   agg:   SUM(code) [LOC] or SUM(added-deleted) cumulative [Churn]
    // For View::Delta, take diffs between consecutive buckets per group.
    todo!("build series SQL")
}

/// Fetch breakdown rows for the table (current scope, honoring filters).
pub fn breakdown(_conn: &Connection, _f: &Filters) -> Result<Vec<BreakdownRow>> {
    todo!("build breakdown SQL")
}

/// List distinct path segments one level below `scope` (for drill-down).
pub fn subpaths(_conn: &Connection, _scope: &str) -> Result<Vec<String>> {
    todo!("SELECT DISTINCT segment via substr/instr on path")
}
