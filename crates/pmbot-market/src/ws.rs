//! Polymarket CLOB WebSocket consumer.
//!
//! Subscribes to orderbook snapshots for the active market and
//! translates them into internal `RawBookMessage`s.

use std::pin::Pin;
use std::str::FromStr;

use futures::Stream;
use futures::StreamExt;
use polymarket_client_sdk::clob::ws::{BookUpdate, Client};
use polymarket_client_sdk::types::U256;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

use pmbot_core::messages::MarketEvent;
use pmbot_core::types::{Level, TokenId};

use crate::actor::RawBookMessage;

/// Run a persistent Polymarket WebSocket connection.
///
/// Listens for `MarketRotation` events to update its subscription.
/// Forwards parsed orderbook snapshots to the `book_tx` channel.
pub async fn run_polymarket_ws(
    mut event_rx: broadcast::Receiver<MarketEvent>,
    book_tx: mpsc::Sender<RawBookMessage>,
    mut shutdown: broadcast::Receiver<()>,
) {
    info!("Polymarket WebSocket task started");

    let client = Client::default();
    let mut current_token: Option<TokenId> = None;
    
    // Type-erased stream for orderbook updates
    let mut stream: Option<Pin<Box<dyn Stream<Item = anyhow::Result<BookUpdate>> + Send>>> = None;

    loop {
        tokio::select! {
            _ = shutdown.recv() => {
                info!("Polymarket WebSocket task shutting down");
                break;
            }
            
            event = event_rx.recv() => {
                match event {
                    Ok(MarketEvent::MarketRotation { new, .. }) => {
                        if let Some(token_id) = new.token_ids.first() {
                            if Some(token_id) != current_token.as_ref() {
                                info!(%token_id, "subscribing to new market orderbook");
                                current_token = Some(token_id.clone());
                                
                                match U256::from_str(&token_id.0) {
                                    Ok(asset_id) => {
                                        match client.subscribe_orderbook(vec![asset_id]) {
                                            Ok(s) => {
                                                // Map SDK results to anyhow::Result for easier boxing
                                                let s = s.map(|res| res.map_err(|e| anyhow::anyhow!(e)));
                                                stream = Some(Box::pin(s));
                                            }
                                            Err(e) => {
                                                error!(error = %e, "failed to subscribe to orderbook");
                                            }
                                        }
                                    }
                                    Err(e) => {
                                        error!(error = %e, %token_id, "invalid token ID format");
                                    }
                                }
                            }
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                    _ => {}
                }
            }

            msg_opt = async {
                if let Some(ref mut s) = stream {
                    s.next().await
                } else {
                    futures::future::pending().await
                }
            } => {
                if let Some(result) = msg_opt {
                    match result {
                        Ok(update) => {
                            let msg = RawBookMessage::Snapshot {
                                bids: update.bids.into_iter().map(|l| Level {
                                    price: l.price,
                                    size: l.size,
                                }).collect(),
                                asks: update.asks.into_iter().map(|l| Level {
                                    price: l.price,
                                    size: l.size,
                                }).collect(),
                            };
                            if book_tx.send(msg).await.is_err() {
                                break;
                            }
                        }
                        Err(e) => {
                            warn!(error = %e, "Polymarket WS stream error");
                            stream = None; 
                        }
                    }
                } else {
                    warn!("Polymarket WS stream ended unexpectedly");
                    stream = None;
                }
            }
        }
    }
}
