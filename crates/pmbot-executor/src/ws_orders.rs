//! WebSocket order tracking — subscribes to real-time order updates from Polymarket.
//!
//! This module provides real-time order status updates via WebSocket instead of
//! polling the REST API. It handles:
//! - Order placements, updates, and cancellations
//! - Partial fill updates
//! - Automatic reconnection on disconnect

use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

use polymarket_client_sdk::clob::ws::{Client, OrderMessage};
use polymarket_client_sdk::clob::ws::types::response::OrderMessageType;
use polymarket_client_sdk::types::B256;

use pmbot_core::types::{MarketId, OrderId, Side};

/// Order status update from WebSocket.
#[derive(Debug, Clone)]
pub enum OrderUpdate {
    /// New order placed.
    Placed {
        order_id: OrderId,
        market_id: MarketId,
        side: Side,
        price: rust_decimal::Decimal,
        size: rust_decimal::Decimal,
    },
    /// Order partially filled.
    PartialFill {
        order_id: OrderId,
        market_id: MarketId,
        filled: rust_decimal::Decimal,
        remaining: rust_decimal::Decimal,
    },
    /// Order fully filled.
    Filled {
        order_id: OrderId,
        market_id: MarketId,
    },
    /// Order cancelled.
    Cancelled {
        order_id: OrderId,
        market_id: MarketId,
    },
}

/// Configuration for the WebSocket order tracker.
#[derive(Debug, Clone)]
pub struct OrderTrackerConfig {
    /// WebSocket endpoint URL.
    pub endpoint: String,
    /// Initial reconnect backoff.
    pub initial_backoff: Duration,
    /// Maximum reconnect backoff.
    pub max_backoff: Duration,
}

impl Default for OrderTrackerConfig {
    fn default() -> Self {
        Self {
            endpoint: "wss://ws-subscriptions-clob.polymarket.com".to_string(),
            initial_backoff: Duration::from_secs(1),
            max_backoff: Duration::from_secs(60),
        }
    }
}

/// Run a persistent WebSocket connection for order updates.
///
/// Reconnects automatically on disconnect with exponential backoff.
/// Sends parsed updates into `update_tx`; exits cleanly on `shutdown`.
///
/// # Arguments
///
/// * `client` - Authenticated Polymarket WebSocket client
/// * `markets` - Market condition IDs to subscribe to
/// * `update_tx` - Channel to send order updates
/// * `shutdown` - Shutdown signal receiver
pub async fn run_order_tracker(
    client: Client<polymarket_client_sdk::auth::state::Authenticated<polymarket_client_sdk::auth::Normal>>,
    markets: Vec<B256>,
    update_tx: mpsc::Sender<OrderUpdate>,
    mut shutdown: broadcast::Receiver<()>,
) {
    if markets.is_empty() {
        info!("no markets configured for order tracking");
        return;
    }

    let config = OrderTrackerConfig::default();
    let mut backoff = config.initial_backoff;

    loop {
        info!(
            markets_count = markets.len(),
            "connecting to Polymarket order WebSocket"
        );

        match subscribe_and_stream(&client, &markets, &update_tx, &mut shutdown).await {
            Ok(()) => {
                info!("order tracker shutting down");
                return;
            }
            Err(e) => {
                error!(
                    error = %e,
                    backoff_secs = backoff.as_secs(),
                    "order tracker error, reconnecting"
                );

                tokio::select! {
                    _ = shutdown.recv() => {
                        info!("order tracker shutdown during backoff");
                        return;
                    }
                    _ = tokio::time::sleep(backoff) => {}
                }

                backoff = (backoff * 2).min(config.max_backoff);
            }
        }
    }
}

