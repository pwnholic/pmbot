use std::collections::HashMap;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

use pmbot_core::messages::FeedEvent;
use pmbot_core::types::Symbol;

use crate::vol::{VolComputer, VolMethod};

// ---------------------------------------------------------------------------
// Raw trade message — the input to the FeedActor
// ---------------------------------------------------------------------------

/// A raw trade message from an external feed.
#[derive(Debug, Clone)]
pub struct RawTradeMessage {
    pub symbol: Symbol,
    pub price: Decimal,
    pub timestamp: DateTime<Utc>,
}

// ---------------------------------------------------------------------------
// FeedActor
// ---------------------------------------------------------------------------

/// Actor that receives raw trades, computes volatility, and broadcasts
/// `FeedEvent`s to all downstream consumers.
pub struct FeedActor {
    events_tx: broadcast::Sender<FeedEvent>,
    raw_rx: mpsc::Receiver<RawTradeMessage>,
    vol_computers: HashMap<Symbol, VolComputer>,
    vol_window: Duration,
}

impl FeedActor {
    /// Create a new FeedActor.
    pub fn new(
        events_tx: broadcast::Sender<FeedEvent>,
        raw_rx: mpsc::Receiver<RawTradeMessage>,
        symbols: &[Symbol],
        vol_method: VolMethod,
        vol_window: Duration,
    ) -> Self {
        let vol_computers = symbols
            .iter()
            .map(|s| (s.clone(), VolComputer::new(vol_method, vol_window)))
            .collect();

        Self {
            events_tx,
            raw_rx,
            vol_computers,
            vol_window,
        }
    }

    /// Run the actor loop. Blocks until the raw trade channel closes.
    pub async fn run(mut self) {
        info!("FeedActor started");

        while let Some(msg) = self.raw_rx.recv().await {
            self.handle_raw_trade(msg);
        }

        info!("FeedActor shutting down (raw channel closed)");
    }

    fn handle_raw_trade(&mut self, msg: RawTradeMessage) {
        info!(symbol = %msg.symbol, price = %msg.price, "Binance Trade Update");

        // Broadcast spot price
        let spot_event = FeedEvent::SpotPrice {
            symbol: msg.symbol.clone(),
            price: msg.price,
            timestamp: msg.timestamp,
        };

        if let Err(e) = self.events_tx.send(spot_event) {
            warn!("no feed event subscribers: {e}");
        }

        // Update vol computer and broadcast vol if available
        if let Some(vc) = self.vol_computers.get_mut(&msg.symbol) {
            vc.record(msg.price, msg.timestamp);

            if let Some(vol) = vc.realized_vol() {
                let vol_event = FeedEvent::VolUpdate {
                    symbol: msg.symbol.clone(),
                    realized_vol: vol,
                    window: self.vol_window,
                };

                if let Err(e) = self.events_tx.send(vol_event) {
                    warn!("no feed event subscribers for vol: {e}");
                }
            }
        } else {
            debug!(symbol = %msg.symbol, "no vol computer for symbol (untracked)");
        }
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
    async fn test_feed_actor_broadcasts_spot_price() {
        let (events_tx, mut events_rx) = broadcast::channel(64);
        let (raw_tx, raw_rx) = mpsc::channel(16);

        let sym = Symbol("BTCUSDT".into());
        let actor = FeedActor::new(
            events_tx,
            raw_rx,
            std::slice::from_ref(&sym),
            VolMethod::Ewma,
            Duration::from_secs(60),
        );

        // Spawn actor
        let handle = tokio::spawn(actor.run());

        // Send a raw trade
        raw_tx
            .send(RawTradeMessage {
                symbol: sym.clone(),
                price: dec!(50000),
                timestamp: ts(0),
            })
            .await
            .unwrap();

        // Receive the spot event
        let event = events_rx.recv().await.unwrap();
        match event {
            FeedEvent::SpotPrice {
                symbol, price, ..
            } => {
                assert_eq!(symbol, sym);
                assert_eq!(price, dec!(50000));
            }
            other => panic!("expected SpotPrice, got: {other:?}"),
        }

        // Drop sender to stop the actor
        drop(raw_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_feed_actor_computes_vol_after_enough_data() {
        let (events_tx, mut events_rx) = broadcast::channel(64);
        let (raw_tx, raw_rx) = mpsc::channel(16);

        let sym = Symbol("BTCUSDT".into());
        let actor = FeedActor::new(
            events_tx,
            raw_rx,
            std::slice::from_ref(&sym),
            VolMethod::Rolling,
            Duration::from_secs(60),
        );

        let handle = tokio::spawn(actor.run());

        // Send 3 trades (need 3 prices for 2 returns for rolling vol)
        for (i, price) in [dec!(50000), dec!(50100), dec!(49900)].iter().enumerate() {
            raw_tx
                .send(RawTradeMessage {
                    symbol: sym.clone(),
                    price: *price,
                    timestamp: ts(i as i64),
                })
                .await
                .unwrap();
        }

        // Collect events — we expect: SpotPrice, SpotPrice, SpotPrice, VolUpdate
        // (First trade: spot only; second: spot only (only 1 return); third: spot + vol)
        let mut spot_count = 0;
        let mut vol_count = 0;

        // Drain events with a short timeout
        loop {
            match tokio::time::timeout(Duration::from_millis(100), events_rx.recv()).await {
                Ok(Ok(FeedEvent::SpotPrice { .. })) => spot_count += 1,
                Ok(Ok(FeedEvent::VolUpdate { realized_vol, .. })) => {
                    assert!(realized_vol > Decimal::ZERO);
                    vol_count += 1;
                }
                _ => break,
            }
        }

        assert_eq!(spot_count, 3);
        assert!(vol_count >= 1, "expected at least 1 vol update, got {vol_count}");

        drop(raw_tx);
        handle.await.unwrap();
    }

    #[tokio::test]
    async fn test_feed_actor_handles_untracked_symbol() {
        let (events_tx, mut events_rx) = broadcast::channel(64);
        let (raw_tx, raw_rx) = mpsc::channel(16);

        // Actor only tracks BTCUSDT
        let actor = FeedActor::new(
            events_tx,
            raw_rx,
            &[Symbol("BTCUSDT".into())],
            VolMethod::Ewma,
            Duration::from_secs(60),
        );

        let handle = tokio::spawn(actor.run());

        // Send a trade for untracked symbol
        raw_tx
            .send(RawTradeMessage {
                symbol: Symbol("DOGEUSDT".into()),
                price: dec!(0.15),
                timestamp: ts(0),
            })
            .await
            .unwrap();

        // Should still get SpotPrice (just no vol computation)
        let event = events_rx.recv().await.unwrap();
        match event {
            FeedEvent::SpotPrice { symbol, .. } => {
                assert_eq!(symbol, Symbol("DOGEUSDT".into()));
            }
            other => panic!("expected SpotPrice, got: {other:?}"),
        }

        drop(raw_tx);
        handle.await.unwrap();
    }
}
