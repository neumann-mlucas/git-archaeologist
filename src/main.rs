use std::io::{IsTerminal, Write as _};
use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand, ValueEnum};

mod cache;
mod config;
mod index;
mod query;
mod repo;

use crate::index::bucket::BucketSize;

const TABLES: &[&str] = &[
    "meta",
    "authors",
    "author_aliases",
    "commits",
    "commit_parents",
    "commit_trailers",
    "tags",
    "hunks",
    "file_churn",
    "file_stats",
    "funcs",
    "line_births",
];

#[derive(Parser, Debug)]
#[command(name = "git-archaeologist", version, about)]
struct Cli {
    /// Repository path (defaults to current directory).
    #[arg(long = "repo", default_value = ".", global = true)]
    repo: PathBuf,

    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand, Debug)]
enum Cmd {
    /// Build or update the cache.
    Index(IndexArgs),
    /// Wipe cache and rebuild from scratch.
    Reindex(IndexArgs),
    /// Dump every table as parquet | csv | json.
    Export {
        #[arg(value_enum)]
        format: ExportFormat,
        dir: PathBuf,
    },
    /// Run a raw DuckDB query against the cache.
    Sql {
        query: String,
        #[arg(long, value_enum)]
        format: Option<FormatArg>,
    },
    /// Cumulative LOC series.
    Burndown(QueryArgs),
    /// Cohort-stacked LOC series.
    Cohort(QueryArgs),
    /// Kaplan-Meier survival curve.
    Survival {
        #[command(flatten)]
        q: QueryArgs,
        /// Fit exponential; emits half-life scalar when >= 100 deletion events.
        #[arg(long)]
        fit: Option<String>,
    },
    /// Top-N file pairs by co-commit.
    Coupling {
        #[command(flatten)]
        q: QueryArgs,
        #[arg(long, default_value_t = 50)]
        top: usize,
        #[arg(long = "max-files-per-commit", default_value_t = 50)]
        max_files_per_commit: usize,
    },
    /// Conventional Commit type shares per bucket.
    Classify(QueryArgs),
    /// Top-N funcs by churn (tree-sitter langs only).
    Hotspot {
        #[command(flatten)]
        q: QueryArgs,
        #[arg(long, default_value_t = 30)]
        top: usize,
    },
    /// File age histogram.
    Age(QueryArgs),
    /// Churn per module / lang / author.
    Churn {
        #[command(flatten)]
        q: QueryArgs,
        /// Path-segment depth for `--by module`. `1` = top-level dir
        /// (`src`, `runtime`), `2` = one level deeper (`src/nvim`,
        /// `runtime/lua`). Ignored for other `--by` axes.
        #[arg(long, default_value_t = 1)]
        depth: usize,
    },
    /// tokei-style snapshot of HEAD: lines, code/comments/blanks, funcs,
    /// line age, owner, 90-day churn per lang | dir | file.
    Stats {
        /// Grouping: lang | dir | file.
        #[arg(long, default_value = "lang")]
        by: String,
        /// Directory depth for `--by dir`.
        #[arg(long, default_value_t = 1)]
        depth: usize,
        /// Commit to snapshot (default HEAD), or `A..B` for the delta B − A.
        /// Non-HEAD: no age/owner/churn, grammar-backed files only.
        #[arg(long)]
        rev: Option<String>,
        #[arg(long)]
        lang: Option<String>,
        #[arg(long)]
        path: Option<String>,
        #[arg(long, value_enum)]
        format: Option<FormatArg>,
    },
}

#[derive(clap::Args, Debug)]
struct IndexArgs {
    #[arg(long, value_enum)]
    bucket: Option<BucketArg>,
}

