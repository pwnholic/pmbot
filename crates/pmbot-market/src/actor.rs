use std::sync::Arc;

use anyhow::Result;
use chrono::Utc;
use rust_decimal::Decimal;
use tokio::sync::{broadcast, mpsc};
use tracing::{debug, info, warn};

use pmbot_core::config::MarketConfig;
use pmbot_core::messages::MarketEvent;
use pmbot_core::types::{Level, MarketId, MarketInfo, OrderbookSnapshot, TokenId};

use crate::book::LocalBook;
use crate::discovery::{DiscoveryFilters, MarketDiscovery};
use crate::rotation::MarketRotator;
use crate::tracker::PriceTracker;

/// Raw book data received from a WebSocket or other source.
#[derive(Debug, Clone)]
pub enum RawBookMessage {
    /// Full snapshot replacement.
    Snapshot {
        bids: Vec<Level>,
        asks: Vec<Level>,
    },
    /// Incremental delta update.
    Delta {
        bid_updates: Vec<Level>,
        ask_updates: Vec<Level>,
    },
}

/// The Market Actor: manages orderbook state, price tracking, and market rotation.
pub struct MarketActor {
    config: MarketConfig,
    events_tx: broadcast::Sender<MarketEvent>,
    book: LocalBook,
    tracker: PriceTracker,
    rotator: MarketRotator,
}

impl MarketActor {
    /// Create a new MarketActor.
    ///
    /// Returns the actor and a broadcast receiver for market events.
    pub fn new(
        config: MarketConfig,
        events_tx: broadcast::Sender<MarketEvent>,
    ) -> Self {
        let book = LocalBook::new(
            MarketId("pending".into()),
            TokenId("pending".into()),
        );

        let tracker = PriceTracker::new(1000);

        let rotator = MarketRotator::new(
            config.market_type.clone(),
            config.no_trade_zone_secs,
            config.rotation_lookahead_secs,
        );

        Self {
            config,
            events_tx,
            book,
            tracker,
            rotator,
        }
    }

