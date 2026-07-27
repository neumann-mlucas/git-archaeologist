use std::collections::HashSet;
use std::io;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyCode, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use crate::cache::Cache;
use crate::config::Loaded;
use crate::index::{self, bucket::BucketSize};
use crate::query::{self, BreakdownRow, Filters, GroupBy, SeriesPoint, View};
use crate::repo::Repo;
use crate::ui;

pub struct AppState {
    pub repo: Repo,
    pub cache: Cache,
    pub cfg: Loaded,
    pub filters: Filters,
    pub series: Vec<SeriesPoint>,
    pub breakdown: Vec<BreakdownRow>,
    pub selected_row: usize,
    pub should_quit: bool,
    pub dirty: bool,
    pub modal: Option<Modal>,
    pub unmerged_count: usize,
    pub status_msg: Option<String>,
}

/// One of these is active when `modal.is_some()`.
pub enum Modal {
    Bucket {
        cursor: usize,
    },
    DateRange {
        cursor: usize,
    },
    Language {
        items: Vec<String>,
        selected: HashSet<String>,
        cursor: usize,
    },
    Author {
        items: Vec<(i64, String, String)>,
        selected: HashSet<i64>,
        cursor: usize,
    },
    AliasMerge {
        pairs: Vec<(i64, i64)>,
        cursor: usize,
    },
    Help,
}

pub const BUCKET_CHOICES: &[(BucketSize, &str)] = &[
    (BucketSize::Commit, "commit"),
    (BucketSize::Day, "day"),
    (BucketSize::Week, "week"),
    (BucketSize::Month, "month"),
];

pub const DATE_PRESETS: &[(&str, Option<i64>)] = &[
    ("all time", None),
    ("last 7 days", Some(7)),
    ("last 30 days", Some(30)),
    ("last 90 days", Some(90)),
    ("last year", Some(365)),
];

pub fn run(
    repo: Repo,
    cfg: Loaded,
    mut cache: Cache,
    force_reindex: bool,
    bucket_override: Option<String>,
) -> Result<()> {
    let bucket_override = bucket_override.as_deref().and_then(BucketSize::parse);

    index::run(
        &repo,
        &mut cache,
        index::IndexOptions {
            force_full: force_reindex,
            bucket_override,
        },
        None,
    )?;

    let applied_bucket = crate::cache::queries::get_meta(&cache.conn, "bucket_size")?
        .and_then(|s| BucketSize::parse(&s.to_lowercase()))
        .unwrap_or(BucketSize::Week);

    let mut filters = Filters::default();
    filters.bucket = applied_bucket;

    let unmerged_count = query::unmerged_candidates(&cache.conn)?.len();

    let mut state = AppState {
        repo,
        cache,
        cfg,
        filters,
        series: vec![],
        breakdown: vec![],
        selected_row: 0,
        should_quit: false,
        dirty: true,
        modal: None,
        unmerged_count,
        status_msg: None,
    };

    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let result = event_loop(&mut terminal, &mut state);

    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    result
}

fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    state: &mut AppState,
) -> Result<()> {
    while !state.should_quit {
        if state.dirty {
            refresh_data(state)?;
            state.dirty = false;
        }

        terminal.draw(|f| ui::render(f, state))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(k) = event::read()? {
                if k.kind == KeyEventKind::Press {
                    if state.modal.is_some() {
                        handle_modal_key(state, k.code)?;
                    } else {
                        handle_key(state, k.code)?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn refresh_data(state: &mut AppState) -> Result<()> {
    state.series = query::series(&state.cache.conn, &state.filters)?;
    state.breakdown = query::breakdown(&state.cache.conn, &state.filters)?;
    if state.selected_row >= state.breakdown.len() {
        state.selected_row = state.breakdown.len().saturating_sub(1);
    }
    Ok(())
}

// ─── main-context keys ────────────────────────────────────────────────────

fn handle_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    match code {
        KeyCode::Char('q') => state.should_quit = true,
        KeyCode::Tab => {
            state.filters.group_by = match state.filters.group_by {
                GroupBy::Language => GroupBy::Author,
                GroupBy::Author => GroupBy::Module,
                GroupBy::Module => GroupBy::Language,
            };
            state.selected_row = 0;
            state.dirty = true;
        }
        KeyCode::Char('d') => {
            state.filters.view = match state.filters.view {
                View::Cumulative => View::Delta,
                View::Delta => View::Cumulative,
            };
            state.dirty = true;
        }
        KeyCode::Down => {
            if state.selected_row + 1 < state.breakdown.len() {
                state.selected_row += 1;
            }
        }
        KeyCode::Up => {
            state.selected_row = state.selected_row.saturating_sub(1);
        }
        KeyCode::Enter | KeyCode::Right => drill_in(state),
        KeyCode::Backspace | KeyCode::Left => drill_out(state),

        KeyCode::Char('b') => {
            state.modal = Some(Modal::Bucket { cursor: 0 });
        }
        KeyCode::Char('f') => {
            state.modal = Some(Modal::DateRange { cursor: 0 });
        }
        KeyCode::Char('l') => {
            let items = query::list_languages(&state.cache.conn)?;
            let selected = state.filters.languages.iter().cloned().collect();
            state.modal = Some(Modal::Language {
                items,
                selected,
                cursor: 0,
            });
        }
        KeyCode::Char('a') => {
            let items = query::list_authors(&state.cache.conn)?;
            let selected = state.filters.author_ids.iter().copied().collect();
            state.modal = Some(Modal::Author {
                items,
                selected,
                cursor: 0,
            });
        }
        KeyCode::Char('m') => {
            let pairs = query::unmerged_candidates(&state.cache.conn)?;
            if !pairs.is_empty() {
                state.modal = Some(Modal::AliasMerge { pairs, cursor: 0 });
            } else {
                state.status_msg = Some("no unmerged identity candidates".into());
            }
        }
        KeyCode::Char('r') => {
            let bucket_override = Some(state.filters.bucket);
            state.status_msg = Some("reindexing…".into());
            index::run(
                &state.repo,
                &mut state.cache,
                index::IndexOptions {
                    force_full: true,
                    bucket_override,
                },
                None,
            )?;
            state.unmerged_count =
                query::unmerged_candidates(&state.cache.conn)?.len();
            state.status_msg = Some("reindex complete".into());
            state.dirty = true;
        }
        KeyCode::Char('?') => {
            state.modal = Some(Modal::Help);
        }
        _ => {}
    }
    Ok(())
}

// ─── modal-context keys ───────────────────────────────────────────────────

fn handle_modal_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    // Escape / q always closes the modal.
    if matches!(code, KeyCode::Esc | KeyCode::Char('q')) {
        state.modal = None;
        return Ok(());
    }

    let Some(modal) = state.modal.as_mut() else {
        return Ok(());
    };

    match modal {
        Modal::Bucket { cursor } => match code {
            KeyCode::Down => {
                *cursor = (*cursor + 1).min(BUCKET_CHOICES.len() - 1);
            }
            KeyCode::Up => {
                *cursor = cursor.saturating_sub(1);
            }
            KeyCode::Enter => {
                let (size, _) = BUCKET_CHOICES[*cursor];
                state.filters.bucket = size;
                state.status_msg = Some(
                    "bucket changed — press [r] to reindex if data was sampled at a different granularity".into(),
                );
                state.modal = None;
                state.dirty = true;
            }
            _ => {}
        },
        Modal::DateRange { cursor } => match code {
            KeyCode::Down => {
                *cursor = (*cursor + 1).min(DATE_PRESETS.len() - 1);
            }
            KeyCode::Up => {
                *cursor = cursor.saturating_sub(1);
            }
            KeyCode::Enter => {
                let (_, days) = DATE_PRESETS[*cursor];
                apply_date_preset(&mut state.filters, days);
                state.modal = None;
                state.dirty = true;
            }
            _ => {}
        },
        Modal::Language {
            items,
            selected,
            cursor,
        } => match code {
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Char(' ') => {
                if let Some(item) = items.get(*cursor) {
                    if selected.contains(item) {
                        selected.remove(item);
                    } else {
                        selected.insert(item.clone());
                    }
                }
            }
            KeyCode::Char('c') => selected.clear(),
            KeyCode::Enter => {
                state.filters.languages = selected.iter().cloned().collect();
                state.filters.languages.sort();
                state.modal = None;
                state.dirty = true;
            }
            _ => {}
        },
        Modal::Author {
            items,
            selected,
            cursor,
        } => match code {
            KeyCode::Down => *cursor = (*cursor + 1).min(items.len().saturating_sub(1)),
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Char(' ') => {
                if let Some((id, _, _)) = items.get(*cursor) {
                    if selected.contains(id) {
                        selected.remove(id);
                    } else {
                        selected.insert(*id);
                    }
                }
            }
            KeyCode::Char('c') => selected.clear(),
            KeyCode::Enter => {
                state.filters.author_ids = selected.iter().copied().collect();
                state.filters.author_ids.sort();
                state.modal = None;
                state.dirty = true;
            }
            _ => {}
        },
        Modal::AliasMerge { pairs, cursor } => match code {
            KeyCode::Down => *cursor = (*cursor + 1).min(pairs.len().saturating_sub(1)),
            KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Enter => {
                let pair = pairs[*cursor];
                match crate::config::merge_authors(
                    &state.cfg.aliases_path,
                    &state.cache.conn,
                    pair.0,
                    pair.1,
                ) {
                    Ok(_) => {
                        state.status_msg =
                            Some("aliases updated — press [r] to reindex".into());
                        state.modal = None;
                    }
                    Err(e) => {
                        state.status_msg = Some(format!("merge failed: {e}"));
                    }
                }
            }
            _ => {}
        },
        Modal::Help => {}
    }
    Ok(())
}

fn apply_date_preset(filters: &mut Filters, days: Option<i64>) {
    match days {
        None => {
            filters.from = None;
            filters.to = None;
        }
        Some(d) => {
            let now = time::OffsetDateTime::now_utc().unix_timestamp();
            filters.from = Some(now - d * 86_400);
            filters.to = Some(now);
        }
    }
}

fn drill_in(state: &mut AppState) {
    if state.filters.group_by != GroupBy::Module {
        return;
    }
    if let Some(row) = state.breakdown.get(state.selected_row) {
        if row.group == "(root)" {
            return;
        }
        let mut scope = state.filters.path_scope.clone();
        if !scope.is_empty() && !scope.ends_with('/') {
            scope.push('/');
        }
        scope.push_str(&row.group);
        state.filters.path_scope = scope;
        state.selected_row = 0;
        state.dirty = true;
    }
}

fn drill_out(state: &mut AppState) {
    let scope = state.filters.path_scope.trim_end_matches('/');
    if let Some(idx) = scope.rfind('/') {
        state.filters.path_scope = scope[..idx].to_string();
    } else if !scope.is_empty() {
        state.filters.path_scope.clear();
    } else {
        return;
    }
    state.selected_row = 0;
    state.dirty = true;
}