#[derive(clap::Args, Debug, Clone)]
struct QueryArgs {
    #[arg(long)]
    from: Option<String>,
    #[arg(long)]
    to: Option<String>,
    #[arg(long, value_enum)]
    bucket: Option<BucketArg>,
    #[arg(long)]
    lang: Option<String>,
    #[arg(long)]
    author: Option<String>,
    #[arg(long)]
    path: Option<String>,
    #[arg(long, value_enum)]
    format: Option<FormatArg>,
    #[arg(long)]
    by: Option<String>,
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum BucketArg {
    Auto,
    Commit,
    Day,
    Week,
    Month,
    Tag,
}

impl BucketArg {
    fn resolve(self) -> Option<BucketSize> {
        match self {
            BucketArg::Auto => None,
            BucketArg::Commit => Some(BucketSize::Commit),
            BucketArg::Day => Some(BucketSize::Day),
            BucketArg::Week => Some(BucketSize::Week),
            BucketArg::Month => Some(BucketSize::Month),
            BucketArg::Tag => Some(BucketSize::Tag),
        }
    }
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum FormatArg {
    Tsv,
    Csv,
    Json,
    Table,
}

#[derive(Copy, Clone, Debug)]
enum Format {
    Tsv,
    Csv,
    Json,
    Table,
}

impl FormatArg {
    fn resolve(opt: Option<FormatArg>) -> Format {
        match opt {
            Some(FormatArg::Tsv) => Format::Tsv,
            Some(FormatArg::Csv) => Format::Csv,
            Some(FormatArg::Json) => Format::Json,
            Some(FormatArg::Table) => Format::Table,
            None => {
                if std::io::stdout().is_terminal() {
                    Format::Table
                } else {
                    Format::Tsv
                }
            }
        }
    }
}

#[derive(Copy, Clone, Debug, ValueEnum)]
enum ExportFormat {
    Parquet,
    Csv,
    Json,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();
    let repo = repo::open(&cli.repo)?;
    if repo.is_shallow() {
        eprintln!("warning: shallow clone — history truncated, metrics degrade at the horizon");
    }
    let mut cache = cache::open(repo.cache_path())?;
    // Config load is best-effort — missing/broken user config falls back to
    // defaults so `git-arch` still runs.
    let cfg = config::load().unwrap_or_else(|_| config::Loaded {
        config: config::Config::default(),
        aliases: config::Aliases::default(),
    });
    let cfg_bucket = cfg.default_bucket();

