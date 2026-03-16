//! pmbot-tui: Terminal dashboard for the Polymarket trading bot.
//!
//! Provides:
//! - [`App`] — TUI application state
//! - [`AppLayout`] — adaptive grid layout
//! - Widget modules for orderbook, positions, log, market, strategy, PnL

pub mod app;
pub mod layout;
pub mod widgets;

pub use app::{App, LogEntry, LogLevel};
pub use layout::AppLayout;
