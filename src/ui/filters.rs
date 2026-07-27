use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::AppState;
use crate::query::{GroupBy, View};
use crate::ui::panel_block;

pub fn render(f: &mut Frame, state: &AppState, area: Rect, focused: bool) {
    let from = state.filters.from.map(fmt_ts).unwrap_or_else(|| "begin".into());
    let to = state.filters.to.map(fmt_ts).unwrap_or_else(|| "now".into());

    let group = match state.filters.group_by {
        GroupBy::Language => "language",
        GroupBy::Author => "author",
        GroupBy::Module => "module",
    };
    let view = match state.filters.view {
        View::Cumulative => "cumulative",
        View::Delta => "delta",
    };
    let lens = state.filters.lens.label();
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
    let bucket = format!("{:?}", state.filters.bucket).to_lowercase();

    let kv = |k: &str, v: String| -> Vec<Span<'static>> {
        vec![
            Span::styled(
                format!("{k}:"),
                Style::default().fg(Color::DarkGray),
            ),
            Span::styled(v, Style::default().add_modifier(Modifier::BOLD)),
            Span::raw("  "),
        ]
    };

    let mut line1: Vec<Span> = vec![Span::raw(" ")];
    line1.extend(kv("From", from));
    line1.extend(kv("To", to));
    line1.extend(kv("Bucket", bucket));
    line1.extend(kv("Lens", lens.into()));
    line1.extend(kv("View", view.into()));

    let mut line2: Vec<Span> = vec![Span::raw(" ")];
    line2.extend(kv("Group", group.into()));
    line2.extend(kv("Depth", state.filters.module_depth.to_string()));
    line2.extend(kv("Lang", langs));
    line2.extend(kv("Author", authors));

    let p = Paragraph::new(vec![Line::from(line1), Line::from(line2)])
        .block(panel_block(" Filters ", focused));
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
