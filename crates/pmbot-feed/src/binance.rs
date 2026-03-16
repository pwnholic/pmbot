use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use tokio::sync::mpsc;
use tracing::{debug, info};

use pmbot_core::messages::FeedEvent;
use pmbot_core::types::Symbol;

use crate::traits::PriceFeed;
use crate::vol::{VolComputer, VolMethod};

// ---------------------------------------------------------------------------
// BinanceFeed
// ---------------------------------------------------------------------------

/// Binance price feed processor.
///
/// Currently operates as a processing pipeline that can be fed trade data
/// externally. Real WebSocket connectivity will be added in a future phase.
pub struct BinanceFeed {
    symbols: Vec<Symbol>,
    connected: bool,
    events_tx: mpsc::Sender<FeedEvent>,
    vol_computers: HashMap<Symbol, VolComputer>,
}

impl BinanceFeed {
    /// Create a new Binance feed.
    pub fn new(
        symbols: Vec<Symbol>,
        events_tx: mpsc::Sender<FeedEvent>,
        vol_method: VolMethod,
        vol_window: Duration,
    ) -> Self {
        let vol_computers = symbols
            .iter()
            .map(|s| (s.clone(), VolComputer::new(vol_method, vol_window)))
            .collect();

        Self {
            symbols,
            connected: false,
            events_tx,
            vol_computers,
        }
    }

    /// Process an incoming trade: record price, compute vol, send events.
    ///
    /// Sends a `FeedEvent::SpotPrice` for every trade, and a
    /// `FeedEvent::VolUpdate` whenever vol can be computed.
    pub async fn process_trade(
        &mut self,
        symbol: &Symbol,
        price: Decimal,
        ts: DateTime<Utc>,
    ) -> Result<()> {
        // Send spot price event
        let spot_event = FeedEvent::SpotPrice {
            symbol: symbol.clone(),
            price,
            timestamp: ts,
        };
        self.events_tx.send(spot_event).await?;

        debug!(symbol = %symbol, price = %price, "processed trade");

        // Update vol computer and send vol event if available
        if let Some(vc) = self.vol_computers.get_mut(symbol) {
            vc.record(price, ts);

            if let Some(vol) = vc.realized_vol() {
                let vol_event = FeedEvent::VolUpdate {
                    symbol: symbol.clone(),
                    realized_vol: vol,
                    window: Duration::from_secs(0), // placeholder — actual window from config
                };
                self.events_tx.send(vol_event).await?;
            }
        }

        Ok(())
    }

    /// Get the list of tracked symbols.
    pub fn symbols(&self) -> &[Symbol] {
        &self.symbols
    }
}

#[async_trait]
impl PriceFeed for BinanceFeed {
    fn name(&self) -> &str {
        "binance"
    }

    async fn connect(&mut self) -> Result<()> {
        info!(
            symbols = ?self.symbols.iter().map(|s| s.0.as_str()).collect::<Vec<_>>(),
            "connecting Binance feed"
        );
        self.connected = true;
        Ok(())
    }

    async fn disconnect(&mut self) -> Result<()> {
        info!("disconnecting Binance feed");
        self.connected = false;
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.connected
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use rust_decimal_macros::dec;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    #[tokio::test]
    async fn test_binance_feed_connect_disconnect() {
        let (tx, _rx) = mpsc::channel(16);
        let mut feed = BinanceFeed::new(
            vec![Symbol("BTCUSDT".into())],
            tx,
            VolMethod::Ewma,
            Duration::from_secs(60),
        );

        assert!(!feed.is_connected());
        assert_eq!(feed.name(), "binance");

        feed.connect().await.unwrap();
        assert!(feed.is_connected());

        feed.disconnect().await.unwrap();
        assert!(!feed.is_connected());
    }

    #[tokio::test]
    async fn test_binance_feed_process_trade_sends_spot_price() {
        let (tx, mut rx) = mpsc::channel(16);
        let sym = Symbol("BTCUSDT".into());
        let mut feed = BinanceFeed::new(
            vec![sym.clone()],
            tx,
            VolMethod::Rolling,
            Duration::from_secs(60),
        );

        feed.process_trade(&sym, dec!(50000), ts(0)).await.unwrap();

        let event = rx.recv().await.unwrap();
        match event {
            FeedEvent::SpotPrice {
                symbol,
                price,
                timestamp,
            } => {
                assert_eq!(symbol, sym);
                assert_eq!(price, dec!(50000));
                assert_eq!(timestamp, ts(0));
            }
            other => panic!("expected SpotPrice, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_binance_feed_sends_vol_update_after_enough_data() {
        let (tx, mut rx) = mpsc::channel(64);
        let sym = Symbol("BTCUSDT".into());
        let mut feed = BinanceFeed::new(
            vec![sym.clone()],
            tx,
            VolMethod::Rolling,
            Duration::from_secs(60),
        );

        // First trade: only SpotPrice (not enough data for vol)
        feed.process_trade(&sym, dec!(50000), ts(0)).await.unwrap();
        let _spot = rx.recv().await.unwrap();

        // Second trade: SpotPrice + VolUpdate (now have 2 returns... well 2 prices = 1 return)
        // Need 3 prices for rolling vol (2 returns, n>=2 for std dev)
        feed.process_trade(&sym, dec!(50100), ts(1)).await.unwrap();
        let _spot = rx.recv().await.unwrap();
        // Rolling needs n >= 2 returns (3 prices), so no vol yet
        // But we might get one anyway if implementation handles n=1; let's drain

        feed.process_trade(&sym, dec!(49900), ts(2)).await.unwrap();
        let _spot = rx.recv().await.unwrap();

        // After 3 prices (2 returns), we should get a vol update
        let vol_event = rx.recv().await.unwrap();
        match vol_event {
            FeedEvent::VolUpdate {
                symbol,
                realized_vol,
                ..
            } => {
                assert_eq!(symbol, sym);
                assert!(realized_vol > Decimal::ZERO);
            }
            other => panic!("expected VolUpdate, got: {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_binance_feed_symbols() {
        let (tx, _rx) = mpsc::channel(16);
        let syms = vec![Symbol("BTCUSDT".into()), Symbol("ETHUSDT".into())];
        let feed = BinanceFeed::new(syms.clone(), tx, VolMethod::Ewma, Duration::from_secs(60));

        assert_eq!(feed.symbols().len(), 2);
        assert_eq!(feed.symbols()[0], Symbol("BTCUSDT".into()));
        assert_eq!(feed.symbols()[1], Symbol("ETHUSDT".into()));
    }
}
