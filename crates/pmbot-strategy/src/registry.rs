//! Strategy registry — stores and retrieves strategies by name.

use std::collections::HashMap;

use crate::traits::Strategy;

/// Registry that holds all active strategies, keyed by name.
pub struct StrategyRegistry {
    strategies: HashMap<&'static str, Box<dyn Strategy>>,
}

impl StrategyRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self {
            strategies: HashMap::new(),
        }
    }

    /// Register a strategy. Overwrites any existing strategy with the same name.
    pub fn register(&mut self, strategy: Box<dyn Strategy>) {
        let name = strategy.name();
        self.strategies.insert(name, strategy);
    }

    /// Look up a strategy by name (immutable).
    pub fn get(&self, name: &str) -> Option<&dyn Strategy> {
        self.strategies.get(name).map(|b| b.as_ref())
    }

    /// Look up a strategy by name (mutable).
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Box<dyn Strategy>> {
        self.strategies.get_mut(name)
    }

    /// Iterate over all registered strategies (mutable).
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Box<dyn Strategy>> {
        self.strategies.values_mut()
    }

    /// Return the names of all registered strategies.
    pub fn names(&self) -> Vec<&'static str> {
        self.strategies.keys().copied().collect()
    }

    /// Number of registered strategies.
    pub fn len(&self) -> usize {
        self.strategies.len()
    }

    /// Whether the registry is empty.
    pub fn is_empty(&self) -> bool {
        self.strategies.is_empty()
    }
}

impl Default for StrategyRegistry {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use pmbot_core::messages::{Signal, StrategyMetrics, WorldState};
    use pmbot_core::types::{FillEvent, MarketId, MarketInfo};

    /// Minimal dummy strategy for testing the registry.
    struct DummyStrategy {
        tag: &'static str,
    }

    impl Strategy for DummyStrategy {
        fn name(&self) -> &'static str {
            self.tag
        }

        fn evaluate(&mut self, _world: &WorldState) -> Vec<Signal> {
            Vec::new()
        }

        fn on_fill(&mut self, _fill: &FillEvent) {}

        fn on_market_change(&mut self, _old: &MarketId, _new: &MarketInfo) {}

        fn metrics(&self) -> StrategyMetrics {
            StrategyMetrics {
                name: self.tag,
                state: "idle",
                edge: None,
                signals_generated: 0,
                trades: 0,
                wins: 0,
                losses: 0,
                total_pnl: rust_decimal::Decimal::ZERO,
                custom: Vec::new(),
            }
        }
    }

    #[test]
    fn test_register_and_lookup() {
        let mut reg = StrategyRegistry::new();
        reg.register(Box::new(DummyStrategy { tag: "alpha" }));

        assert_eq!(reg.len(), 1);
        assert!(!reg.is_empty());
        assert!(reg.get("alpha").is_some());
        assert!(reg.get("beta").is_none());
    }

    #[test]
    fn test_names() {
        let mut reg = StrategyRegistry::new();
        reg.register(Box::new(DummyStrategy { tag: "alpha" }));
        reg.register(Box::new(DummyStrategy { tag: "beta" }));

        let mut names = reg.names();
        names.sort();
        assert_eq!(names, vec!["alpha", "beta"]);
    }

    #[test]
    fn test_iter_mut() {
        let mut reg = StrategyRegistry::new();
        reg.register(Box::new(DummyStrategy { tag: "alpha" }));
        reg.register(Box::new(DummyStrategy { tag: "beta" }));

        let count = reg.iter_mut().count();
        assert_eq!(count, 2);
    }

    #[test]
    fn test_overwrite_on_duplicate_name() {
        let mut reg = StrategyRegistry::new();
        reg.register(Box::new(DummyStrategy { tag: "alpha" }));
        reg.register(Box::new(DummyStrategy { tag: "alpha" }));

        assert_eq!(reg.len(), 1);
    }

    #[test]
    fn test_empty_registry() {
        let reg = StrategyRegistry::new();
        assert!(reg.is_empty());
        assert_eq!(reg.len(), 0);
        assert!(reg.names().is_empty());
    }

    #[test]
    fn test_get_mut() {
        let mut reg = StrategyRegistry::new();
        reg.register(Box::new(DummyStrategy { tag: "alpha" }));

        let strat = reg.get_mut("alpha").unwrap();
        assert_eq!(strat.name(), "alpha");
    }
}
