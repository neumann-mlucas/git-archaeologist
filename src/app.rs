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
use crate::query::{BreakdownRow, Filters, GroupBy, SeriesPoint, View};
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
}

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

    let mut state = AppState {
        repo,
        cache,
        cfg,
        filters: Filters::default(),
        series: vec![],
        breakdown: vec![],
        selected_row: 0,
        should_quit: false,
        dirty: true,
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
                    handle_key(state, k.code);
                }
            }
        }
    }
    Ok(())
}

fn refresh_data(state: &mut AppState) -> Result<()> {
    state.series = crate::query::series(&state.cache.conn, &state.filters)?;
    state.breakdown = crate::query::breakdown(&state.cache.conn, &state.filters)?;
    if state.selected_row >= state.breakdown.len() {
        state.selected_row = state.breakdown.len().saturating_sub(1);
    }
    Ok(())
}

fn handle_key(state: &mut AppState, code: KeyCode) {
    match code {
        KeyCode::Char('q') => state.should_quit = true,
        KeyCode::Tab => {
            state.filters.group_by = match state.filters.group_by {
                GroupBy::Language => GroupBy::Author,
                GroupBy::Author => GroupBy::Module,
                GroupBy::Module => GroupBy::Language,
            };
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
        KeyCode::Enter => drill_in(state),
        KeyCode::Backspace => drill_out(state),
        // TODO: b/f/l/a/r/? modals
        _ => {}
    }
}

fn drill_in(state: &mut AppState) {
    if state.filters.group_by != GroupBy::Module {
        return;
    }
    if let Some(row) = state.breakdown.get(state.selected_row) {
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
    } else {
        state.filters.path_scope.clear();
    }
    state.selected_row = 0;
    state.dirty = true;
}
