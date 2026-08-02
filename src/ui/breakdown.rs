use std::collections::{BTreeMap, HashMap};

use ratatui::layout::{Constraint, Flex, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::app::{AppState, SortCol};
use crate::ui::{palette, panel_block};

const SPARK_WIDTH: usize = 12;
const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];

pub fn render(f: &mut Frame, state: &AppState, area: Rect, focused: bool) {
    let mark = |c: SortCol, label: &str| {
        if state.sort_col == c {
            format!("{label} ▼")
        } else {
            label.to_string()
        }
    };

    let header = Row::new(vec![
        Cell::from("●"),
        Cell::from(mark(SortCol::Group, "Group")),
        Cell::from(mark(SortCol::Total, "Total")),
        Cell::from(mark(SortCol::Delta, "Δ")),
        Cell::from("Trend"),
        Cell::from("% of scope"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    // Delta view can produce negative totals; using signed sum for the share
    // denominator flips signs and makes >100% percentages. Use absolute values.
    let total_sum: i64 = state.breakdown.iter().map(|r| r.total.abs()).sum();
    let palette_kind = state.cfg.palette_kind();
    let trends = per_group_sparklines(state);

    let rows: Vec<Row> = state
        .breakdown
        .iter()
        .map(|r| {
            let color = palette::color_for(&r.group, palette_kind);
            let share = if total_sum > 0 {
                (r.total.abs() as f64 / total_sum as f64) * 100.0
            } else {
                0.0
            };
            let trend = trends.get(&r.group).cloned().unwrap_or_default();
            Row::new(vec![
                Cell::from("●").style(Style::default().fg(color)),
                Cell::from(truncate(&r.group, 28)),
                Cell::from(fmt_num(r.total)),
                Cell::from(fmt_delta(r.delta)),
                Cell::from(trend),
                Cell::from(format!("{share:.1}%")),
            ])
        })
        .collect();

    let mut table_state = TableState::default();
    table_state.select(Some(state.selected_row));

    let table = Table::new(
        rows,
        [
            Constraint::Length(2),
            Constraint::Length(30),
            Constraint::Length(14),
            Constraint::Length(14),
            Constraint::Length(SPARK_WIDTH as u16),
            Constraint::Length(11),
        ],
    )
    .flex(Flex::Start)
    .column_spacing(2)
    .header(header)
    .block(panel_block(" Breakdown ", focused))
    .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    f.render_stateful_widget(table, area, &mut table_state);
}

/// Build a per-group tail sparkline from the current series. Uses the last
/// `SPARK_WIDTH` buckets of each group; single-bucket groups render nothing.
fn per_group_sparklines(state: &AppState) -> HashMap<String, String> {
    let mut per_group: HashMap<String, BTreeMap<i64, i64>> = HashMap::new();
    for p in &state.series {
        per_group
            .entry(p.group.clone())
            .or_default()
            .insert(p.bucket, p.value);
    }
    per_group
        .into_iter()
        .map(|(g, m)| (g, sparkline(m.values().copied())))
        .collect()
}

fn sparkline<I: IntoIterator<Item = i64>>(values: I) -> String {
    let vs: Vec<i64> = values.into_iter().collect();
    if vs.len() < 2 {
        return String::new();
    }
    let tail: &[i64] = if vs.len() > SPARK_WIDTH {
        &vs[vs.len() - SPARK_WIDTH..]
    } else {
        &vs
    };
    let min = *tail.iter().min().unwrap();
    let max = *tail.iter().max().unwrap();
    if min == max {
        // Flat line — render middle-block to convey "signal exists, no motion".
        return BLOCKS[3].to_string().repeat(tail.len());
    }
    let span = (max - min) as f64;
    tail.iter()
        .map(|v| {
            let n = ((*v - min) as f64 / span * (BLOCKS.len() - 1) as f64).round() as usize;
            BLOCKS[n.min(BLOCKS.len() - 1)]
        })
        .collect()
}

fn fmt_num(n: i64) -> String {
    let mut s = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in s.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    s = out.chars().rev().collect();
    if n < 0 {
        format!("-{s}")
    } else {
        s
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let head: String = s.chars().take(max.saturating_sub(1)).collect();
        format!("{head}…")
    }
}

fn fmt_delta(n: i64) -> String {
    if n >= 0 {
        format!("+{}", fmt_num(n))
    } else {
        fmt_num(n)
    }
}
