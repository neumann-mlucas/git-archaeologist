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
];

#[derive(Parser, Debug)]
#[command(name = "git-archaeologist", bin_name = "git-arch", version, about)]
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
    Churn(QueryArgs),
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
    let mut cache = cache::open(repo.cache_path())?;

    match cli.cmd {
        Cmd::Index(args) => run_index(&repo, &mut cache, args, false),
        Cmd::Reindex(args) => run_index(&repo, &mut cache, args, true),
        Cmd::Export { format, dir } => {
            ensure_indexed(&repo, &mut cache, None, false)?;
            run_export(&mut cache, format, &dir)
        }
        Cmd::Sql { query, format } => {
            ensure_indexed(&repo, &mut cache, None, false)?;
            run_sql(&cache, &query, FormatArg::resolve(format))
        }
        Cmd::Classify(q) => {
            ensure_indexed(&repo, &mut cache, None, false)?;
            let filters = build_filters(&q)?;
            let table = query::classify(&cache, &filters)?;
            render_table(&table, FormatArg::resolve(q.format))
        }
        Cmd::Age(q) => {
            ensure_indexed(&repo, &mut cache, None, false)?;
            let filters = build_filters(&q)?;
            let table = query::age(&cache, &filters)?;
            render_table(&table, FormatArg::resolve(q.format))
        }
        Cmd::Churn(q) => {
            ensure_indexed(&repo, &mut cache, None, false)?;
            let by = q.by.clone().unwrap_or_else(|| "module".to_string());
            let filters = build_filters(&q)?;
            let table = query::churn(&cache, &filters, &by)?;
            render_table(&table, FormatArg::resolve(q.format))
        }
        Cmd::Coupling {
            q,
            top,
            max_files_per_commit,
        } => {
            ensure_indexed(&repo, &mut cache, None, false)?;
            let filters = build_filters(&q)?;
            let table = query::coupling(&cache, &filters, top, max_files_per_commit)?;
            render_table(&table, FormatArg::resolve(q.format))
        }
        Cmd::Burndown(q) => {
            ensure_indexed(&repo, &mut cache, None, false)?;
            let by = q.by.clone().unwrap_or_else(|| "language".to_string());
            let filters = build_filters(&q)?;
            let table = query::burndown(&cache, &filters, &by)?;
            render_table(&table, FormatArg::resolve(q.format))
        }
        Cmd::Hotspot { q, top } => {
            ensure_indexed(&repo, &mut cache, None, false)?;
            let filters = build_filters(&q)?;
            let table = query::hotspot(&cache, &filters, top)?;
            render_table(&table, FormatArg::resolve(q.format))
        }
        Cmd::Cohort(q) => {
            ensure_indexed(&repo, &mut cache, None, false)?;
            let filters = build_filters(&q)?;
            let table = query::cohort(&cache, &filters)?;
            render_table(&table, FormatArg::resolve(q.format))
        }
        Cmd::Survival { q, fit } => {
            ensure_indexed(&repo, &mut cache, None, false)?;
            let filters = build_filters(&q)?;
            let fit_exp = fit.as_deref().map(|s| s.eq_ignore_ascii_case("exp")).unwrap_or(false);
            let table = query::survival(&cache, &filters, fit_exp)?;
            render_table(&table, FormatArg::resolve(q.format))
        }
    }
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
    force_full: bool,
) -> Result<()> {
    index::run(
        repo,
        cache,
        index::IndexOptions {
            force_full,
            bucket_override: args.bucket.and_then(BucketArg::resolve),
        },
        None,
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
            None,
        )?;
    }
    Ok(())
}

fn run_export(
    cache: &mut cache::Cache,
    format: ExportFormat,
    out_dir: &std::path::Path,
) -> Result<()> {
    std::fs::create_dir_all(out_dir)
        .with_context(|| format!("creating {}", out_dir.display()))?;

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
            writeln!(out, "{}", headers.join("\t"))?;
            for r in rows {
                writeln!(out, "{}", r.join("\t"))?;
            }
        }
        Format::Csv => {
            writeln!(out, "{}", headers.iter().map(|s| csv_escape(s)).collect::<Vec<_>>().join(","))?;
            for r in rows {
                writeln!(out, "{}", r.iter().map(|s| csv_escape(s)).collect::<Vec<_>>().join(","))?;
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
        Format::Table => {
            let widths: Vec<usize> = headers
                .iter()
                .enumerate()
                .map(|(i, h)| {
                    let cell = rows.iter().map(|r| r[i].len()).max().unwrap_or(0);
                    h.len().max(cell)
                })
                .collect();
            let sep = |o: &mut dyn std::io::Write| -> std::io::Result<()> {
                for (i, w) in widths.iter().enumerate() {
                    if i > 0 {
                        write!(o, "-+-")?;
                    }
                    write!(o, "{}", "-".repeat(*w))?;
                }
                writeln!(o)
            };
            for (i, h) in headers.iter().enumerate() {
                if i > 0 {
                    write!(out, " | ")?;
                }
                write!(out, "{:<width$}", h, width = widths[i])?;
            }
            writeln!(out)?;
            sep(&mut out)?;
            for r in rows {
                for (i, c) in r.iter().enumerate() {
                    if i > 0 {
                        write!(out, " | ")?;
                    }
                    write!(out, "{:<width$}", c, width = widths[i])?;
                }
                writeln!(out)?;
            }
        }
    }
    Ok(())
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
