use anyhow::Result;
use async_trait::async_trait;

/// Trait for external price feed providers (Binance, etc.).
#[async_trait]
pub trait PriceFeed: Send + Sync {
    /// Human-readable name of the feed.
    fn name(&self) -> &str;

    /// Connect to the feed source.
    async fn connect(&mut self) -> Result<()>;

    /// Disconnect from the feed source.
    async fn disconnect(&mut self) -> Result<()>;

    /// Whether the feed is currently connected.
    fn is_connected(&self) -> bool;
}
