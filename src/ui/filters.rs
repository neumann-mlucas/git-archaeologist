use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::AppState;
use crate::query::{GroupBy, Metric, View};

pub fn render(f: &mut Frame, state: &AppState, area: Rect) {
    let f_str = |ts: Option<i64>| {
        ts.map(|_| "…".to_string()).unwrap_or_else(|| "begin".into())
    };
    let t_str = |ts: Option<i64>| {
        ts.map(|_| "…".to_string()).unwrap_or_else(|| "now".into())
    };

    let group = match state.filters.group_by {
        GroupBy::Language => "language",
        GroupBy::Author => "author",
        GroupBy::Module => "module",
    };
    let view = match state.filters.view {
        View::Cumulative => "cumulative",
        View::Delta => "delta",
    };
    let metric = match state.filters.metric {
        Metric::Loc => "LOC",
        Metric::Churn => "churn",
    };

    let text = format!(
        "  From: [{}]   To: [{}]\n  Bucket: [{:?}]   Metric: [{}]   View: [{}]\n  Group-by: [{}]   Depth: [{}]\n  Lang: [{} sel]   Author: [{} sel]",
        f_str(state.filters.from),
        t_str(state.filters.to),
        state.filters.bucket,
        metric,
        view,
        group,
        state.filters.module_depth,
        if state.filters.languages.is_empty() { "all".to_string() } else { state.filters.languages.len().to_string() },
        if state.filters.author_ids.is_empty() { "all".to_string() } else { state.filters.author_ids.len().to_string() },
    );

    let block = Block::default().borders(Borders::ALL).title(" Filters ");
    let p = Paragraph::new(text).block(block);
    f.render_widget(p, area);
}
