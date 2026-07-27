pub mod breakdown;
pub mod chart;
pub mod filters;
pub mod modals;
pub mod palette;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, Paragraph};
use ratatui::Frame;

use crate::app::AppState;

/// Lazygit-ish accent for panel borders / titles.
pub const ACCENT: Color = Color::Cyan;
pub const DIM: Color = Color::DarkGray;

pub fn render(f: &mut Frame, state: &AppState) {
    let root = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Length(4), // filters
            Constraint::Min(6),    // chart
            Constraint::Length(std::cmp::max(6, breakdown_height(state.breakdown.len()))),
            Constraint::Length(1), // footer
        ])
        .split(f.area());

    render_title(f, state, root[0]);
    // Only Breakdown is interactive (arrow keys + Enter). Highlight it,
    // dim the passive panels.
    filters::render(f, state, root[1], false);
    chart::render(f, state, root[2], false);
    breakdown::render(f, state, root[3], true);
    render_footer(f, state, root[4]);

    modals::render(f, state);
}

fn breakdown_height(rows: usize) -> u16 {
    (rows as u16 + 3).clamp(5, 16)
}

/// Lazygit style: title always accented, border bright when focused else dim.
pub fn panel_block<'a>(title: &'a str, focused: bool) -> Block<'a> {
    let border_fg = if focused { ACCENT } else { DIM };
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(border_fg))
        .title(Span::styled(
            title,
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ))
}

fn render_title(f: &mut Frame, state: &AppState, area: Rect) {
    let repo_name = state
        .repo
        .root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or_else(|| state.repo.root.to_str().unwrap_or("?"));
    let branch = state.repo.branch_name().unwrap_or_else(|_| "?".into());
    let scope = if state.filters.path_scope.is_empty() {
        "/".into()
    } else {
        format!("/{}", state.filters.path_scope)
    };

    let mut spans = vec![
        Span::styled(
            " git-archaeologist ",
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ),
        Span::raw("│ "),
        Span::styled(repo_name.to_string(), Style::default().add_modifier(Modifier::BOLD)),
        Span::raw("  "),
        Span::styled(branch, Style::default().fg(Color::Magenta)),
        Span::raw("  "),
        Span::styled(scope, Style::default().fg(Color::Yellow)),
    ];
    if state.shallow {
        spans.push(Span::raw("  "));
        spans.push(Span::styled(
            "⚠ shallow clone",
            Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        ));
    }

    let p = Paragraph::new(Line::from(spans));
    f.render_widget(p, area);

    let block = Block::default()
        .borders(Borders::BOTTOM)
        .border_style(Style::default().fg(DIM));
    f.render_widget(block, area);
}

fn render_footer(f: &mut Frame, state: &AppState, area: Rect) {
    if let Some(msg) = &state.status_msg {
        let p = Paragraph::new(format!(" {msg} "))
            .style(Style::default().fg(Color::Yellow));
        f.render_widget(p, area);
        return;
    }

    let hints: &[(&str, &str)] = &[
        ("Tab", "group"),
        ("↵", "drill"),
        ("⌫", "up"),
        ("L", "lens"),
        ("d", "delta"),
        ("s", "sort"),
        ("b", "bucket"),
        ("f", "dates"),
        ("l", "lang"),
        ("a", "author"),
        ("r", "reindex"),
        ("?", "help"),
        ("q", "quit"),
    ];

    let mut spans = vec![Span::raw(" ")];
    for (i, (k, v)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::styled(" · ", Style::default().fg(DIM)));
        }
        spans.push(Span::styled(
            (*k).to_string(),
            Style::default().fg(ACCENT).add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::raw(" "));
        spans.push(Span::styled((*v).to_string(), Style::default().fg(DIM)));
    }

    let p = Paragraph::new(Line::from(spans));
    f.render_widget(p, area);
}
