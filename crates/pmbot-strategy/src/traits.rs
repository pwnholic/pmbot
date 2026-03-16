//! Strategy trait — the core abstraction for signal generation.
//!
//! Strategies are pure signal generators with no I/O or SDK calls.
//! They receive an immutable [`WorldState`] snapshot every tick and
//! return zero or more [`Signal`]s.

use pmbot_core::messages::{Signal, StrategyMetrics, WorldState};
use pmbot_core::types::{FillEvent, MarketId, MarketInfo};

/// A trading strategy that generates signals from world state.
///
/// All strategies must be `Send + Sync + 'static` so they can be
/// stored in a registry and evaluated on the actor's tick loop.
pub trait Strategy: Send + Sync + 'static {
    /// Unique name of this strategy (e.g., `"lead_lag"`).
    fn name(&self) -> &'static str;

    /// Called every tick. Examine the world state and return signals.
    fn evaluate(&mut self, world: &WorldState) -> Vec<Signal>;

    /// Called when a fill occurs for a signal from this strategy.
    fn on_fill(&mut self, fill: &FillEvent);

    /// Called when the active market rotates. Reset internal state if needed.
    fn on_market_change(&mut self, old: &MarketId, new: &MarketInfo);

    /// Return strategy-specific metrics for the TUI dashboard.
    fn metrics(&self) -> StrategyMetrics;
}
