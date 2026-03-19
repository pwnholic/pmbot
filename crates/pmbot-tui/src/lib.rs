//! pmbot-tui: Terminal dashboard for the Polymarket trading bot.
//!
//! Provides:
//! - [`App`] — TUI application state
//! - [`AppMode`] — View mode (Dashboard, Search, Filter)
//! - [`AppLayout`] — adaptive grid layout
//! - Widget modules for orderbook, positions, log, market, strategy, PnL

pub mod app;
pub mod layout;
pub mod run;
pub mod theme;
pub mod ui;
pub mod widgets;

pub use app::{App, AppMode, LogEntry, LogLevel};
pub use layout::AppLayout;
pub use ui::draw;
