use ratatui::layout::{Constraint, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Cell, Row, Table, TableState};
use ratatui::Frame;

use crate::app::AppState;
use crate::ui::palette;

pub fn render(f: &mut Frame, state: &AppState, area: Rect) {
    let header = Row::new(vec!["●", "Group", "Total", "Δ", "% of scope"])
        .style(Style::default().add_modifier(Modifier::BOLD));

    let total_sum: i64 = state.breakdown.iter().map(|r| r.total).sum();

    let rows: Vec<Row> = state
        .breakdown
        .iter()
        .map(|r| {
            let color = palette::color_for(&r.group);
            let share = if total_sum > 0 {
                (r.total as f64 / total_sum as f64) * 100.0
            } else {
                0.0
            };
            Row::new(vec![
                Cell::from("●").style(Style::default().fg(color)),
                Cell::from(r.group.clone()),
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
            Constraint::Percentage(40),
            Constraint::Length(12),
            Constraint::Length(12),
            Constraint::Length(10),
        ],
    )
    .header(header)
    .block(Block::default().borders(Borders::ALL).title(" Breakdown "))
    .row_highlight_style(Style::default().add_modifier(Modifier::REVERSED));

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

fn fmt_delta(n: i64) -> String {
    if n >= 0 {
        format!("+{}", fmt_num(n))
    } else {
        fmt_num(n)
    }
}
