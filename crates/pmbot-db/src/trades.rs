//! Trade history store.
//!
//! Persists completed trades for strategy performance analysis and history.

use sqlx::{SqlitePool, Row};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use anyhow::Result;
use tracing::instrument;
use uuid::Uuid;

use pmbot_core::types::{OrderId, SignalId, MarketId, TokenId, Side};

/// A completed or in-progress trade.
#[derive(Debug, Clone)]
pub struct Trade {
    pub id: i64,
    pub order_id: OrderId,
    pub signal_id: SignalId,
    pub market_id: MarketId,
    pub token_id: TokenId,
    pub outcome: String,
    pub strategy: String,
    pub side: Side,
    pub entry_price: Decimal,
    pub exit_price: Option<Decimal>,
    pub size: Decimal,
    pub filled_size: Decimal,
    pub pnl: Option<Decimal>,
    pub opened_at: DateTime<Utc>,
    pub closed_at: Option<DateTime<Utc>>,
}

/// Store for trade history.
pub struct TradeStore {
    pool: SqlitePool,
}

impl TradeStore {
    /// Create a new trade store.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
    
    /// Insert a new trade.
    #[instrument(skip(self, trade))]
    pub async fn insert(&self, trade: &Trade) -> Result<i64> {
        let result = sqlx::query(
            r#"INSERT INTO trades (
                order_id, signal_id, market_id, token_id, outcome, strategy,
                side, entry_price, size, filled_size, opened_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#
        )
        .bind(&trade.order_id.0)
        .bind(trade.signal_id.0.to_string())
        .bind(&trade.market_id.0)
        .bind(&trade.token_id.0)
        .bind(&trade.outcome)
        .bind(&trade.strategy)
        .bind(format!("{:?}", trade.side))
        .bind(trade.entry_price.to_string())
        .bind(trade.size.to_string())
        .bind(trade.filled_size.to_string())
        .bind(trade.opened_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        
        Ok(result.last_insert_rowid())
    }
    
    /// Get all trades for a strategy.
    #[instrument(skip(self))]
    pub async fn get_by_strategy(&self, strategy: &str) -> Result<Vec<Trade>> {
        let rows = sqlx::query(
            r#"SELECT id, order_id, signal_id, market_id, token_id, outcome,
                      strategy, side, entry_price, exit_price, size, filled_size,
                      pnl, opened_at, closed_at
               FROM trades WHERE strategy = ?"#
        )
        .bind(strategy)
        .fetch_all(&self.pool)
        .await?;
        
rows.into_iter().map(|r| {
            Ok(Trade {
                id: r.get("id"),
                order_id: OrderId(r.get::<&str, _>("order_id").to_string()),
                signal_id: SignalId(Uuid::parse_str(r.get::<&str, _>("signal_id")).unwrap_or_else(|_| Uuid::nil())),
                market_id: MarketId(r.get::<&str, _>("market_id").to_string()),
                token_id: TokenId(r.get::<&str, _>("token_id").to_string()),
                outcome: r.get::<&str, _>("outcome").to_string(),
                strategy: r.get::<&str, _>("strategy").to_string(),
                side: r.get::<&str, _>("side").parse()?,
                entry_price: r.get::<&str, _>("entry_price").parse()?,
                exit_price: r.get::<Option<&str>, _>("exit_price").map(|s| s.parse()).transpose()?,
                size: r.get::<&str, _>("size").parse()?,
                filled_size: r.get::<&str, _>("filled_size").parse()?,
                pnl: r.get::<Option<&str>, _>("pnl").map(|s| s.parse()).transpose()?,
                opened_at: r.get::<&str, _>("opened_at").parse()?,
                closed_at: r.get::<Option<&str>, _>("closed_at").map(|s| s.parse()).transpose()?,
            })
        }).collect()
    }
    
    /// Get total PnL for today (UTC).
    #[instrument(skip(self))]
    pub async fn get_daily_pnl(&self) -> Result<Decimal> {
        let today = Utc::now().format("%Y-%m-%d").to_string();
        let pnl: Option<String> = sqlx::query(
            r#"SELECT COALESCE(SUM(pnl), '0') FROM trades 
               WHERE closed_at IS NOT NULL 
               AND date(opened_at) = date(?)"#
        )
        .bind(&today)
        .fetch_one(&self.pool)
        .await?
        .get(0);
        
        Ok(pnl.unwrap_or_else(|| "0".to_string()).parse()?)
    }
    
    /// Get recent trades (last N closed trades).
    #[instrument(skip(self))]
    pub async fn get_recent(&self, limit: usize) -> Result<Vec<Trade>> {
        let rows = sqlx::query(
            r#"SELECT id, order_id, signal_id, market_id, token_id, outcome,
                      strategy, side, entry_price, exit_price, size, filled_size,
                      pnl, opened_at, closed_at
               FROM trades 
               WHERE closed_at IS NOT NULL
               ORDER BY closed_at DESC
               LIMIT ?"#
        )
        .bind(limit as i64)
        .fetch_all(&self.pool)
        .await?;
        
        rows.into_iter().map(|r| {
            Ok(Trade {
                id: r.get("id"),
                order_id: OrderId(r.get::<&str, _>("order_id").to_string()),
                signal_id: SignalId(Uuid::parse_str(r.get::<&str, _>("signal_id")).unwrap_or_else(|_| Uuid::nil())),
                market_id: MarketId(r.get::<&str, _>("market_id").to_string()),
                token_id: TokenId(r.get::<&str, _>("token_id").to_string()),
                outcome: r.get::<&str, _>("outcome").to_string(),
                strategy: r.get::<&str, _>("strategy").to_string(),
                side: r.get::<&str, _>("side").parse()?,
                entry_price: r.get::<&str, _>("entry_price").parse()?,
                exit_price: r.get::<Option<&str>, _>("exit_price").map(|s| s.parse()).transpose()?,
                size: r.get::<&str, _>("size").parse()?,
                filled_size: r.get::<&str, _>("filled_size").parse()?,
                pnl: r.get::<Option<&str>, _>("pnl").map(|s| s.parse()).transpose()?,
                opened_at: r.get::<&str, _>("opened_at").parse()?,
                closed_at: r.get::<Option<&str>, _>("closed_at").map(|s| s.parse()).transpose()?,
            })
        }).collect()
    }
    
    /// Get cumulative PnL history for a strategy (last N data points).
    #[instrument(skip(self))]
    pub async fn get_pnl_history(&self, strategy: &str, points: usize) -> Result<Vec<Decimal>> {
        let rows = sqlx::query(
            r#"SELECT pnl, opened_at FROM trades 
               WHERE strategy = ? AND closed_at IS NOT NULL AND pnl IS NOT NULL
               ORDER BY opened_at ASC"#
        )
        .bind(strategy)
        .fetch_all(&self.pool)
        .await?;
        
        let mut cumulative = Decimal::ZERO;
        let mut history: Vec<Decimal> = rows.into_iter()
            .filter_map(|r| {
                let pnl: Option<String> = r.get("pnl");
                pnl.and_then(|p| p.parse::<Decimal>().ok())
            })
            .map(|pnl: Decimal| {
                cumulative += pnl;
                cumulative
            })
            .collect();
        history.reverse();
        
        if history.len() > points {
            history.truncate(points);
        }
        
        Ok(history)
    }
    
    /// Close a trade with exit price and PnL.
    #[instrument(skip(self))]
    pub async fn close_trade(
        &self,
        order_id: &OrderId,
        exit_price: Decimal,
        pnl: Decimal,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"UPDATE trades 
               SET exit_price = ?, pnl = ?, closed_at = ?
               WHERE order_id = ?"#
        )
        .bind(exit_price.to_string())
        .bind(pnl.to_string())
        .bind(&now)
        .bind(&order_id.0)
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;

    #[tokio::test]
    async fn test_insert_and_get_trade() {
        let db = Database::in_memory().await.unwrap();
        let store = db.trades();
        
        let trade = Trade {
            id: 0,
            order_id: OrderId("ord-123".into()),
            signal_id: SignalId::new(),
            market_id: MarketId("mkt-789".into()),
            token_id: TokenId("tok-012".into()),
            outcome: "Yes".into(),
            strategy: "fair_value".into(),
            side: Side::Buy,
            entry_price: Decimal::from(50),
            exit_price: None,
            size: Decimal::from(100),
            filled_size: Decimal::from(0),
            pnl: None,
            opened_at: Utc::now(),
            closed_at: None,
        };
        
        let id = store.insert(&trade).await.unwrap();
        assert!(id > 0);
        
        let trades = store.get_by_strategy("fair_value").await.unwrap();
        assert_eq!(trades.len(), 1);
        assert_eq!(trades[0].order_id.0, "ord-123");
    }
}