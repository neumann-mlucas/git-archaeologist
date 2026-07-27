use std::collections::BTreeMap;

use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::symbols;
use ratatui::text::Span;
use ratatui::widgets::{Axis, Block, Borders, Chart, Dataset, GraphType, Paragraph};
use ratatui::Frame;

use crate::app::AppState;
use crate::ui::palette;

pub fn render(f: &mut Frame, state: &AppState, area: Rect) {
    let block = Block::default().borders(Borders::ALL).title(" Evolution ");

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

    // Ensure non-zero axis ranges so single-point series still render.
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

    let datasets: Vec<Dataset> = points
        .iter()
        .map(|(name, pts)| {
            let color = palette::color_for(name);
            // Dot marker is much more visible than Braille on sparse data.
            Dataset::default()
                .name(name.clone())
                .marker(symbols::Marker::Dot)
                .graph_type(GraphType::Line)
                .style(Style::default().fg(color))
                .data(pts)
        })
        .collect();

    let x_axis = Axis::default()
        .bounds([x_min, x_max])
        .labels(vec![
            Span::raw(fmt_bucket(x_min as i64, state.filters.bucket)),
            Span::raw(fmt_bucket(x_max as i64, state.filters.bucket)),
        ]);

    let y_axis = Axis::default()
        .bounds([0.0, y_max])
        .labels(vec![
            Span::raw("0"),
            Span::raw(compact_num(y_max as i64)),
        ]);

    let chart = Chart::new(datasets).block(block).x_axis(x_axis).y_axis(y_axis);
    f.render_widget(chart, area);
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
