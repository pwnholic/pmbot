use std::collections::HashMap;
use std::fmt;

use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use rust_decimal_macros::dec;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Newtypes — strong typing for IDs
// ---------------------------------------------------------------------------

/// Polymarket condition ID (B256 hex string).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct MarketId(pub String);

impl fmt::Display for MarketId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// ERC-1155 token ID (uint256 decimal string).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TokenId(pub String);

impl fmt::Display for TokenId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Internal position identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PositionId(pub Uuid);

impl PositionId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for PositionId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for PositionId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// CLOB order ID returned by the API.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct OrderId(pub String);

impl fmt::Display for OrderId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

/// Unique signal identifier for tracking signal → order linkage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SignalId(pub Uuid);

impl SignalId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for SignalId {
    fn default() -> Self {
        Self::new()
    }
}

/// Trading symbol (e.g., "BTCUSDT").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Symbol(pub String);

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

// ---------------------------------------------------------------------------
// Domain enums
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Buy,
    Sell,
}

impl fmt::Display for Side {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Side::Buy => write!(f, "BUY"),
            Side::Sell => write!(f, "SELL"),
        }
    }
}

impl std::str::FromStr for Side {
    type Err = anyhow::Error;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_uppercase().as_str() {
            "BUY" => Ok(Side::Buy),
            "SELL" => Ok(Side::Sell),
            _ => Err(anyhow::anyhow!("invalid side: {}", s)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OrderType {
    /// Good 'til cancelled
    Gtc,
    /// Fill or kill
    Fok,
    /// Good 'til date
    Gtd,
    /// Fill and kill
    Fak,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SignatureType {
    Eoa,
    Proxy,
    GnosisSafe,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ExitReason {
    TakeProfit,
    StopLoss,
    MarketExpiry,
    ManualCancel,
    StrategyExit,
    KillSwitch,
}

impl fmt::Display for ExitReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ExitReason::TakeProfit => write!(f, "TP"),
            ExitReason::StopLoss => write!(f, "SL"),
            ExitReason::MarketExpiry => write!(f, "EXPIRY"),
            ExitReason::ManualCancel => write!(f, "MANUAL"),
            ExitReason::StrategyExit => write!(f, "STRATEGY"),
            ExitReason::KillSwitch => write!(f, "KILL"),
        }
    }
}

// ---------------------------------------------------------------------------
// Domain structs
// ---------------------------------------------------------------------------

/// Market metadata from Gamma API.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketInfo {
    pub id: MarketId,
    pub question: String,
    pub slug: String,
    pub outcomes: Vec<String>,
    pub token_ids: Vec<TokenId>,
    pub outcome_prices: HashMap<String, Decimal>,
    pub condition_id: String,
    pub neg_risk: bool,
    pub active: bool,
    pub end_date: Option<DateTime<Utc>>,
    pub liquidity: Decimal,
    pub volume: Decimal,
    pub category: String,
    pub tags: Vec<String>,
}

impl MarketInfo {
    pub fn is_binary(&self) -> bool {
        self.outcomes.len() == 2
    }

    pub fn is_multi_option(&self) -> bool {
        self.outcomes.len() > 2
    }

    pub fn token_for_outcome(&self, outcome: &str) -> Option<&TokenId> {
        self.outcomes
            .iter()
            .position(|o| o == outcome)
            .map(|i| &self.token_ids[i])
    }

    pub fn price_for_outcome(&self, outcome: &str) -> Option<Decimal> {
        self.outcome_prices.get(outcome).copied()
    }

    pub fn validate_probabilities(&self) -> bool {
        let sum: Decimal = self.outcome_prices.values().sum();
        (sum - Decimal::ONE).abs() < dec!(0.01)
    }

    pub fn market_type(&self) -> MarketType {
        if self.outcomes.len() == 2 {
            if self.outcomes.contains(&"Yes".to_string()) {
                let yes_idx = self.outcomes.iter().position(|o| o == "Yes").unwrap();
                let no_idx = 1 - yes_idx;
                MarketType::Binary {
                    yes_token: self.token_ids[yes_idx].clone(),
                    no_token: self.token_ids[no_idx].clone(),
                }
            } else {
                MarketType::MultiOption {
                    outcomes: self
                        .outcomes
                        .iter()
                        .zip(self.token_ids.iter())
                        .map(|(o, t)| (o.clone(), t.clone()))
                        .collect(),
                }
            }
        } else {
            MarketType::MultiOption {
                outcomes: self
                    .outcomes
                    .iter()
                    .zip(self.token_ids.iter())
                    .map(|(o, t)| (o.clone(), t.clone()))
                    .collect(),
            }
        }
    }

    #[cfg(test)]
    pub fn binary_test(id: &str) -> Self {
        let mut outcome_prices = std::collections::HashMap::new();
        outcome_prices.insert("Yes".to_string(), dec!(0.5));
        outcome_prices.insert("No".to_string(), dec!(0.5));

        Self {
            id: MarketId(id.into()),
            question: format!("Market {}?", id),
            slug: id.into(),
            outcomes: vec!["Yes".into(), "No".into()],
            token_ids: vec![
                TokenId(format!("{}-yes", id)),
                TokenId(format!("{}-no", id)),
            ],
            outcome_prices,
            condition_id: format!("cond-{}", id),
            neg_risk: false,
            active: true,
            end_date: None,
            liquidity: dec!(10000),
            volume: dec!(50000),
            category: "Test".into(),
            tags: vec!["test".into()],
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MarketType {
    Binary {
        yes_token: TokenId,
        no_token: TokenId,
    },
    MultiOption {
        outcomes: Vec<(String, TokenId)>,
    },
}

/// Single orderbook price level.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Level {
    pub price: Decimal,
    pub size: Decimal,
}

/// Full orderbook snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderbookSnapshot {
    pub market_id: MarketId,
    pub token_id: TokenId,
    pub bids: Vec<Level>,
    pub asks: Vec<Level>,
    pub timestamp: DateTime<Utc>,
}

impl OrderbookSnapshot {
    pub fn best_bid(&self) -> Option<Level> {
        self.bids.first().copied()
    }

    pub fn best_ask(&self) -> Option<Level> {
        self.asks.first().copied()
    }

    pub fn mid_price(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some((bid.price + ask.price) / Decimal::TWO),
            _ => None,
        }
    }

    pub fn spread(&self) -> Option<Decimal> {
        match (self.best_bid(), self.best_ask()) {
            (Some(bid), Some(ask)) => Some(ask.price - bid.price),
            _ => None,
        }
    }
}

/// A recorded price at a point in time.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PricePoint {
    pub price: Decimal,
    pub timestamp: DateTime<Utc>,
}

/// External spot price from a feed.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SpotPrice {
    pub price: Decimal,
    pub timestamp: DateTime<Utc>,
}

/// Open order as tracked locally.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenOrder {
    pub order_id: OrderId,
    pub market_id: MarketId,
    pub token_id: TokenId,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    pub filled: Decimal,
    pub order_type: OrderType,
}

/// Execution fill event.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FillEvent {
    pub order_id: OrderId,
    pub signal_id: SignalId,
    pub market_id: MarketId,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    pub timestamp: DateTime<Utc>,
}
