//! Order state store for reconciliation.
//!
//! Tracks open orders across restarts for state recovery.

use sqlx::{SqlitePool, Row};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use anyhow::Result;
use tracing::instrument;
use uuid::Uuid;

use pmbot_core::types::{OrderId, SignalId, MarketId, TokenId, Side};

/// Order status.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OrderStatus {
    Open,
    PartiallyFilled,
    Filled,
    Cancelled,
}

impl std::fmt::Display for OrderStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OrderStatus::Open => write!(f, "open"),
            OrderStatus::PartiallyFilled => write!(f, "partially_filled"),
            OrderStatus::Filled => write!(f, "filled"),
            OrderStatus::Cancelled => write!(f, "cancelled"),
        }
    }
}

impl std::str::FromStr for OrderStatus {
    type Err = anyhow::Error;
    
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "open" => Ok(OrderStatus::Open),
            "partially_filled" => Ok(OrderStatus::PartiallyFilled),
            "filled" => Ok(OrderStatus::Filled),
            "cancelled" => Ok(OrderStatus::Cancelled),
            _ => Err(anyhow::anyhow!("invalid order status: {}", s)),
        }
    }
}

/// A tracked order for reconciliation.
#[derive(Debug, Clone)]
pub struct TrackedOrder {
    pub order_id: OrderId,
    pub signal_id: SignalId,
    pub market_id: MarketId,
    pub token_id: TokenId,
    pub outcome: String,
    pub side: Side,
    pub price: Decimal,
    pub size: Decimal,
    pub filled: Decimal,
    pub status: OrderStatus,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Store for open orders.
pub struct OrderStore {
    pool: SqlitePool,
}

impl OrderStore {
    /// Create a new order store.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
    
    /// Insert a new open order.
    #[instrument(skip(self, order))]
    pub async fn insert(&self, order: &TrackedOrder) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO open_orders (
                order_id, signal_id, market_id, token_id, outcome, side,
                price, size, filled, status, created_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#
        )
        .bind(&order.order_id.0)
        .bind(order.signal_id.0.to_string())
        .bind(&order.market_id.0)
        .bind(&order.token_id.0)
        .bind(&order.outcome)
        .bind(order.side.to_string())
        .bind(order.price.to_string())
        .bind(order.size.to_string())
        .bind(order.filled.to_string())
        .bind(order.status.to_string())
        .bind(order.created_at.to_rfc3339())
        .bind(order.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    /// Get all open orders.
    #[instrument(skip(self))]
    pub async fn get_open(&self) -> Result<Vec<TrackedOrder>> {
        let rows = sqlx::query(
            r#"SELECT order_id, signal_id, market_id, token_id, outcome,
                      side, price, size, filled, status, created_at, updated_at
               FROM open_orders
               WHERE status IN ('open', 'partially_filled')"#
        )
        .fetch_all(&self.pool)
        .await?;
        
        rows.into_iter().map(|r| {
            Ok(TrackedOrder {
                order_id: OrderId(r.get::<&str, _>("order_id").to_string()),
                signal_id: SignalId(Uuid::parse_str(r.get::<&str, _>("signal_id")).unwrap_or_else(|_| Uuid::nil())),
                market_id: MarketId(r.get::<&str, _>("market_id").to_string()),
                token_id: TokenId(r.get::<&str, _>("token_id").to_string()),
                outcome: r.get::<&str, _>("outcome").to_string(),
                side: r.get::<&str, _>("side").parse()?,
                price: r.get::<&str, _>("price").parse()?,
                size: r.get::<&str, _>("size").parse()?,
                filled: r.get::<&str, _>("filled").parse()?,
                status: r.get::<&str, _>("status").parse()?,
                created_at: r.get::<&str, _>("created_at").parse()?,
                updated_at: r.get::<&str, _>("updated_at").parse()?,
            })
        }).collect()
    }
    
    /// Update filled amount.
    #[instrument(skip(self))]
    pub async fn update_filled(
        &self,
        order_id: &OrderId,
        filled: Decimal,
        status: OrderStatus,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"UPDATE open_orders 
               SET filled = ?, status = ?, updated_at = ?
               WHERE order_id = ?"#
        )
        .bind(filled.to_string())
        .bind(status.to_string())
        .bind(&now)
        .bind(&order_id.0)
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    /// Remove a filled or cancelled order.
    #[instrument(skip(self))]
    pub async fn remove(&self, order_id: &OrderId) -> Result<()> {
        sqlx::query(
            "DELETE FROM open_orders WHERE order_id = ?"
        )
        .bind(&order_id.0)
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    /// Clear all orders (for clean start).
    #[instrument(skip(self))]
    pub async fn clear(&self) -> Result<()> {
        sqlx::query("DELETE FROM open_orders")
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
    async fn test_insert_and_get_orders() {
        let db = Database::in_memory().await.unwrap();
        let store = db.orders();
        
        let order = TrackedOrder {
            order_id: OrderId("ord-123".into()),
            signal_id: SignalId::new(),
            market_id: MarketId("mkt-789".into()),
            token_id: TokenId("tok-012".into()),
            outcome: "Yes".into(),
            side: Side::Buy,
            price: Decimal::from(50),
            size: Decimal::from(100),
            filled: Decimal::from(0),
            status: OrderStatus::Open,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        };
        
        store.insert(&order).await.unwrap();
        
        let open = store.get_open().await.unwrap();
        assert_eq!(open.len(), 1);
        assert_eq!(open[0].order_id.0, "ord-123");
        
        store.update_filled(&OrderId("ord-123".into()), Decimal::from(50), OrderStatus::PartiallyFilled).await.unwrap();
        
        let open = store.get_open().await.unwrap();
        assert_eq!(open[0].filled, Decimal::from(50));
    }
}