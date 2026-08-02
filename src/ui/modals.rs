use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, ListState, Paragraph};
use ratatui::Frame;

use crate::app::{AppState, Modal, BUCKET_CHOICES, DATE_PRESETS};

pub fn render(f: &mut Frame, state: &AppState) {
    let Some(modal) = &state.modal else { return };

    match modal {
        Modal::Bucket { cursor } => radio(
            f,
            " Bucket size ",
            BUCKET_CHOICES.iter().map(|(_, s)| (*s).to_string()).collect(),
            *cursor,
            &format!("{:?}", state.filters.bucket).to_lowercase(),
        ),
        Modal::DateRange { cursor } => radio(
            f,
            " Date range ",
            DATE_PRESETS.iter().map(|(l, _)| (*l).to_string()).collect(),
            *cursor,
            "",
        ),
        Modal::Language {
            items,
            selected,
            cursor,
            filter,
        } => {
            let (labels, ids): (Vec<String>, Vec<String>) = items
                .iter()
                .filter(|s| Modal::matches_filter(filter, s))
                .map(|s| (s.clone(), s.clone()))
                .unzip();
            checklist_labeled(
                f,
                " Language filter — [space] toggle, [C] clear, [Enter] apply ",
                labels,
                ids,
                selected.iter().cloned().collect(),
                *cursor,
                filter,
            )
        }
        Modal::Author {
            items,
            selected,
            cursor,
            filter,
        } => {
            let visible: Vec<(String, String)> = items
                .iter()
                .filter_map(|(id, n, e)| {
                    let label = format!("{n} <{e}>");
                    if Modal::matches_filter(filter, &label) {
                        Some((label, id.to_string()))
                    } else {
                        None
                    }
                })
                .collect();
            let (labels, ids): (Vec<String>, Vec<String>) = visible.into_iter().unzip();
            let sel: Vec<String> =
                selected.iter().map(|id| id.to_string()).collect();
            checklist_labeled(
                f,
                " Author filter — [space] toggle, [C] clear, [Enter] apply ",
                labels,
                ids,
                sel.into_iter().collect(),
                *cursor,
                filter,
            )
        }
        Modal::Help => render_help(f, state),
    }
}

fn radio(f: &mut Frame, title: &str, options: Vec<String>, cursor: usize, current: &str) {
    let area = centered_rect(50, 40, f.area());
    f.render_widget(Clear, area);

    let items: Vec<ListItem> = options
        .iter()
        .map(|o| {
            let marker = if o == current { "●" } else { "○" };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" {marker} "), Style::default().fg(Color::Cyan)),
                Span::raw(o.clone()),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut ls = ListState::default();
    ls.select(Some(cursor));
    f.render_stateful_widget(list, area, &mut ls);
}

fn checklist_labeled(
    f: &mut Frame,
    title: &str,
    labels: Vec<String>,
    ids: Vec<String>,
    selected: std::collections::HashSet<String>,
    cursor: usize,
    filter: &str,
) {
    let area = centered_rect(60, 60, f.area());
    f.render_widget(Clear, area);

    // Reserve one row at the bottom of the modal for the filter input.
    let inner = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(3)])
        .split(area);

    let items: Vec<ListItem> = labels
        .iter()
        .zip(ids.iter())
        .map(|(label, id)| {
            let mark = if selected.contains(id) { "✓" } else { " " };
            ListItem::new(Line::from(vec![
                Span::styled(format!(" [{mark}] "), Style::default().fg(Color::Yellow)),
                Span::raw(label.clone()),
            ]))
        })
        .collect();

    let list = List::new(items)
        .block(Block::default().borders(Borders::ALL).title(title))
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut ls = ListState::default();
    ls.select(Some(cursor));
    f.render_stateful_widget(list, inner[0], &mut ls);

    let filter_title = if filter.is_empty() {
        " filter (type to narrow, Backspace, Esc clears) "
    } else {
        " filter "
    };
    let filter_widget = Paragraph::new(Line::from(vec![
        Span::styled(" / ", Style::default().fg(Color::Cyan)),
        Span::raw(filter.to_string()),
    ]))
    .block(Block::default().borders(Borders::ALL).title(filter_title));
    f.render_widget(filter_widget, inner[1]);
}

fn render_help(f: &mut Frame, state: &AppState) {
    let area = centered_rect(65, 75, f.area());
    f.render_widget(Clear, area);

    let stats = crate::query::cache_stats(&state.cache.conn).ok();
    let cache_bytes = std::fs::metadata(state.repo.cache_path())
        .ok()
        .map(|m| m.len())
        .unwrap_or(0);

    let mut text = vec![
        Line::from(" Navigation "),
        Line::from("   ↑ ↓          move selection"),
        Line::from("   Enter, →     drill into module (when Group=module)"),
        Line::from("   Bksp,  ←     drill out"),
        Line::from(""),
        Line::from(" Views "),
        Line::from("   L            cycle lens (structure → activity → ownership)"),
        Line::from("   Tab          cycle Group-by within current lens"),
        Line::from("   d            toggle cumulative / delta"),
        Line::from("   s            cycle sort column"),
        Line::from(""),
        Line::from(" Filters "),
        Line::from("   b            bucket size"),
        Line::from("   f            date range"),
        Line::from("   l            languages"),
        Line::from("   a            authors"),
        Line::from("   , / .        pan date window left / right"),
        Line::from("   - / =        zoom out / in"),
        Line::from(""),
        Line::from(" Repo "),
        Line::from("   r            force reindex"),
        Line::from(""),
        Line::from(" App "),
        Line::from("   ? / F1       this help"),
        Line::from("   q / Esc      close modal / quit"),
        Line::from(""),
        Line::from(" Cache "),
        Line::from(format!("   file          {}", state.repo.cache_path().display())),
        Line::from(format!("   size          {}", fmt_bytes(cache_bytes))),
    ];
    if let Some(s) = stats {
        text.push(Line::from(format!("   commits       {}", s.commits)));
        text.push(Line::from(format!("   file_stats    {}", s.file_stats)));
        text.push(Line::from(format!("   churn rows    {}", s.churn)));
        text.push(Line::from(format!("   authors       {}", s.authors)));
    }

    let p = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title(" Help "));
    f.render_widget(p, area);
}

fn fmt_bytes(n: u64) -> String {
    const K: u64 = 1024;
    if n >= K * K * K {
        format!("{:.2} GiB", n as f64 / (K * K * K) as f64)
    } else if n >= K * K {
        format!("{:.2} MiB", n as f64 / (K * K) as f64)
    } else if n >= K {
        format!("{:.1} KiB", n as f64 / K as f64)
    } else {
        format!("{n} B")
    }
}

fn centered_rect(pct_x: u16, pct_y: u16, area: Rect) -> Rect {
    let vert = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - pct_y) / 2),
            Constraint::Percentage(pct_y),
            Constraint::Percentage((100 - pct_y) / 2),
        ])
        .split(area);

    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - pct_x) / 2),
            Constraint::Percentage(pct_x),
            Constraint::Percentage((100 - pct_x) / 2),
        ])
        .split(vert[1])[1]
}
