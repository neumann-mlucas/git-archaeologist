pub mod breakdown;
pub mod chart;
pub mod filters;
pub mod modals;
pub mod palette;

use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::widgets::{Block, Borders};
use ratatui::Frame;

use crate::app::AppState;

pub fn render(f: &mut Frame, state: &AppState) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1), // title
            Constraint::Length(6), // filters
            Constraint::Min(10),   // chart
            Constraint::Length(12),// breakdown
            Constraint::Length(1), // footer
        ])
        .split(f.area());

    render_title(f, state, chunks[0]);
    filters::render(f, state, chunks[1]);
    chart::render(f, state, chunks[2]);
    breakdown::render(f, state, chunks[3]);
    render_footer(f, chunks[4]);
}

fn render_title(f: &mut Frame, state: &AppState, area: ratatui::layout::Rect) {
    let repo_name = state
        .repo
        .root
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("?");
    let branch = state.repo.branch_name().unwrap_or_else(|_| "?".into());
    let scope = if state.filters.path_scope.is_empty() {
        "/".into()
    } else {
        format!("/{}", state.filters.path_scope)
    };
    let title = format!(
        " git-archaeologist — repo: {repo_name} — branch: {branch} — scope: {scope} "
    );
    let block = Block::default().borders(Borders::BOTTOM).title(title);
    f.render_widget(block, area);
}

fn render_footer(f: &mut Frame, area: ratatui::layout::Rect) {
    let hint = " [Tab] group  [Enter] drill  [Bksp] up  [d] delta  [b] bucket  [f] dates  [l] lang  [a] author  [r] reindex  [?] help  [q] quit ";
    let p = ratatui::widgets::Paragraph::new(hint);
    f.render_widget(p, area);
}
