use std::collections::HashSet;
use std::io;
use std::sync::Once;
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
    pub shallow: bool,
    pub sort_col: SortCol,
    pub pending_reindex: bool,
    pub status_msg_at: Option<std::time::Instant>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortCol {
    Total,
    Delta,
    Group,
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
    cache: Cache,
    force_reindex: bool,
    bucket_override: Option<BucketSize>,
) -> Result<()> {
    let bucket_override = bucket_override.or_else(|| cfg.default_bucket());

    let empty_cache = query::cache_stats(&cache.conn)
        .map(|s| s.commits == 0)
        .unwrap_or(true);
    let show_progress = empty_cache || force_reindex;

    let (repo, cache) = run_indexer(
        repo,
        cache,
        index::IndexOptions {
            force_full: force_reindex,
            bucket_override,
        },
        show_progress,
    )?;

    let applied_bucket = crate::cache::queries::get_meta(&cache.conn, "bucket_size")?
        .and_then(|s| BucketSize::parse(&s.to_lowercase()))
        .unwrap_or(BucketSize::Week);

    let filters = Filters {
        bucket: applied_bucket,
        view: cfg.default_view(),
        group_by: cfg.default_group(),
        ..Filters::default()
    };

    let unmerged_count = query::unmerged_candidates(&cache.conn)?.len();

    let shallow = repo.is_shallow();

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
        shallow,
        sort_col: SortCol::Total,
        pending_reindex: false,
        status_msg_at: None,
    };

    // Panic hook restores the terminal so an unwrap!/panic! deep in a widget
    // doesn't leave the user in a wedged raw-mode alt-screen after crash.
    install_panic_hook();

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

fn run_indexer(
    repo: Repo,
    cache: Cache,
    opts: index::IndexOptions,
    show_progress: bool,
) -> Result<(Repo, Cache)> {
    if !show_progress {
        let mut cache = cache;
        index::run(&repo, &mut cache, opts, None)?;
        return Ok((repo, cache));
    }
    let mut cache = cache;
    run_index_with_splash(&repo, &mut cache, opts, "indexing repo (first run) — this can take a minute…")?;
    Ok((repo, cache))
}

/// Run the indexer on the current thread and stream a progress bar on stderr
/// from a helper thread that owns the receiver. gix::Repository is not Sync,
/// so we can't share `&Repo` across threads — but the receiver only needs the
/// mpsc rx end, which is trivially Send.
fn run_index_with_splash(
    repo: &Repo,
    cache: &mut Cache,
    opts: index::IndexOptions,
    banner: &str,
) -> Result<()> {
    let (tx, rx) = crossbeam_channel::unbounded::<index::Progress>();

    eprintln!("{banner}");
    let start = std::time::Instant::now();

    let renderer = std::thread::spawn(move || {
        let mut done = 0usize;
        let mut total = 0usize;
        let mut sampled = 0usize;
        loop {
            // Block up to 100ms for the next event; if the channel closes,
            // exit — the indexer has finished (successfully or otherwise).
            match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                Ok(index::Progress::Started { total_commits, sampled: s }) => {
                    total = total_commits;
                    sampled = s;
                }
                Ok(index::Progress::Commit { done: d, total: t }) => {
                    done = d;
                    total = t;
                }
                Ok(index::Progress::Finished) => {
                    done = total;
                    render_indexer_bar(done, total, sampled, start.elapsed());
                    break;
                }
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
            }
            render_indexer_bar(done, total, sampled, start.elapsed());
        }
        eprintln!();
    });

    let result = index::run(repo, cache, opts, Some(tx));
    // Dropping tx (moved into index::run and dropped there) closes the channel.
    let _ = renderer.join();
    result
}

fn render_indexer_bar(done: usize, total: usize, sampled: usize, elapsed: std::time::Duration) {
    let width: usize = 40;
    let pct = if total > 0 {
        (done as f64 / total as f64).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let filled = (width as f64 * pct).round() as usize;
    let bar: String = "█".repeat(filled) + &"░".repeat(width.saturating_sub(filled));
    let secs = elapsed.as_secs();
    // \r rewrites the same line; explicit flush via stderr auto-flush per write.
    eprint!(
        "\r  [{bar}] {done}/{total}  ({:.1}%)  sampled={sampled}  {secs}s   ",
        pct * 100.0
    );
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

        if state.pending_reindex {
            state.pending_reindex = false;
            do_reindex(terminal, state)?;
        }
        expire_status_msg(state);
    }
    Ok(())
}

