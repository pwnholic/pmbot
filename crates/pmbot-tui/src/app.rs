use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use pmbot_core::messages::{StrategyMetrics, WorldState};

/// A log entry for the scrolling log panel.
#[derive(Debug, Clone)]
pub struct LogEntry {
    /// When the log entry was created.
    pub timestamp: DateTime<Utc>,
    /// Severity level.
    pub level: LogLevel,
    /// Human-readable message.
    pub message: String,
}

/// Severity level for log entries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogLevel {
    /// Informational message.
    Info,
    /// Warning condition.
    Warn,
    /// Error condition.
    Error,
    /// Trade execution event.
    Trade,
}

/// TUI application state.
///
/// Holds all data the dashboard needs to render, updated each tick
/// by messages from the bot engine.
pub struct App {
    /// Latest world state snapshot from the engine.
    pub world: Option<WorldState>,
    /// Latest strategy metrics.
    pub strategy_metrics: Vec<StrategyMetrics>,
    /// Ring buffer of log entries.
    pub logs: VecDeque<LogEntry>,
    /// Maximum number of log entries to retain.
    pub log_buffer_size: usize,
    /// PnL history for sparkline rendering.
    pub pnl_history: VecDeque<Decimal>,
    /// Whether the TUI is still running.
    pub running: bool,
    /// Number of ticks processed.
    pub tick_count: u64,
}

impl App {
    /// Create a new App with the given log buffer capacity.
    pub fn new(log_buffer_size: usize) -> Self {
        Self {
            world: None,
            strategy_metrics: Vec::new(),
            logs: VecDeque::with_capacity(log_buffer_size),
            log_buffer_size,
            pnl_history: VecDeque::with_capacity(120),
            running: true,
            tick_count: 0,
        }
    }

    /// Update world state snapshot.
    pub fn update_world(&mut self, world: WorldState) {
        self.world = Some(world);
        self.tick_count += 1;
    }

    /// Update strategy metrics.
    pub fn update_metrics(&mut self, metrics: Vec<StrategyMetrics>) {
        self.strategy_metrics = metrics;
    }

    /// Add a log entry, evicting the oldest if the buffer is full.
    pub fn log(&mut self, level: LogLevel, message: String) {
        if self.logs.len() >= self.log_buffer_size {
            self.logs.pop_front();
        }
        self.logs.push_back(LogEntry {
            timestamp: Utc::now(),
            level,
            message,
        });
    }

    /// Record a PnL data point for the sparkline.
    pub fn record_pnl(&mut self, pnl: Decimal) {
        if self.pnl_history.len() >= 120 {
            self.pnl_history.pop_front();
        }
        self.pnl_history.push_back(pnl);
    }

    /// Signal the application to shut down.
    pub fn quit(&mut self) {
        self.running = false;
    }

    /// Check whether the application is still running.
    pub fn is_running(&self) -> bool {
        self.running
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;
    use std::collections::HashMap;

    fn make_world_state() -> WorldState {
        WorldState {
            active_market_id: None,
            markets: HashMap::new(),
            positions: Vec::new(),
            open_orders: Vec::new(),
            balance: dec!(1000),
            daily_pnl: dec!(50),
            external_prices: HashMap::new(),
            network_latency: HashMap::new(),
            timestamp: Utc::now(),
        }
    }

    #[test]
    fn new_app_starts_running_with_empty_state() {
        let app = App::new(100);
        assert!(app.is_running());
        assert!(app.world.is_none());
        assert!(app.logs.is_empty());
        assert_eq!(app.tick_count, 0);
    }

    #[test]
    fn update_world_increments_tick_count() {
        let mut app = App::new(100);
        app.update_world(make_world_state());
        assert_eq!(app.tick_count, 1);
        assert!(app.world.is_some());

        app.update_world(make_world_state());
        assert_eq!(app.tick_count, 2);
    }

    #[test]
    fn log_buffer_overflow_evicts_oldest() {
        let mut app = App::new(3);
        app.log(LogLevel::Info, "first".into());
        app.log(LogLevel::Info, "second".into());
        app.log(LogLevel::Info, "third".into());
        assert_eq!(app.logs.len(), 3);

        // Adding a fourth should evict "first"
        app.log(LogLevel::Warn, "fourth".into());
        assert_eq!(app.logs.len(), 3);
        assert_eq!(app.logs.front().unwrap().message, "second");
        assert_eq!(app.logs.back().unwrap().message, "fourth");
    }

    #[test]
    fn quit_stops_running() {
        let mut app = App::new(100);
        assert!(app.is_running());
        app.quit();
        assert!(!app.is_running());
    }

    #[test]
    fn record_pnl_respects_capacity() {
        let mut app = App::new(10);
        // Fill beyond internal capacity of 120
        for i in 0..130 {
            app.record_pnl(Decimal::from(i));
        }
        assert_eq!(app.pnl_history.len(), 120);
        // Oldest should be 10 (first 10 were evicted)
        assert_eq!(*app.pnl_history.front().unwrap(), Decimal::from(10));
    }

    #[test]
    fn update_metrics_replaces_previous() {
        let mut app = App::new(10);
        let m1 = vec![StrategyMetrics {
            name: "arb",
            state: "active",
            edge: Some(dec!(0.02)),
            signals_generated: 5,
            custom: vec![],
        }];
        app.update_metrics(m1);
        assert_eq!(app.strategy_metrics.len(), 1);

        let m2 = vec![
            StrategyMetrics {
                name: "arb",
                state: "paused",
                edge: None,
                signals_generated: 10,
                custom: vec![],
            },
            StrategyMetrics {
                name: "mm",
                state: "active",
                edge: Some(dec!(0.01)),
                signals_generated: 3,
                custom: vec![],
            },
        ];
        app.update_metrics(m2);
        assert_eq!(app.strategy_metrics.len(), 2);
        assert_eq!(app.strategy_metrics[0].state, "paused");
    }
}
