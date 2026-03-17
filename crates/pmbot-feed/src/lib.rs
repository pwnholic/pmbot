//! pmbot-feed: External price feeds and volatility computation.
//!
//! This crate provides:
//! - [`FeedActor`] that broadcasts [`FeedEvent`]s to downstream consumers
//! - [`VolComputer`] for realized volatility (EWMA, Rolling, Parkinson)

pub mod actor;
pub mod vol;
pub mod ws;

pub use actor::{FeedActor, RawFeedMessage};
pub use vol::{VolComputer, VolMethod};
pub use ws::run_binance_ws;
