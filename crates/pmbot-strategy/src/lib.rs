//! pmbot-strategy: Trading strategy implementations.
//!
//! This crate provides:
//! - [`Strategy`] trait — the core abstraction for signal generation
//! - [`WorldStateBuilder`] — accumulates events into immutable snapshots
//! - [`StrategyRegistry`] — stores and retrieves strategies by name
//! - [`StrategyActor`] — tick-driven evaluation loop
//! - [`LeadLag`] — lead-lag strategy based on BTC price movements
//! - [`FairValue`] — binary option pricing via Black-Scholes
//! - [`FlashCrash`] — mean reversion on sharp drops
//! - [`BookImbalance`] — order flow imbalance with momentum confirmation
//! - [`NegRiskArb`] — NegRisk sum deviation arbitrage
//! - [`Convergence`] — near-expiry convergence to terminal values
//! - [`MarketMaker`] — two-sided market making with inventory skew

pub mod actor;
pub mod book_imbalance;
pub mod context;
pub mod convergence;
pub mod fair_value;
pub mod flash_crash;
pub mod lead_lag;
pub mod market_maker;
pub mod negrisk_arb;
pub mod registry;
pub mod traits;

pub use actor::StrategyActor;
pub use book_imbalance::BookImbalance;
pub use context::WorldStateBuilder;
pub use convergence::Convergence;
pub use fair_value::FairValue;
pub use flash_crash::FlashCrash;
pub use lead_lag::LeadLag;
pub use market_maker::MarketMaker;
pub use negrisk_arb::NegRiskArb;
pub use registry::StrategyRegistry;
pub use traits::Strategy;