    /// Run the actor loop.
    ///
    /// 1. Discovers a market using the provided discovery trait.
    /// 2. Processes raw book messages from the `book_rx` channel.
    /// 3. Emits `MarketEvent`s on the broadcast channel.
    pub async fn run(
        &mut self,
        discovery: &dyn MarketDiscovery,
        mut book_rx: mpsc::Receiver<RawBookMessage>,
    ) -> Result<()> {
        // Step 1: Discover and select best market
        let filters = DiscoveryFilters {
            min_liquidity: Decimal::from(self.config.min_liquidity),
            min_volume: Decimal::from(self.config.min_volume),
            tags: self.config.tags.clone(),
            market_type: self.config.market_type.clone(),
            keyword: self.config.keyword.clone(),
            active_only: true,
        };

        let raw_markets = discovery.discover(&filters).await?;
        let markets = self.viable_markets(raw_markets);

        if markets.is_empty() {
            warn!("no viable markets available matching filters (all expired or missing end_date)");
            return Err(anyhow::anyhow!("no viable markets available"));
        }

        // Pick the market with the highest liquidity
        let best = markets
            .into_iter()
            .max_by_key(|m| m.liquidity)
            .unwrap();

        info!(
            market_id = %best.id,
            question = %best.question,
            liquidity = %best.liquidity,
            "selected market"
        );

        // Emit initial rotation event so WS consumers can subscribe
        let _ = self.events_tx.send(MarketEvent::MarketRotation {
            old: MarketId("initial".into()),
            new: best.clone(),
        });

        // Initialize book with the selected market's first token
        let token_id = best.token_ids.first().cloned().unwrap_or(TokenId("unknown".into()));
        self.book = LocalBook::new(best.id.clone(), token_id);
        self.rotator.set_current(best);

        let _ = self.events_tx.send(MarketEvent::Connected);

        let mut discovery_interval = tokio::time::interval(std::time::Duration::from_secs(self.config.discovery_interval_secs));
        let mut ping_interval = tokio::time::interval(std::time::Duration::from_secs(10));
        // Fast expiry check — detects expired market between discovery ticks
        let mut expiry_check_interval = tokio::time::interval(std::time::Duration::from_secs(1));
        let mut rotation_pending = false;
        let reqwest_client = reqwest::Client::new();

        // Step 2: Process book messages
        loop {
            tokio::select! {
                _ = expiry_check_interval.tick() => {
                    // Quick check: if current market is expired/expiring, flag for
                    // immediate rotation on the next discovery tick.
                    if self.rotator.should_rotate() && !rotation_pending {
                        info!("expiry check: current market expired or expiring, triggering rotation");
                        rotation_pending = true;
                        // Reset discovery interval so it fires immediately
                        discovery_interval.reset();
                    }
                }
                _ = discovery_interval.tick() => {
                    // Check rotation
                    if self.rotator.should_rotate() || rotation_pending {
                        rotation_pending = false;
                        info!("market rotation needed");
                        let filters = crate::discovery::DiscoveryFilters {
                            min_liquidity: rust_decimal::Decimal::from(self.config.min_liquidity),
                            min_volume: rust_decimal::Decimal::from(self.config.min_volume),
                            tags: self.config.tags.clone(),
                            market_type: self.config.market_type.clone(),
                            keyword: self.config.keyword.clone(),
                            active_only: true,
                        };
                        match discovery.discover(&filters).await {
                            Ok(raw_markets) => {
                                // Filter to viable (non-expired, with end_date) markets
                                let mut viable = self.viable_markets(raw_markets);
                                viable.sort_by(|a, b| b.liquidity.cmp(&a.liquidity));

                                if let Some(new_market) = viable.into_iter().next() {
                                    let old_market = self.rotator.current_market().cloned();
                                    
                                    info!(
                                        slug = %new_market.slug,
                                        end_date = ?new_market.end_date,
                                        liquidity = %new_market.liquidity,
                                        "rotating to new market"
                                    );

                                    // Initialize new book
                                    let token_id = new_market.token_ids.first().cloned().unwrap_or(pmbot_core::types::TokenId("unknown".into()));
                                    self.book = crate::book::LocalBook::new(new_market.id.clone(), token_id);
                                    self.rotator.set_current(new_market.clone());
                                    
                                    if let Some(old) = old_market {
                                        let _ = self.events_tx.send(pmbot_core::messages::MarketEvent::MarketRotation {
                                            old: old.id,
                                            new: new_market,
                                        });
                                    }
                                } else {
                                    warn!("rotation needed but no viable markets found — will retry next tick");
                                }
                            }
                            Err(e) => {
                                warn!(%e, "failed to discover new market during rotation");
                            }
                        }
                    }
                }
                _ = ping_interval.tick() => {
                    let start = std::time::Instant::now();
                    // We just do a lightweight HEAD or GET to the polymarket time endpoint
                    let res = reqwest_client.get("https://clob.polymarket.com/time").send().await;
                    match res {
                        Ok(_) => {
                            let latency = start.elapsed();
                            let _ = self.events_tx.send(MarketEvent::LatencyUpdate { latency });
                        }
                        Err(e) => {
                            // Suppress verbose network errors here to avoid spam, just warn once
                            warn!(%e, "failed to ping polymarket clob api");
                        }
                    }
                }
                msg_opt = book_rx.recv() => {
                    match msg_opt {
                        Some(msg) => {
                            let ts = chrono::Utc::now();

                            match msg {
                                RawBookMessage::Snapshot { bids, asks } => {
                                    self.book.apply_snapshot(bids, asks, ts);
                                }
                                RawBookMessage::Delta {
                                    bid_updates,
                                    ask_updates,
                                } => {
                                    self.book.apply_delta(&bid_updates, &ask_updates, ts);
                                }
                            }

                            // Update price tracker with mid price
                            let snapshot = self.book.snapshot();
                            if let Some(mid) = snapshot.mid_price() {
                                let best_bid = snapshot.bids.first().map(|l| l.price).unwrap_or(Decimal::ZERO);
                                let best_ask = snapshot.asks.first().map(|l| l.price).unwrap_or(Decimal::ZERO);
                                info!(
                                    market_id = %snapshot.market_id,
                                    bid = %best_bid,
                                    ask = %best_ask,
                                    mid = %mid,
                                    "Polymarket Book Update"
                                );

                                self.tracker.record(mid, ts);

                                let _ = self.events_tx.send(MarketEvent::PriceChange {
                                    market_id: snapshot.market_id.clone(),
                                    token_id: snapshot.token_id.clone(),
                                    price: mid,
                                });
                            }

                            // Emit book update
                            let _ = self.events_tx.send(MarketEvent::BookUpdate {
                                market_id: snapshot.market_id.clone(),
                                book: Arc::new(snapshot),
                            });
                        }
                        None => {
                            break;
                        }
                    }
                }
            }
        }

        // Channel closed => disconnected
        let _ = self.events_tx.send(MarketEvent::Disconnected);

        Ok(())
    }

    /// Get the current orderbook snapshot.
    pub fn snapshot(&self) -> OrderbookSnapshot {
        self.book.snapshot()
    }

    /// Get the price tracker.
    pub fn tracker(&self) -> &PriceTracker {
        &self.tracker
    }

