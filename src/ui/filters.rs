use ratatui::layout::Rect;
use ratatui::widgets::{Block, Borders, Paragraph};
use ratatui::Frame;

use crate::app::AppState;
use crate::query::{GroupBy, Metric, View};

pub fn render(f: &mut Frame, state: &AppState, area: Rect) {
    let from = state
        .filters
        .from
        .map(fmt_ts)
        .unwrap_or_else(|| "begin".into());
    let to = state
        .filters
        .to
        .map(fmt_ts)
        .unwrap_or_else(|| "now".into());

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
    let langs = if state.filters.languages.is_empty() {
        "all".to_string()
    } else {
        state.filters.languages.len().to_string()
    };
    let authors = if state.filters.author_ids.is_empty() {
        "all".to_string()
    } else {
        state.filters.author_ids.len().to_string()
    };

    let bucket = format!("{:?}", state.filters.bucket);

    let text = format!(
        "  From:[{from}]  To:[{to}]  Bucket:[{bucket}]  Metric:[{metric}]  View:[{view}]\n  Group:[{group}]  Depth:[{}]  Lang:[{langs}]  Author:[{authors}]",
        state.filters.module_depth,
    );

    let block = Block::default().borders(Borders::ALL).title(" Filters ");
    let p = Paragraph::new(text).block(block);
    f.render_widget(p, area);
}

fn fmt_ts(unix: i64) -> String {
    time::OffsetDateTime::from_unix_timestamp(unix)
        .ok()
        .and_then(|dt| {
            let fmt = time::macros::format_description!("[year]-[month]-[day]");
            dt.format(&fmt).ok()
        })
        .unwrap_or_else(|| unix.to_string())
}