/// Subscribe to order updates and stream them to the update channel.
async fn subscribe_and_stream(
    client: &Client<polymarket_client_sdk::auth::state::Authenticated<polymarket_client_sdk::auth::Normal>>,
    markets: &[B256],
    update_tx: &mpsc::Sender<OrderUpdate>,
    shutdown: &mut broadcast::Receiver<()>,
) -> Result<()> {
    let stream = client
        .subscribe_orders(markets.to_vec())
        .context("failed to subscribe to order updates")?;

    info!("connected to Polymarket order WebSocket");

    tokio::pin!(stream);

    loop {
        tokio::select! {
            result = stream.next() => {
                match result {
                    Some(Ok(order_msg)) => {
                        if let Some(update) = parse_order_message(&order_msg) {
                            if update_tx.send(update).await.is_err() {
                                info!("order update channel closed");
                                return Ok(());
                            }
                        }
                    }
                    Some(Err(e)) => {
                        warn!(error = %e, "order stream error");
                    }
                    None => {
                        info!("order stream ended");
                        return Ok(());
                    }
                }
            }
            _ = shutdown.recv() => {
                info!("order tracker shutdown signal received");
                return Ok(());
            }
        }
    }
}

/// Parse an SDK OrderMessage into our OrderUpdate type.
fn parse_order_message(msg: &OrderMessage) -> Option<OrderUpdate> {
    let order_id = OrderId(msg.id.clone());
    let market_id = MarketId(format!("{:?}", msg.market));

    match msg.msg_type.as_ref()? {
        OrderMessageType::Placement => {
            let side = match msg.side {
                polymarket_client_sdk::clob::types::Side::Buy => Side::Buy,
                polymarket_client_sdk::clob::types::Side::Sell => Side::Sell,
                _ => return None,
            };
            Some(OrderUpdate::Placed {
                order_id,
                market_id,
                side,
                price: msg.price,
                size: msg.original_size?,
            })
        }
        OrderMessageType::Update => {
            let filled = msg.size_matched?;
            let original = msg.original_size?;
            let remaining = original - filled;
            Some(OrderUpdate::PartialFill {
                order_id,
                market_id,
                filled,
                remaining,
            })
        }
        OrderMessageType::Cancellation => Some(OrderUpdate::Cancelled {
            order_id,
            market_id,
        }),
        OrderMessageType::Unknown(t) => {
            warn!(msg_type = %t, order_id = %msg.id, "unknown order message type");
            None
        }
        // Handle any future variants added to the non-exhaustive enum
        _ => {
            warn!(order_id = %msg.id, "unhandled order message type variant");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_order_update_variants() {
        // Test that all OrderUpdate variants can be created
        let placed = OrderUpdate::Placed {
            order_id: OrderId("order-123".into()),
            market_id: MarketId("market-1".into()),
            side: Side::Buy,
            price: rust_decimal::Decimal::ONE,
            size: rust_decimal::Decimal::ONE_HUNDRED,
        };

        let partial = OrderUpdate::PartialFill {
            order_id: OrderId("order-456".into()),
            market_id: MarketId("market-2".into()),
            filled: rust_decimal::Decimal::from(50),
            remaining: rust_decimal::Decimal::from(50),
        };

        let filled = OrderUpdate::Filled {
            order_id: OrderId("order-789".into()),
            market_id: MarketId("market-3".into()),
        };

        let cancelled = OrderUpdate::Cancelled {
            order_id: OrderId("order-000".into()),
            market_id: MarketId("market-4".into()),
        };

        // Verifyvariants are constructible
        assert!(matches!(placed, OrderUpdate::Placed { .. }));
        assert!(matches!(partial, OrderUpdate::PartialFill { .. }));
        assert!(matches!(filled, OrderUpdate::Filled { .. }));
        assert!(matches!(cancelled, OrderUpdate::Cancelled { .. }));
    }

    #[test]
    fn test_order_tracker_config_defaults() {
        let config = OrderTrackerConfig::default();
        assert_eq!(config.endpoint, "wss://ws-subscriptions-clob.polymarket.com");
        assert_eq!(config.initial_backoff, Duration::from_secs(1));
        assert_eq!(config.max_backoff, Duration::from_secs(60));
    }
}