fn expire_status_msg(state: &mut AppState) {
    if let Some(t) = state.status_msg_at {
        if t.elapsed() > std::time::Duration::from_secs(5) {
            state.status_msg = None;
            state.status_msg_at = None;
        }
    }
}

fn do_reindex<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    state: &mut AppState,
) -> Result<()> {
    // Leave alt-screen so stderr splash renders on the real terminal.
    disable_raw_mode()?;
    execute!(std::io::stdout(), LeaveAlternateScreen)?;

    let opts = index::IndexOptions {
        force_full: true,
        bucket_override: Some(state.filters.bucket),
    };
    let res = run_index_with_splash(&state.repo, &mut state.cache, opts, "reindexing…");

    // Re-enter alt-screen and force a full redraw.
    enable_raw_mode()?;
    execute!(std::io::stdout(), EnterAlternateScreen)?;
    terminal.clear()?;

    res?;
    state.unmerged_count = query::unmerged_candidates(&state.cache.conn)?.len();
    set_status(state, "reindex complete");
    state.dirty = true;
    Ok(())
}

fn set_status(state: &mut AppState, msg: impl Into<String>) {
    state.status_msg = Some(msg.into());
    state.status_msg_at = Some(std::time::Instant::now());
}

fn refresh_data(state: &mut AppState) -> Result<()> {
    state.series = query::series(&state.cache.conn, &state.filters)?;
    state.breakdown = query::breakdown(&state.cache.conn, &state.filters)?;
    // Drop groups with no data — they clutter the table without adding signal.
    state.breakdown.retain(|r| r.total != 0 || r.delta != 0);
    apply_sort(&mut state.breakdown, state.sort_col);
    if state.selected_row >= state.breakdown.len() {
        state.selected_row = state.breakdown.len().saturating_sub(1);
    }
    Ok(())
}

fn apply_sort(rows: &mut [BreakdownRow], col: SortCol) {
    match col {
        SortCol::Total => rows.sort_by_key(|r| std::cmp::Reverse(r.total)),
        SortCol::Delta => rows.sort_by_key(|r| std::cmp::Reverse(r.delta.abs())),
        SortCol::Group => rows.sort_by(|a, b| a.group.cmp(&b.group)),
    }
}

static PANIC_HOOK: Once = Once::new();

fn install_panic_hook() {
    PANIC_HOOK.call_once(|| {
        let prev = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            // Best-effort — either call can legitimately fail (e.g. not in
            // raw mode yet). Swallow so the original panic surfaces.
            let _ = disable_raw_mode();
            let _ = execute!(std::io::stdout(), LeaveAlternateScreen);
            prev(info);
        }));
    });
}

// ─── main-context keys ────────────────────────────────────────────────────

fn handle_key(state: &mut AppState, code: KeyCode) -> Result<()> {
    // Any user input clears the previous status message so it doesn't linger;
    // handlers below re-set it if they still need to say something.
    state.status_msg = None;
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
        KeyCode::Char('M') => {
            state.filters.metric = match state.filters.metric {
                crate::query::Metric::Loc => crate::query::Metric::Churn,
                crate::query::Metric::Churn => crate::query::Metric::Loc,
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
                set_status(state, "no unmerged identity candidates");
            }
        }
        KeyCode::Char('r') => {
            // Deferred: event_loop leaves alt-screen, runs splash, re-enters.
            state.pending_reindex = true;
        }
        KeyCode::Char('?') => {
            state.modal = Some(Modal::Help);
        }
        KeyCode::Char('s') => {
            state.sort_col = match state.sort_col {
                SortCol::Total => SortCol::Delta,
                SortCol::Delta => SortCol::Group,
                SortCol::Group => SortCol::Total,
            };
            apply_sort(&mut state.breakdown, state.sort_col);
            state.selected_row = 0;
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
                set_status(state, "bucket changed — press [r] to reindex if sampling granularity differs");
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
                state.selected_row = 0;
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
                state.selected_row = 0;
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
                state.selected_row = 0;
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
                        set_status(state, "aliases updated — press [r] to reindex");
                        state.modal = None;
                    }
                    Err(e) => {
                        set_status(state, format!("merge failed: {e}"));
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
