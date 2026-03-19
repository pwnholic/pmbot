use std::collections::VecDeque;

use chrono::{DateTime, Utc};
use pmbot_core::types::MarketInfo;
use rust_decimal::Decimal;

use crate::widgets::{filter_menu::FilterMenu, market_search::MarketSearchState};
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
    Info,
    Warn,
    Error,
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
    /// PnL history (retained for API compatibility).
    pub pnl_history: VecDeque<Decimal>,
    /// Whether the TUI is still running.
    pub running: bool,
    /// Number of ticks processed.
    pub tick_count: u64,
    /// Current view mode.
    pub mode: AppMode,
    /// Market search state.
    pub search_state: MarketSearchState,
    /// Filter menu state.
    pub filter_menu: FilterMenu,
}

/// Application view mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum AppMode {
    /// Main dashboard view.
    #[default]
    Dashboard,
    /// Market search view.
    Search,
    /// Filter menu view.
    Filter,
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
            mode: AppMode::Dashboard,
            search_state: MarketSearchState::new(),
            filter_menu: FilterMenu::new(),
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

    /// Apply filters from FilterMenu to markets from WorldState.
    /// Updates search_state with filtered markets.
    pub fn apply_filters_to_markets(&mut self) {
        let filters = self.filter_menu.to_filters();

        let selected_category = self.filter_menu.current_category().to_lowercase();
        let markets: Vec<MarketInfo> = if let Some(ref world) = self.world {
            world
                .markets
                .values()
                .filter(|snapshot| {
                    // Filter by category (skip if "all")
                    if selected_category != "all" {
                        let info_category = snapshot.info.category.to_lowercase();
                        if info_category != selected_category {
                            // Also check tags for category match
                            let in_tags = snapshot
                                .info
                                .tags
                                .iter()
                                .any(|t| t.to_lowercase() == selected_category);
                            if !in_tags {
                                return false;
                            }
                        }
                    }

                    // Filter by min_liquidity
                    if filters.min_liquidity > Decimal::ZERO
                        && snapshot.info.liquidity < filters.min_liquidity
                    {
                        return false;
                    }

                    // Filter by exclude_tags
                    for tag in &filters.exclude_tags {
                        let tag_lower = tag.to_lowercase();
                        if snapshot
                            .info
                            .tags
                            .iter()
                            .any(|t: &String| t.to_lowercase() == tag_lower)
                        {
                            return false;
                        }
                    }

                    true
                })
                .map(|snapshot| snapshot.info.clone())
                .collect()
        } else {
            Vec::new()
        };

        self.search_state.set_markets(markets);
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
            trades: 0,
            wins: 0,
            losses: 0,
            total_pnl: rust_decimal::Decimal::ZERO,
            pnl_history: Vec::new(),
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
                trades: 0,
                wins: 0,
                losses: 0,
                total_pnl: rust_decimal::Decimal::ZERO,
                pnl_history: Vec::new(),
                custom: vec![],
            },
            StrategyMetrics {
                name: "mm",
                state: "active",
                edge: Some(dec!(0.01)),
                signals_generated: 3,
                trades: 0,
                wins: 0,
                losses: 0,
                total_pnl: rust_decimal::Decimal::ZERO,
                pnl_history: Vec::new(),
                custom: vec![],
            },
        ];
        app.update_metrics(m2);
        assert_eq!(app.strategy_metrics.len(), 2);
        assert_eq!(app.strategy_metrics[0].state, "paused");
    }
}
