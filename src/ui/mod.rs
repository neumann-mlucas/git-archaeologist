pub mod breakdown;
pub mod chart;
pub mod filters;
pub mod modals;
pub mod palette;

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::AppState;

pub fn render(f: &mut Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Length(4), // filters (2 content + 2 border)
            Constraint::Min(6),    // chart — takes what's left, shrinks with terminal
            Constraint::Length(std::cmp::max(6, breakdown_height(state.breakdown.len()))),
            Constraint::Length(1), // footer
        ])
        .split(f.area());

    render_title(f, state, chunks[0]);
    filters::render(f, state, chunks[1]);
    chart::render(f, state, chunks[2]);
    breakdown::render(f, state, chunks[3]);
    render_footer(f, state, chunks[4]);

    modals::render(f, state);
}

fn breakdown_height(rows: usize) -> u16 {
    // Header (1) + rows + borders (2). Cap so chart still gets space.
    (rows as u16 + 3).clamp(5, 16)
}

fn render_title(f: &mut Frame, state: &AppState, area: ratatui::layout::Rect) {
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
    let unmerged = if state.unmerged_count > 0 {
        format!(" ⚠ {} unmerged ", state.unmerged_count)
    } else {
        String::new()
    };
    let title = format!(
        " git-archaeologist ─ repo: {repo_name} ─ branch: {branch} ─ scope: {scope} {unmerged}"
    );
    let p = Paragraph::new(title).style(Style::default().add_modifier(Modifier::BOLD));
    f.render_widget(p, area);

    // Underline via a bottom-only border block on the same area.
    let block = Block::default().borders(Borders::BOTTOM);
    f.render_widget(block, area);
}

fn render_footer(f: &mut Frame, state: &AppState, area: ratatui::layout::Rect) {
    let hint = if let Some(msg) = &state.status_msg {
        format!(" {msg} ")
    } else {
        " [Tab] group  [Enter] drill  [Bksp] up  [d] delta  [b] bucket  [f] dates  [l] lang  [a] author  [m] merge  [r] reindex  [?] help  [q] quit ".to_string()
    };
    let p = Paragraph::new(hint).style(Style::default().add_modifier(Modifier::DIM));
    f.render_widget(p, area);
}
