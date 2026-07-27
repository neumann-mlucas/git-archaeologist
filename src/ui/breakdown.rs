use ratatui::layout::{Constraint, Flex, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::app::{AppState, SortCol};
use crate::ui::{palette, panel_block};

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
        Cell::from("% of scope"),
    ])
    .style(Style::default().add_modifier(Modifier::BOLD));

    // Delta view can produce negative totals; using signed sum for the share
    // denominator flips signs and makes >100% percentages. Use absolute values.
    let total_sum: i64 = state.breakdown.iter().map(|r| r.total.abs()).sum();
    let palette_kind = state.cfg.palette_kind();

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
            Row::new(vec![
                Cell::from("●").style(Style::default().fg(color)),
                Cell::from(truncate(&r.group, 28)),
                Cell::from(fmt_num(r.total)),
                Cell::from(fmt_delta(r.delta)),
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
