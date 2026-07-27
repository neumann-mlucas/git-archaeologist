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
        } => checklist(
            f,
            " Language filter — [space] toggle, [c] clear, [Enter] apply ",
            items.iter().cloned().collect(),
            selected.iter().cloned().collect(),
            *cursor,
        ),
        Modal::Author {
            items,
            selected,
            cursor,
        } => {
            let labels: Vec<String> = items
                .iter()
                .map(|(_, n, e)| format!("{n} <{e}>"))
                .collect();
            let ids: Vec<String> =
                items.iter().map(|(id, _, _)| id.to_string()).collect();
            let sel: Vec<String> =
                selected.iter().map(|id| id.to_string()).collect();
            checklist_labeled(
                f,
                " Author filter — [space] toggle, [c] clear, [Enter] apply ",
                labels,
                ids,
                sel.into_iter().collect(),
                *cursor,
            )
        }
        Modal::AliasMerge { pairs, cursor } => render_alias_merge(f, state, pairs, *cursor),
        Modal::Help => render_help(f),
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

fn checklist(
    f: &mut Frame,
    title: &str,
    options: Vec<String>,
    selected: Vec<String>,
    cursor: usize,
) {
    let ids: Vec<String> = options.clone();
    checklist_labeled(f, title, options, ids, selected.into_iter().collect(), cursor)
}

fn checklist_labeled(
    f: &mut Frame,
    title: &str,
    labels: Vec<String>,
    ids: Vec<String>,
    selected: std::collections::HashSet<String>,
    cursor: usize,
) {
    let area = centered_rect(60, 60, f.area());
    f.render_widget(Clear, area);

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
    f.render_stateful_widget(list, area, &mut ls);
}

fn render_alias_merge(f: &mut Frame, state: &AppState, pairs: &[(i64, i64)], cursor: usize) {
    let area = centered_rect(70, 60, f.area());
    f.render_widget(Clear, area);

    let names = author_name_map(state);

    let items: Vec<ListItem> = pairs
        .iter()
        .map(|(a, b)| {
            let na = names.get(a).cloned().unwrap_or_default();
            let nb = names.get(b).cloned().unwrap_or_default();
            ListItem::new(format!("  {na}   ⇄   {nb}"))
        })
        .collect();

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(" Merge author aliases — [Enter] merge, [Esc] cancel "),
        )
        .highlight_style(Style::default().add_modifier(Modifier::REVERSED));

    let mut ls = ListState::default();
    ls.select(Some(cursor));
    f.render_stateful_widget(list, area, &mut ls);
}

fn author_name_map(state: &AppState) -> std::collections::HashMap<i64, String> {
    crate::query::list_authors(&state.cache.conn)
        .unwrap_or_default()
        .into_iter()
        .map(|(id, name, email)| (id, format!("{name} <{email}>")))
        .collect()
}

fn render_help(f: &mut Frame) {
    let area = centered_rect(60, 60, f.area());
    f.render_widget(Clear, area);

    let text = vec![
        Line::from(" Navigation "),
        Line::from("   ↑ ↓          move selection"),
        Line::from("   Enter, →     drill into module (when Group=module)"),
        Line::from("   Bksp,  ←     drill out"),
        Line::from(""),
        Line::from(" Views "),
        Line::from("   Tab          cycle Group-by (lang → author → module)"),
        Line::from("   d            toggle cumulative / delta"),
        Line::from(""),
        Line::from(" Filters "),
        Line::from("   b            bucket size"),
        Line::from("   f            date range"),
        Line::from("   l            languages"),
        Line::from("   a            authors"),
        Line::from("   m            merge unmerged identities"),
        Line::from(""),
        Line::from(" Repo "),
        Line::from("   r            force reindex"),
        Line::from(""),
        Line::from(" App "),
        Line::from("   ? / F1       this help"),
        Line::from("   q / Esc      close modal / quit"),
    ];

    let p = Paragraph::new(text)
        .block(Block::default().borders(Borders::ALL).title(" Help "));
    f.render_widget(p, area);
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
