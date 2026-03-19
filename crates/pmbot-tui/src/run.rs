use std::time::Duration;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::{watch, broadcast};

use pmbot_core::messages::{StrategyMetrics, WorldState};
use crate::{App, AppMode};

/// Run the TUI rendering loop.
/// Subscribes to backend state updates and listens for 'q' to quit.
pub async fn run_tui(
    mut tui_rx: watch::Receiver<Option<(WorldState, Vec<StrategyMetrics>)>>,
    shutdown_tx: broadcast::Sender<()>,
) -> Result<()> {
    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    let mut app = App::new(200);
    let mut prev_positions_count = 0usize;
    let mut prev_balance = rust_decimal::Decimal::ZERO;

    // Initial draw
    terminal.draw(|f| crate::ui::draw(f, &mut app))?;

    let mut shutdown_rx = shutdown_tx.subscribe();

    loop {
        // Redraw
        terminal.draw(|f| crate::ui::draw(f, &mut app))?;

        // Wait for state updates or timeout
        tokio::select! {
            _ = shutdown_rx.recv() => {
                break;
            }
            Ok(_) = tui_rx.changed() => {
                let current_state = tui_rx.borrow().clone();
                if let Some((world, metrics)) = current_state {
                    // Log position changes
                    let curr_positions = world.positions.len();
                    if curr_positions != prev_positions_count {
                        if curr_positions > prev_positions_count {
                            app.log(crate::LogLevel::Trade, format!("+{} position(s) opened", curr_positions - prev_positions_count));
                        } else if curr_positions < prev_positions_count {
                            app.log(crate::LogLevel::Trade, format!("-{} position(s) closed", prev_positions_count - curr_positions));
                        }
                        prev_positions_count = curr_positions;
                    }

                    // Log significant balance changes
                    let balance_diff = world.balance - prev_balance;
                    if balance_diff.abs() > rust_decimal::Decimal::new(1, 0) {
                        if balance_diff > rust_decimal::Decimal::ZERO {
                            app.log(crate::LogLevel::Info, format!("Balance +${}", balance_diff.round_dp(2)));
                        } else {
                            app.log(crate::LogLevel::Warn, format!("Balance -${}", balance_diff.abs().round_dp(2)));
                        }
                        prev_balance = world.balance;
                    }

                    app.update_world(world);
                    app.update_metrics(metrics);
                    app.record_pnl(app.world.as_ref().map(|w| w.daily_pnl).unwrap_or(rust_decimal::Decimal::ZERO));
                }
            }
            // Add a timeout fallback in `select!` just to keep polling keys at 10Hz
            _ = tokio::time::sleep(Duration::from_millis(100)) => {}
        }

        // Poll non-blocking for keyboard input
        if event::poll(Duration::from_millis(0))? {
            if let Event::Key(key) = event::read()? {
                handle_key_event(key.code, key.modifiers, &mut app, &shutdown_tx);
            }
        }
    }

    // Teardown terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}

/// Handle keyboard input based on current mode.
fn handle_key_event(
    code: KeyCode,
    modifiers: KeyModifiers,
    app: &mut App,
    shutdown_tx: &broadcast::Sender<()>,
) {
    match app.mode {
        AppMode::Dashboard => handle_dashboard_input(code, modifiers, app, shutdown_tx),
        AppMode::Search => handle_search_input(code, app),
        AppMode::Filter => handle_filter_input(code, app),
    }
}

/// Handle input in dashboard mode.
fn handle_dashboard_input(
    code: KeyCode,
    modifiers: KeyModifiers,
    app: &mut App,
    shutdown_tx: &broadcast::Sender<()>,
) {
    match (code, modifiers) {
        (KeyCode::Char('q'), _) | (KeyCode::Esc, _) => {
            let _ = shutdown_tx.send(());
        }
        (KeyCode::Char('/'), _) => {
            app.mode = AppMode::Search;
            app.search_state.focused = true;
            // Load markets from WorldState
            app.apply_filters_to_markets();
        }
        (KeyCode::Char('f'), _) => {
            app.mode = AppMode::Filter;
        }
        _ => {}
    }
}

/// Handle input in search mode.
fn handle_search_input(code: KeyCode, app: &mut App) {
    match code {
        KeyCode::Esc => {
            if app.search_state.focused {
                app.search_state.focused = false;
            } else {
                app.mode = AppMode::Dashboard;
            }
        }
        KeyCode::Char('/') => {
            app.search_state.focused = !app.search_state.focused;
        }
        KeyCode::Enter => {
            // TODO: Trigger search with current query/category
            // For now, just unfocus
            app.search_state.focused = false;
        }
        KeyCode::Up => {
            app.search_state.cursor_up();
        }
        KeyCode::Down => {
            app.search_state.cursor_down();
        }
        KeyCode::Left => {
            if !app.search_state.focused {
                app.search_state.prev_category();
            }
        }
        KeyCode::Right => {
            if !app.search_state.focused {
                app.search_state.next_category();
            }
        }
        KeyCode::Backspace => {
            if app.search_state.focused {
                app.search_state.pop_char();
            }
        }
        KeyCode::Char(c) => {
            if app.search_state.focused {
                app.search_state.push_char(c);
            }
        }
        _ => {}
    }
}

/// Handle input in filter mode.
fn handle_filter_input(code: KeyCode, app: &mut App) {
    match code {
        KeyCode::Esc | KeyCode::Char('q') => {
            if app.filter_menu.state.editing_field.is_some() {
                app.filter_menu.state.editing_field = None;
                app.filter_menu.state.input_buffer.clear();
            } else {
                app.mode = AppMode::Dashboard;
            }
        }
        KeyCode::Left => {
            app.filter_menu.prev_tab();
        }
        KeyCode::Right => {
            app.filter_menu.next_tab();
        }
        KeyCode::Up => {
            app.filter_menu.cursor_up();
        }
        KeyCode::Down => {
            app.filter_menu.cursor_down();
        }
        KeyCode::Enter => {
            app.filter_menu.select_current();
        }
        KeyCode::Char(' ') => {
            app.filter_menu.toggle_strategy();
        }
        KeyCode::Char('a') => {
            app.apply_filters_to_markets();
            app.mode = AppMode::Search;
        }
        KeyCode::Char('d') => {
            app.filter_menu.remove_tag();
        }
        KeyCode::Backspace => {
            app.filter_menu.handle_backspace();
        }
        KeyCode::Char(c) => {
            app.filter_menu.handle_char(c);
        }
        _ => {}
    }
}
