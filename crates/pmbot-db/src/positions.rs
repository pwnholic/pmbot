//! Position state store for tracking open positions.
//!
//! Persists position state across restarts for recovery.

use sqlx::{SqlitePool, Row};
use chrono::{DateTime, Utc};
use rust_decimal::Decimal;
use anyhow::Result;
use tracing::instrument;
use uuid::Uuid;

use pmbot_core::types::{PositionId, MarketId, TokenId, Side};

/// A tracked position.
#[derive(Debug, Clone)]
pub struct StoredPosition {
    pub id: PositionId,
    pub market_id: MarketId,
    pub token_id: TokenId,
    pub outcome: String,
    pub strategy: String,
    pub side: Side,
    pub entry_price: Decimal,
    pub size: Decimal,
    pub unrealized_pnl: Decimal,
    pub opened_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

/// Store for positions.
pub struct PositionStore {
    pool: SqlitePool,
}

impl PositionStore {
    /// Create a new position store.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
    
    /// Insert a new position.
    #[instrument(skip(self, position))]
    pub async fn insert(&self, position: &StoredPosition) -> Result<()> {
        sqlx::query(
            r#"INSERT INTO positions (
                id, market_id, token_id, outcome, strategy, side,
                entry_price, size, unrealized_pnl, opened_at, updated_at
            ) VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"#
        )
        .bind(position.id.0.to_string())
        .bind(&position.market_id.0)
        .bind(&position.token_id.0)
        .bind(&position.outcome)
        .bind(&position.strategy)
        .bind(position.side.to_string())
        .bind(position.entry_price.to_string())
        .bind(position.size.to_string())
        .bind(position.unrealized_pnl.to_string())
        .bind(position.opened_at.to_rfc3339())
        .bind(position.updated_at.to_rfc3339())
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    /// Parse a row into a StoredPosition.
    fn parse_position(row: &sqlx::sqlite::SqliteRow) -> Result<StoredPosition> {
        Ok(StoredPosition {
            id: PositionId(Uuid::parse_str(row.get::<&str, _>("id")).unwrap_or_else(|_| Uuid::nil())),
            market_id: MarketId(row.get("market_id")),
            token_id: TokenId(row.get("token_id")),
            outcome: row.get("outcome"),
            strategy: row.get("strategy"),
            side: row.get::<&str, _>("side").parse()?,
            entry_price: row.get::<&str, _>("entry_price").parse()?,
            size: row.get::<&str, _>("size").parse()?,
            unrealized_pnl: row.get::<&str, _>("unrealized_pnl").parse()?,
            opened_at: row.get::<&str, _>("opened_at").parse()?,
            updated_at: row.get::<&str, _>("updated_at").parse()?,
        })
    }
    
    /// Get all open positions.
    #[instrument(skip(self))]
    pub async fn get_all(&self) -> Result<Vec<StoredPosition>> {
        let rows = sqlx::query(
            r#"SELECT id, market_id, token_id, outcome, strategy, side,
                      entry_price, size, unrealized_pnl, opened_at, updated_at
               FROM positions"#
        )
        .fetch_all(&self.pool)
        .await?;
        
        rows.iter().map(|r| Self::parse_position(r)).collect()
    }
    
    /// Get positions by strategy.
    #[instrument(skip(self))]
    pub async fn get_by_strategy(&self, strategy: &str) -> Result<Vec<StoredPosition>> {
        let rows = sqlx::query(
            r#"SELECT id, market_id, token_id, outcome, strategy, side,
                      entry_price, size, unrealized_pnl, opened_at, updated_at
               FROM positions WHERE strategy = ?"#
        )
        .bind(strategy)
        .fetch_all(&self.pool)
        .await?;
        
        rows.iter().map(|r| Self::parse_position(r)).collect()
    }
    
    /// Update position PnL.
    #[instrument(skip(self))]
    pub async fn update_pnl(
        &self,
        id: &PositionId,
        size: Decimal,
        unrealized_pnl: Decimal,
    ) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"UPDATE positions 
               SET size = ?, unrealized_pnl = ?, updated_at = ?
               WHERE id = ?"#
        )
        .bind(size.to_string())
        .bind(unrealized_pnl.to_string())
        .bind(&now)
        .bind(id.0.to_string())
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    /// Remove a position (closed).
    #[instrument(skip(self))]
    pub async fn remove(&self, id: &PositionId) -> Result<()> {
        sqlx::query(
            "DELETE FROM positions WHERE id = ?"
        )
        .bind(id.0.to_string())
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    /// Clear all positions (for clean start).
    #[instrument(skip(self))]
    pub async fn clear(&self) -> Result<()> {
        sqlx::query("DELETE FROM positions")
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
    async fn test_insert_and_get_positions() {
        let db = Database::in_memory().await.unwrap();
        let store = db.positions();
        
        let position = StoredPosition {
            id: PositionId::new(),
            market_id: MarketId("mkt-789".into()),
            token_id: TokenId("tok-012".into()),
            outcome: "Yes".into(),
            strategy: "fair_value".into(),
            side: Side::Buy,
            entry_price: Decimal::from(50),
            size: Decimal::from(100),
            unrealized_pnl: Decimal::from(10),
            opened_at: Utc::now(),
            updated_at: Utc::now(),
        };
        
        store.insert(&position).await.unwrap();
        
        let all = store.get_all().await.unwrap();
        assert_eq!(all.len(), 1);
        
        let by_strategy = store.get_by_strategy("fair_value").await.unwrap();
        assert_eq!(by_strategy.len(), 1);
    }
}