    /// Get the market rotator.
    pub fn rotator(&self) -> &MarketRotator {
        &self.rotator
    }

    /// Filter markets to only those with enough time remaining to trade.
    /// Rejects markets that are expired, expiring within no_trade_zone_secs,
    /// or missing an end_date entirely (unsafe for time-bounded trading).
    fn viable_markets(&self, markets: Vec<MarketInfo>) -> Vec<MarketInfo> {
        let now = Utc::now();
        let cutoff = chrono::Duration::seconds(self.config.no_trade_zone_secs as i64);

        markets
            .into_iter()
            .filter(|m| {
                if !m.active || m.liquidity <= Decimal::ZERO {
                    debug!(slug = %m.slug, "skipping inactive/zero-liquidity market");
                    return false;
                }
                match m.end_date {
                    Some(end) => {
                        if end <= now + cutoff {
                            debug!(
                                slug = %m.slug,
                                end_date = %end,
                                "skipping expired/expiring market"
                            );
                            false
                        } else {
                            true
                        }
                    }
                    None => {
                        debug!(slug = %m.slug, "skipping market with no end_date");
                        false
                    }
                }
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discovery::MockDiscovery;
    use chrono::Utc;
    use pmbot_core::types::MarketInfo;
    use rust_decimal_macros::dec;

    fn make_market(id: &str) -> MarketInfo {
        MarketInfo {
            id: MarketId(id.into()),
            question: format!("Market {id}?"),
            slug: id.into(),
            outcomes: vec!["Yes".into(), "No".into()],
            token_ids: vec![TokenId(format!("{id}-yes")), TokenId(format!("{id}-no"))],
            condition_id: format!("cond-{id}"),
            neg_risk: false,
            active: true,
            end_date: Some(Utc::now() + chrono::Duration::hours(1)),
            liquidity: dec!(10000),
            volume: dec!(50000),
        }
    }

    #[tokio::test]
    async fn test_actor_discovers_and_processes_snapshot() {
        let config = MarketConfig::default();
        let (events_tx, mut events_rx) = broadcast::channel(32);
        let mut actor = MarketActor::new(config, events_tx);

        let discovery = MockDiscovery::new(vec![make_market("m1")]);
        let (book_tx, book_rx) = mpsc::channel(32);

        // Send a snapshot then close the channel
        book_tx
            .send(RawBookMessage::Snapshot {
                bids: vec![Level { price: dec!(0.50), size: dec!(100) }],
                asks: vec![Level { price: dec!(0.55), size: dec!(100) }],
            })
            .await
            .unwrap();
        drop(book_tx);

        actor.run(&discovery, book_rx).await.unwrap();

        // Should have received Connected, PriceChange, BookUpdate, Disconnected
        let mut connected = false;
        let mut book_update = false;
        let mut disconnected = false;

        while let Ok(event) = events_rx.try_recv() {
            match event {
                MarketEvent::Connected => connected = true,
                MarketEvent::BookUpdate { .. } => book_update = true,
                MarketEvent::Disconnected => disconnected = true,
                _ => {}
            }
        }

        assert!(connected);
        assert!(book_update);
        assert!(disconnected);
    }

    #[tokio::test]
    async fn test_actor_fails_with_no_markets() {
        let config = MarketConfig::default();
        let (events_tx, _events_rx) = broadcast::channel(32);
        let mut actor = MarketActor::new(config, events_tx);

        let discovery = MockDiscovery::new(vec![]);
        let (_book_tx, book_rx) = mpsc::channel(32);

        let result = actor.run(&discovery, book_rx).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_actor_processes_delta() {
        let config = MarketConfig::default();
        let (events_tx, _events_rx) = broadcast::channel(32);
        let mut actor = MarketActor::new(config, events_tx);

        let discovery = MockDiscovery::new(vec![make_market("m1")]);
        let (book_tx, book_rx) = mpsc::channel(32);

        // Send snapshot then delta
        book_tx
            .send(RawBookMessage::Snapshot {
                bids: vec![Level { price: dec!(0.50), size: dec!(100) }],
                asks: vec![Level { price: dec!(0.55), size: dec!(100) }],
            })
            .await
            .unwrap();
        book_tx
            .send(RawBookMessage::Delta {
                bid_updates: vec![Level { price: dec!(0.51), size: dec!(50) }],
                ask_updates: vec![],
            })
            .await
            .unwrap();
        drop(book_tx);

        actor.run(&discovery, book_rx).await.unwrap();

        // After processing, the book should have 2 bid levels
        let snap = actor.snapshot();
        assert_eq!(snap.bids.len(), 2);
        assert_eq!(snap.bids[0].price, dec!(0.51));
    }
}
