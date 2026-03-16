//! pmbot-feed: External price feeds and volatility computation.
//!
//! This crate provides:
//! - [`PriceFeed`] trait for price feed providers
//! - [`BinanceFeed`] for Binance trade processing
//! - [`FeedActor`] that broadcasts [`FeedEvent`]s to downstream consumers
//! - [`VolComputer`] for realized volatility (EWMA, Rolling, Parkinson)

pub mod actor;
pub mod binance;
pub mod traits;
pub mod vol;
pub mod ws;

pub use actor::{FeedActor, RawTradeMessage};
pub use binance::BinanceFeed;
pub use traits::PriceFeed;
pub use vol::{VolComputer, VolMethod};
pub use ws::run_binance_ws;
