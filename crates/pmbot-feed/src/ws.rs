//! Real Binance WebSocket feed — connects to Binance trade streams and
//! forwards `RawTradeMessage`s to the feed actor.

use std::time::Duration;

use anyhow::{Context, Result};
use chrono::{TimeZone, Utc};
use futures::StreamExt;
use rust_decimal::Decimal;
use serde::Deserialize;
use tokio::sync::{broadcast, mpsc};
use tracing::{error, info, warn};

use pmbot_core::types::Symbol;

use crate::actor::{RawFeedMessage, RawFeedMessageTrade};

/// Binance trade event from the WebSocket stream.
#[derive(Debug, Deserialize)]
struct BinanceTrade {
    /// Symbol, e.g. "BTCUSDT".
    s: String,
    /// Price as a decimal string.
    p: String,
    /// Trade time in epoch milliseconds.
    #[serde(rename = "T")]
    trade_time: i64,
}

/// Wrapper for the combined-stream format.
#[derive(Debug, Deserialize)]
struct CombinedStream {
    data: BinanceTrade,
}

/// Run a persistent Binance WebSocket connection.
///
/// Reconnects automatically on disconnect with exponential backoff.
/// Sends parsed trades into `trade_tx`; exits cleanly on `shutdown`.
pub async fn run_binance_ws(
    symbols: &[Symbol],
    msg_tx: mpsc::Sender<RawFeedMessage>,
    mut shutdown: broadcast::Receiver<()>,
) {
    if symbols.is_empty() {
        warn!("no symbols configured for Binance feed");
        return;
    }

    let streams: Vec<String> = symbols
        .iter()
        .map(|s| format!("{}@trade", s.0.to_lowercase()))
        .collect();
    let url = format!(
        "wss://stream.binance.com:9443/stream?streams={}",
        streams.join("/")
    );

    let mut backoff = Duration::from_secs(1);
    let max_backoff = Duration::from_secs(60);

    loop {
        info!(%url, "connecting to Binance WebSocket");

        match connect_and_stream(&url, &msg_tx, &mut shutdown).await {
            Ok(()) => {
                info!("Binance WebSocket shutting down");
                return;
            }
            Err(e) => {
                error!(error = %e, backoff_secs = backoff.as_secs(), "Binance WS error, reconnecting");

                tokio::select! {
                    _ = shutdown.recv() => {
                        info!("Binance WebSocket shutdown during backoff");
                        return;
                    }
                    _ = tokio::time::sleep(backoff) => {}
                }

                backoff = (backoff * 2).min(max_backoff);
            }
        }
    }
}

async fn connect_and_stream(
    url: &str,
    msg_tx: &mpsc::Sender<RawFeedMessage>,
    shutdown: &mut broadcast::Receiver<()>,
) -> Result<()> {
    let (ws_stream, _response) = tokio_tungstenite::connect_async(url)
        .await
        .context("WebSocket connect failed")?;

    info!("connected to Binance WebSocket");

    let (mut write, mut read) = ws_stream.split();
    let mut ping_interval = tokio::time::interval(Duration::from_secs(5));
    // When we sent the last ping
    let mut last_ping_time: Option<std::time::Instant> = None;

    loop {
        tokio::select! {
            _ = shutdown.recv() => {
                return Ok(());
            }
            msg = read.next() => {
                match msg {
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Text(text))) => {
                        // Try combined-stream format first, then single-stream.
                        let trade = serde_json::from_str::<CombinedStream>(&text)
                            .map(|c| c.data)
                            .or_else(|_| serde_json::from_str::<BinanceTrade>(&text));

                        match trade {
                            Ok(t) => {
                                if let Some(msg) = parse_trade(&t) {
                                    if msg_tx.send(RawFeedMessage::Trade(msg)).await.is_err() {
                                        return Ok(()); // channel closed
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(error = %e, "failed to parse Binance message");
                            }
                        }
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Close(_))) => {
                        warn!("Binance WebSocket closed by server");
                        anyhow::bail!("WebSocket closed by server");
                    }
                    Some(Ok(tokio_tungstenite::tungstenite::Message::Pong(_))) => {
                        if let Some(sent) = last_ping_time.take() {
                            let latency = sent.elapsed();
                            // Send latency update to actor
                            if msg_tx.send(RawFeedMessage::Latency(latency)).await.is_err() {
                                return Ok(());
                            }
                        }
                    }
                    Some(Ok(_)) => {} // Other Ping, Binary — ignore
                    Some(Err(e)) => {
                        anyhow::bail!("WebSocket read error: {e}");
                    }
                    None => {
                        anyhow::bail!("WebSocket stream ended");
                    }
                }
            }
            _ = ping_interval.tick() => {
                last_ping_time = Some(std::time::Instant::now());
                if futures::SinkExt::send(&mut write, tokio_tungstenite::tungstenite::Message::Ping(vec![].into())).await.is_err() {
                    return Ok(());
                }
            }
        }
    }
}

fn parse_trade(trade: &BinanceTrade) -> Option<RawFeedMessageTrade> {
    let price = trade.p.parse::<Decimal>().ok()?;
    let timestamp = Utc.timestamp_millis_opt(trade.trade_time).single()?;

    Some(RawFeedMessageTrade {
        symbol: Symbol(trade.s.clone()),
        price,
        timestamp,
    })
}
