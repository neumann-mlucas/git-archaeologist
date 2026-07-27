use std::collections::BTreeMap;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::symbols;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Axis, Chart, Dataset, GraphType, Paragraph};
use ratatui::Frame;

use crate::app::AppState;
use crate::ui::{palette, panel_block, DIM};

pub fn render(f: &mut Frame, state: &AppState, area: Rect, focused: bool) {
    let lens = state.filters.lens.label();
    let view = match state.filters.view {
        crate::query::View::Cumulative => "cumulative",
        crate::query::View::Delta => "delta",
    };
    let title = format!(" Evolution — {lens} ({view}) ");
    let block = panel_block(&title, focused);

    if state.series.is_empty() {
        let msg = Paragraph::new("  (no data — try widening filters, changing bucket, or [r]eindex)")
            .style(Style::default().add_modifier(Modifier::DIM))
            .block(block);
        f.render_widget(msg, area);
        return;
    }

    let mut by_group: BTreeMap<String, Vec<(f64, f64)>> = BTreeMap::new();
    let mut x_min = f64::INFINITY;
    let mut x_max = f64::NEG_INFINITY;
    let mut y_max: f64 = 0.0;

    for p in &state.series {
        let x = p.bucket as f64;
        let y = p.value as f64;
        by_group.entry(p.group.clone()).or_default().push((x, y));
        x_min = x_min.min(x);
        x_max = x_max.max(x);
        y_max = y_max.max(y);
    }

    if (x_max - x_min).abs() < f64::EPSILON {
        x_min -= 1.0;
        x_max += 1.0;
    }
    if y_max < 1.0 {
        y_max = 1.0;
    }

    let points: Vec<(String, Vec<(f64, f64)>)> = by_group
        .into_iter()
        .map(|(name, mut pts)| {
            pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
            (name, pts)
        })
        .collect();

    // Braille is denser and prettier when there's enough data per series;
    // fall back to Dot when series are sparse so single points don't vanish.
    let avg = points.iter().map(|(_, p)| p.len()).sum::<usize>() as f64
        / points.len().max(1) as f64;
    let marker = if avg >= 3.0 {
        symbols::Marker::Braille
    } else {
        symbols::Marker::Dot
    };

    let palette_kind = state.cfg.palette_kind();

    // Split the chart area horizontally so a legend can sit on the right when
    // width allows. Below ~50 cols we drop the legend entirely.
    let (chart_area, legend_area) = if area.width >= 60 {
        let sp = Layout::default()
            .direction(Direction::Horizontal)
            .constraints([Constraint::Min(20), Constraint::Length(22)])
            .split(area);
        (sp[0], Some(sp[1]))
    } else {
        (area, None)
    };

    let datasets: Vec<Dataset> = points
        .iter()
        .map(|(_name, pts)| {
            let color = palette::color_for(_name, palette_kind);
            // Deliberately no .name(): our sidebar Legend replaces the built-in one.
            Dataset::default()
                .marker(marker)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(color))
                .data(pts)
        })
        .collect();

    // Commit-bucket x-values are unix seconds; when the whole span is under a
    // day, dates alone read the same — show HH:MM instead.
    let span_seconds = (x_max - x_min).abs();
    let use_hhmm = matches!(state.filters.bucket, crate::index::bucket::BucketSize::Commit)
        && span_seconds < 86_400.0;
    let x_lo = if use_hhmm {
        fmt_hhmm(x_min as i64)
    } else {
        fmt_bucket(x_min as i64, state.filters.bucket)
    };
    let x_hi = if use_hhmm {
        fmt_hhmm(x_max as i64)
    } else {
        fmt_bucket(x_max as i64, state.filters.bucket)
    };
    let x_axis = Axis::default()
        .style(Style::default().fg(DIM))
        .bounds([x_min, x_max])
        .labels(vec![
            Span::styled(x_lo, Style::default().fg(DIM)),
            Span::styled(x_hi, Style::default().fg(DIM)),
        ]);

    let y_mid = (y_max / 2.0) as i64;
    let y_axis = Axis::default()
        .style(Style::default().fg(DIM))
        .bounds([0.0, y_max])
        .labels(vec![
            Span::styled("0", Style::default().fg(DIM)),
            Span::styled(compact_num(y_mid), Style::default().fg(DIM)),
            Span::styled(compact_num(y_max as i64), Style::default().fg(DIM)),
        ]);

    let chart = Chart::new(datasets).block(block).x_axis(x_axis).y_axis(y_axis);
    f.render_widget(chart, chart_area);

    if let Some(la) = legend_area {
        render_legend(f, state, la, &points, palette_kind);
    }
}

fn render_legend(
    f: &mut Frame,
    state: &AppState,
    area: Rect,
    points: &[(String, Vec<(f64, f64)>)],
    palette_kind: palette::PaletteKind,
) {
    // Sort legend by breakdown order (already sorted by user's sort choice).
    let order: Vec<String> = state.breakdown.iter().map(|r| r.group.clone()).collect();
    let mut ordered: Vec<&(String, Vec<(f64, f64)>)> = points.iter().collect();
    ordered.sort_by_key(|(name, _)| {
        order.iter().position(|g| g == name).unwrap_or(usize::MAX)
    });

    let mut lines: Vec<Line> = Vec::new();
    let max_rows = area.height.saturating_sub(2) as usize; // borders
    for (name, _) in ordered.iter().take(max_rows) {
        let color = palette::color_for(name, palette_kind);
        let mut label = name.clone();
        if label.chars().count() > 16 {
            label = label.chars().take(15).collect::<String>() + "…";
        }
        lines.push(Line::from(vec![
            Span::styled(" ● ", Style::default().fg(color)),
            Span::raw(label),
        ]));
    }
    if ordered.len() > max_rows {
        lines.push(Line::from(Span::styled(
            format!(" +{} more", ordered.len() - max_rows),
            Style::default().fg(DIM),
        )));
    }

    let block = panel_block(" Legend ", false);
    let p = Paragraph::new(lines).block(block);
    f.render_widget(p, area);
}

fn compact_num(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}k", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

fn fmt_hhmm(unix: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(unix)
        .ok()
        .and_then(|dt| {
            let fmt = time::macros::format_description!("[year]-[month]-[day] [hour]:[minute]");
            dt.format(&fmt).ok()
        })
        .unwrap_or_else(|| unix.to_string())
}

fn fmt_bucket(key: i64, size: crate::index::bucket::BucketSize) -> String {
    use crate::index::bucket::BucketSize::*;
    match size {
        Commit => time::OffsetDateTime::from_unix_timestamp(key)
            .ok()
            .and_then(|dt| {
                let fmt = time::macros::format_description!("[year]-[month]-[day]");
                dt.format(&fmt).ok()
            })
            .unwrap_or_else(|| key.to_string()),
        Day => {
            let y = key / 10_000;
            let m = (key / 100) % 100;
            let d = key % 100;
            format!("{y:04}-{m:02}-{d:02}")
        }
        Week => {
            let y = key / 100;
            let w = key % 100;
            format!("{y}-W{w:02}")
        }
        Month => {
            let y = key / 100;
            let m = key % 100;
            format!("{y:04}-{m:02}")
        }
    }
}
