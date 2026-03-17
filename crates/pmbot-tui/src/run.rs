use std::time::Duration;
use anyhow::Result;
use crossterm::{
    event::{self, Event, KeyCode},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use tokio::sync::{watch, broadcast};

use pmbot_core::messages::{StrategyMetrics, WorldState};
use crate::App;

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
                if key.code == KeyCode::Char('q') || key.code == KeyCode::Esc {
                    let _ = shutdown_tx.send(());
                    break;
                }
            }
        }
    }

    // Teardown terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    Ok(())
}