    match cli.cmd {
        Cmd::Index(args) => run_index(&repo, &mut cache, args, cfg_bucket, false),
        Cmd::Reindex(args) => run_index(&repo, &mut cache, args, cfg_bucket, true),
        Cmd::Export { format, dir } => {
            ensure_indexed(&repo, &mut cache, cfg_bucket, false)?;
            run_export(&mut cache, format, &dir)
        }
        Cmd::Sql { query, format } => {
            ensure_indexed(&repo, &mut cache, cfg_bucket, false)?;
            run_sql(&cache, &query, FormatArg::resolve(format))
        }
        Cmd::Classify(q) => {
            ensure_indexed(&repo, &mut cache, pick_bucket(&q, cfg_bucket), false)?;
            let filters = build_filters(&q)?;
            let table = query::classify(&cache, &filters)?;
            render_table(&table, FormatArg::resolve(q.format))
        }
        Cmd::Age(q) => {
            ensure_indexed(&repo, &mut cache, pick_bucket(&q, cfg_bucket), false)?;
            let filters = build_filters(&q)?;
            let table = query::age(&cache, &filters)?;
            render_table(&table, FormatArg::resolve(q.format))
        }
        Cmd::Churn { q, depth } => {
            ensure_indexed(&repo, &mut cache, pick_bucket(&q, cfg_bucket), false)?;
            if depth == 0 {
                bail!("--depth must be >= 1");
            }
            let by = q.by.clone().unwrap_or_else(|| "module".to_string());
            let filters = build_filters(&q)?;
            let table = query::churn(&cache, &filters, &by, depth)?;
            render_table(&table, FormatArg::resolve(q.format))
        }
        Cmd::Coupling {
            q,
            top,
            max_files_per_commit,
        } => {
            ensure_indexed(&repo, &mut cache, pick_bucket(&q, cfg_bucket), false)?;
            let filters = build_filters(&q)?;
            let table = query::coupling(&cache, &filters, top, max_files_per_commit)?;
            render_table(&table, FormatArg::resolve(q.format))
        }
        Cmd::Burndown(q) => {
            ensure_indexed(&repo, &mut cache, pick_bucket(&q, cfg_bucket), false)?;
            let by = q.by.clone().unwrap_or_else(|| "language".to_string());
            let filters = build_filters(&q)?;
            let table = query::burndown(&cache, &filters, &by)?;
            render_table(&table, FormatArg::resolve(q.format))
        }
        Cmd::Hotspot { q, top } => {
            if q.lang.is_none() {
                bail!("hotspot requires --lang <L>");
            }
            ensure_indexed(&repo, &mut cache, pick_bucket(&q, cfg_bucket), false)?;
            let filters = build_filters(&q)?;
            let table = query::hotspot(&cache, &filters, top)?;
            render_table(&table, FormatArg::resolve(q.format))
        }
        Cmd::Cohort(q) => {
            ensure_indexed(&repo, &mut cache, pick_bucket(&q, cfg_bucket), false)?;
            let filters = build_filters(&q)?;
            let table = query::cohort(&cache, &filters)?;
            render_table(&table, FormatArg::resolve(q.format))
        }
        Cmd::Stats {
            by,
            depth,
            rev,
            lang,
            path,
            format,
        } => {
            if depth == 0 {
                bail!("--depth must be >= 1");
            }
            let filters =
                query::Filters::parse(None, None, lang.as_deref(), None, path.as_deref())?;
            // `A..B` (git semantics: empty side = HEAD) → delta; else one commit.
            let (base, tip) = match rev.as_deref() {
                Some(r) if r.contains("...") => bail!("--rev takes A..B, not A...B"),
                Some(r) => match r.split_once("..") {
                    Some((a, b)) => (
                        Some(index::resolve_rev(
                            &repo,
                            if a.is_empty() { "HEAD" } else { a },
                        )?),
                        index::resolve_rev(&repo, if b.is_empty() { "HEAD" } else { b })?,
                    ),
                    None => (None, index::resolve_rev(&repo, r)?),
                },
                None => (None, index::resolve_rev(&repo, "HEAD")?),
            };
            let table = if base.is_none() && tip == index::resolve_rev(&repo, "HEAD")? {
                ensure_indexed(&repo, &mut cache, cfg_bucket, false)?;
                index::snapshot_head(&repo, &cache)?;
                query::stats(&cache, &filters, &by, depth)?
            } else {
                index::snapshot(&repo, &cache, Some(&tip), "rev")?;
                index::snapshot(&repo, &cache, base.as_deref(), "base")?;
                query::stats_rev(&cache, &filters, &by, depth, base.is_some())?
            };
            render_table(&table, FormatArg::resolve(format))
        }
        Cmd::Survival { q, fit } => {
            ensure_indexed(&repo, &mut cache, pick_bucket(&q, cfg_bucket), false)?;
            let filters = build_filters(&q)?;
            let fit_exp = match fit.as_deref() {
                None => false,
                Some(s) if s.eq_ignore_ascii_case("exp") => true,
                Some(other) => bail!("--fit must be 'exp', got {other:?}"),
            };
            let table = query::survival(&cache, &filters, fit_exp)?;
            render_table(&table, FormatArg::resolve(q.format))
        }
    }
}

/// Resolve bucket override for a query subcommand.
/// Priority: CLI `--bucket` > config `default_bucket` > None (auto).
fn pick_bucket(q: &QueryArgs, cfg: Option<BucketSize>) -> Option<BucketSize> {
    q.bucket.and_then(BucketArg::resolve).or(cfg)
}

fn build_filters(q: &QueryArgs) -> Result<query::Filters> {
    query::Filters::parse(
        q.from.as_deref(),
        q.to.as_deref(),
        q.lang.as_deref(),
        q.author.as_deref(),
        q.path.as_deref(),
    )
}

fn render_table(t: &query::Table, format: Format) -> Result<()> {
    render(&t.headers, &t.rows, format)
}

fn run_index(
    repo: &repo::Repo,
    cache: &mut cache::Cache,
    args: IndexArgs,
    cfg_bucket: Option<BucketSize>,
    force_full: bool,
) -> Result<()> {
    let bucket = args.bucket.and_then(BucketArg::resolve).or(cfg_bucket);
    index::run(
        repo,
        cache,
        index::IndexOptions {
            force_full,
            bucket_override: bucket,
        },
    )
}

fn ensure_indexed(
    repo: &repo::Repo,
    cache: &mut cache::Cache,
    bucket: Option<BucketSize>,
    force_full: bool,
) -> Result<()> {
    let empty: bool = cache
        .conn
        .query_row("SELECT COUNT(*) = 0 FROM commits", [], |r| r.get(0))
        .unwrap_or(true);
    if empty || force_full {
        eprintln!("indexing repo…");
        index::run(
            repo,
            cache,
            index::IndexOptions {
                force_full,
                bucket_override: bucket,
            },
        )?;
    }
    Ok(())
}

fn run_export(
    cache: &mut cache::Cache,
    format: ExportFormat,
    out_dir: &std::path::Path,
) -> Result<()> {
    std::fs::create_dir_all(out_dir).with_context(|| format!("creating {}", out_dir.display()))?;

    for table in TABLES {
        let (ext, opts) = match format {
            ExportFormat::Parquet => ("parquet", "(FORMAT PARQUET)"),
            ExportFormat::Csv => ("csv", "(FORMAT CSV, HEADER)"),
            ExportFormat::Json => ("json", "(FORMAT JSON, ARRAY true)"),
        };
        let path = out_dir.join(format!("{table}.{ext}"));
        let sql = format!(
            "COPY {table} TO '{}' {opts}",
            path.display().to_string().replace('\'', "''")
        );
        cache
            .conn
            .execute_batch(&sql)
            .with_context(|| format!("exporting {table}"))?;
        eprintln!("  wrote {}", path.display());
    }
    Ok(())
}

fn run_sql(cache: &cache::Cache, query: &str, format: Format) -> Result<()> {
    let mut stmt = cache.conn.prepare(query)?;
    let mut rows_iter = stmt.query([])?;

    // DuckDB requires the statement executed (query()) before column
    // metadata is available.
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
    render(&headers, &rows, format)
}

fn value_to_string(v: duckdb::types::ValueRef) -> String {
    use duckdb::types::ValueRef;
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

fn render(headers: &[String], rows: &[Vec<String>], format: Format) -> Result<()> {
    let out = std::io::stdout();
    let mut out = out.lock();
    match format {
        Format::Tsv => {
            let line = |cells: &[String]| {
                cells
                    .iter()
                    .map(|s| tsv_escape(s))
                    .collect::<Vec<_>>()
                    .join("\t")
            };
            writeln!(out, "{}", line(headers))?;
            for r in rows {
                writeln!(out, "{}", line(r))?;
            }
        }
        Format::Csv => {
            writeln!(
                out,
                "{}",
                headers
                    .iter()
                    .map(|s| csv_escape(s))
                    .collect::<Vec<_>>()
                    .join(",")
            )?;
            for r in rows {
                writeln!(
                    out,
                    "{}",
                    r.iter()
                        .map(|s| csv_escape(s))
                        .collect::<Vec<_>>()
                        .join(",")
                )?;
            }
        }
        Format::Json => {
            writeln!(out, "[")?;
            for (i, r) in rows.iter().enumerate() {
                let sep = if i + 1 == rows.len() { "" } else { "," };
                let pairs: Vec<String> = headers
                    .iter()
                    .zip(r.iter())
                    .map(|(k, v)| format!("\"{}\":\"{}\"", json_escape(k), json_escape(v)))
                    .collect();
                writeln!(out, "  {{{}}}{sep}", pairs.join(","))?;
            }
            writeln!(out, "]")?;
        }
        Format::Table => write!(out, "{}", tokei_table(headers, rows))?,
    }
    Ok(())
}

/// tokei look: `=` rules, Title Case headers, numeric columns right-aligned,
/// a trailing `Total` row split off by a `-` rule.
fn tokei_table(headers: &[String], rows: &[Vec<String>]) -> String {
    let headers: Vec<String> = headers.iter().map(|h| title_case(h)).collect();
    let chars = |s: &str| s.chars().count();
    let widths: Vec<usize> = headers
        .iter()
        .enumerate()
        .map(|(i, h)| rows.iter().map(|r| chars(&r[i])).fold(chars(h), usize::max))
        .collect();
    // First column is the row label, even when it parses as a number.
    let numeric: Vec<bool> = (0..headers.len())
        .map(|i| {
            i > 0
                && rows
                    .iter()
                    .all(|r| r[i].is_empty() || r[i].parse::<f64>().is_ok())
        })
        .collect();
    let line = |cells: &[String]| -> String {
        let cells: Vec<String> = cells
            .iter()
            .enumerate()
            .map(|(i, c)| {
                let w = widths[i];
                if numeric[i] {
                    format!("{c:>w$}")
                } else {
                    format!("{c:<w$}")
                }
            })
            .collect();
        format!(" {}\n", cells.join("  ").trim_end())
    };
    let rule = |c: &str| c.repeat(widths.iter().map(|w| w + 2).sum::<usize>()) + "\n";

    let total = rows.last().filter(|r| r[0] == "Total");
    let body = &rows[..rows.len() - total.iter().len()];
    let mut out = rule("=") + &line(&headers) + &rule("=");
    out.extend(body.iter().map(|r| line(r)));
    if let Some(t) = total {
        if !body.is_empty() {
            out += &rule("-");
        }
        out += &line(t);
    }
    out + &rule("=")
}

/// `test_code` → `Test Code`.
fn title_case(s: &str) -> String {
    s.split('_')
        .map(|w| {
            let mut c = w.chars();
            c.next()
                .map_or(String::new(), |f| f.to_uppercase().chain(c).collect())
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Backslash-escape TSV metacharacters (PostgreSQL text-format convention)
/// so one row stays one line. DuckDB `read_csv(delim='\t')` reads it back.
fn tsv_escape(s: &str) -> String {
    if !s.contains(['\\', '\t', '\n', '\r']) {
        return s.to_string();
    }
    s.replace('\\', "\\\\")
        .replace('\t', "\\t")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn csv_escape(s: &str) -> String {
    if s.contains(',') || s.contains('"') || s.contains('\n') {
        format!("\"{}\"", s.replace('"', "\"\""))
    } else {
        s.to_string()
    }
}

fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::{tokei_table, tsv_escape};

    fn cells(row: &[&str]) -> Vec<String> {
        row.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn tokei_table_aligns_and_splits_total() {
        let got = tokei_table(
            &cells(&["lang", "test_code", "owner"]),
            &[
                cells(&["Rust", "1200", "ana"]),
                cells(&["Süß", "", "bo"]),
                cells(&["Total", "1200", "ana"]),
            ],
        );
        let want = "\
=========================
 Lang   Test Code  Owner
=========================
 Rust        1200  ana
 Süß               bo
-------------------------
 Total       1200  ana
=========================
";
        assert_eq!(got, want);
    }

    #[test]
    fn tokei_table_no_rows() {
        let got = tokei_table(&cells(&["bucket"]), &[]);
        assert_eq!(got, "========\n Bucket\n========\n========\n");
    }

    #[test]
    fn tsv_escape_keeps_one_row_per_line() {
        assert_eq!(tsv_escape("plain"), "plain");
        assert_eq!(tsv_escape("a\tb\nc\r\\"), "a\\tb\\nc\\r\\\\");
    }
}
