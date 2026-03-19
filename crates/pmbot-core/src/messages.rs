use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;

use crate::types::{
    ExitReason, MarketId, MarketInfo, OpenOrder, OrderId, OrderType, OrderbookSnapshot, PositionId,
    PricePoint, Side, SignalId, SpotPrice, Symbol, TokenId,
};

// ---------------------------------------------------------------------------
// Market Actor → broadcast
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum MarketEvent {
    BookUpdate {
        market_id: MarketId,
        book: Arc<OrderbookSnapshot>,
    },
    PriceChange {
        market_id: MarketId,
        token_id: TokenId,
        price: Decimal,
    },
    MarketRotation {
        old: MarketId,
        new: MarketInfo,
    },
    /// All discovered markets available for trading.
    MarketsDiscovered {
        markets: Vec<MarketInfo>,
    },
    LatencyUpdate {
        latency: Duration,
    },
    Connected,
    Disconnected,
}

// ---------------------------------------------------------------------------
// Feed Actor → broadcast
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum FeedEvent {
    SpotPrice {
        symbol: Symbol,
        price: Decimal,
        timestamp: DateTime<Utc>,
    },
    VolUpdate {
        symbol: Symbol,
        realized_vol: Decimal,
        window: Duration,
    },
    LatencyUpdate {
        latency: Duration,
    },
}

// ---------------------------------------------------------------------------
// Strategy Actor → Risk Actor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum Signal {
    Enter {
        id: SignalId,
        strategy: &'static str,
        market_id: MarketId,
        token_id: TokenId,
        outcome: String,
        side: Side,
        size: Decimal,
        price: Option<Decimal>,
        edge: Decimal,
        confidence: Decimal,
    },
    Exit {
        id: SignalId,
        strategy: &'static str,
        signal_id: SignalId,
        reason: ExitReason,
    },
    Amend {
        strategy: &'static str,
        order_id: OrderId,
        new_price: Decimal,
        new_size: Decimal,
    },
    CancelAll {
        market_id: MarketId,
    },
}

impl Signal {
    pub fn binary_enter(
        strategy: &'static str,
        market_id: MarketId,
        yes_token: TokenId,
        size: Decimal,
        price: Option<Decimal>,
        edge: Decimal,
    ) -> Self {
        Signal::Enter {
            id: SignalId::new(),
            strategy,
            market_id,
            token_id: yes_token,
            outcome: "Yes".to_string(),
            side: Side::Buy,
            size,
            price,
            edge,
            confidence: Decimal::ONE,
        }
    }

    pub fn strategy_name(&self) -> &'static str {
        match self {
            Signal::Enter { strategy, .. } => strategy,
            Signal::Exit { strategy, .. } => strategy,
            Signal::Amend { strategy, .. } => strategy,
            Signal::CancelAll { .. } => "system",
        }
    }
}

// ---------------------------------------------------------------------------
// Risk Actor → Executor Actor
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ExecutableOrder {
    Limit {
        signal_id: SignalId,
        market_id: MarketId,
        token_id: TokenId,
        side: Side,
        price: Decimal,
        size: Decimal,
        order_type: OrderType,
        post_only: bool,
    },
    Market {
        signal_id: SignalId,
        market_id: MarketId,
        token_id: TokenId,
        side: Side,
        size: Decimal,
    },
    Cancel {
        order_id: OrderId,
    },
    CancelAll,
}

// ---------------------------------------------------------------------------
// Executor Actor → broadcast (all actors listen)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub enum ExecutionEvent {
    OrderPlaced {
        order_id: OrderId,
        signal_id: SignalId,
    },
    OrderFilled {
        order_id: OrderId,
        signal_id: SignalId,
        market_id: MarketId,
        side: Side,
        price: Decimal,
        size: Decimal,
    },
    OrderPartialFill {
        order_id: OrderId,
        signal_id: SignalId,
        market_id: MarketId,
        side: Side,
        price: Decimal,
        filled: Decimal,
        remaining: Decimal,
    },
    OrderCancelled {
        order_id: OrderId,
    },
    OrderRejected {
        order_id: Option<OrderId>,
        signal_id: SignalId,
        reason: String,
    },
}

// ---------------------------------------------------------------------------
// World State — immutable snapshot shared across actors
// ---------------------------------------------------------------------------

/// Immutable snapshot of the entire system state at a point in time.
/// Built every tick by the strategy actor, shared via `Arc`.
#[derive(Debug, Clone, Default)]
pub struct WorldState {
    pub active_market_id: Option<MarketId>,
    pub markets: std::collections::HashMap<MarketId, MarketSnapshot>,
    /// All discovered markets available for trading (for TUI search).
    pub discovered_markets: Vec<MarketInfo>,
    pub positions: Vec<Position>,
    pub open_orders: Vec<OpenOrder>,
    pub balance: Decimal,
    pub daily_pnl: Decimal,
    pub external_prices: std::collections::HashMap<Symbol, SpotPrice>,
    pub network_latency: std::collections::HashMap<&'static str, Duration>,
    pub timestamp: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct MarketSnapshot {
    pub info: MarketInfo,
    pub book: Arc<OrderbookSnapshot>,
    pub mid_price: Option<Decimal>,
    pub spread: Option<Decimal>,
    pub imbalance: Decimal,
    pub price_history: Vec<PricePoint>,
}

/// Live position tracked by the risk actor.
#[derive(Debug, Clone)]
pub struct Position {
    pub id: PositionId,
    pub market_id: MarketId,
    pub token_id: TokenId,
    pub outcome: String,
    pub strategy: &'static str,
    pub side: Side,
    pub entry_price: Decimal,
    pub size: Decimal,
    pub tp_price: Decimal,
    pub sl_price: Decimal,
    pub opened_at: DateTime<Utc>,
    pub unrealized_pnl: Decimal,
}

// ---------------------------------------------------------------------------
// Risk Actor → Strategy Actor (position updates)
// ---------------------------------------------------------------------------

/// Sent by the risk actor whenever position state changes, so the strategy
/// actor can keep its `WorldStateBuilder` in sync.
#[derive(Debug, Clone)]
pub struct PositionSnapshot {
    pub positions: Vec<Position>,
    /// Realized daily PnL from the circuit breaker + sum of unrealized PnL from open positions.
    pub daily_pnl: Decimal,
}

// ---------------------------------------------------------------------------
// Strategy metrics for TUI
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct StrategyMetrics {
    pub name: &'static str,
    pub state: &'static str,
    pub edge: Option<Decimal>,
    pub signals_generated: u64,
    pub trades: u64,
    pub wins: u64,
    pub losses: u64,
    pub total_pnl: Decimal,
    /// Cumulative PnL history for sparkline (most recent first).
    pub pnl_history: Vec<Decimal>,
    pub custom: Vec<(&'static str, String)>,
}

impl Default for StrategyMetrics {
    fn default() -> Self {
        Self {
            name: "unknown",
            state: "idle",
            edge: None,
            signals_generated: 0,
            trades: 0,
            wins: 0,
            losses: 0,
            total_pnl: Decimal::ZERO,
            pnl_history: Vec::new(),
            custom: Vec::new(),
        }
    }
}
