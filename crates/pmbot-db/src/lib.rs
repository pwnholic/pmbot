//! Database persistence layer for pmbot.
//!
//! Provides SQLite-based storage for trade history, order state,
//! positions, and app state for crash recovery.

pub mod trades;
pub mod orders;
pub mod positions;
pub mod state;

pub use trades::TradeStore;
pub use orders::OrderStore;
pub use positions::PositionStore;
pub use state::AppStateStore;

use sqlx::SqlitePool;
use anyhow::Result;

/// Main database handle.
///
/// Provides access to all stores through dedicated methods.
pub struct Database {
    pool: SqlitePool,
}

impl Database {
    /// Connect to the database at the given path.
    ///
    /// Creates the database if it doesn't exist.
    /// Enables WAL mode for better concurrency.
    /// Runs schema migrations on startup.
    pub async fn connect(path: &str) -> Result<Self> {
        let pool = SqlitePool::connect(&format!("sqlite:{}?mode=rwc", path)).await?;
        
        // Enable WAL mode for better concurrency
        sqlx::query("PRAGMA journal_mode=WAL;").execute(&pool).await?;
        sqlx::query("PRAGMA foreign_keys=ON;").execute(&pool).await?;
        
        // Run initial schema
        sqlx::query(include_str!("schema.sql")).execute(&pool).await?;
        
        // Run migrations
        Self::run_migrations(&pool).await?;
        
        Ok(Self { pool })
    }
    
    /// Create in-memory database (for testing).
    #[cfg(test)]
    pub async fn in_memory() -> Result<Self> {
        let pool = SqlitePool::connect("sqlite::memory:").await?;
        sqlx::query("PRAGMA journal_mode=WAL;").execute(&pool).await?;
        sqlx::query("PRAGMA foreign_keys=ON;").execute(&pool).await?;
        sqlx::query(include_str!("schema.sql")).execute(&pool).await?;
        Self::run_migrations(&pool).await?;
        Ok(Self { pool })
    }
    
    /// Run database migrations.
    async fn run_migrations(pool: &SqlitePool) -> Result<()> {
        let current_version: i64 = sqlx::query_scalar("SELECT COALESCE(MAX(version), 0) FROM schema_version")
            .fetch_one(pool)
            .await?;
        
        // Future migrations go here:
        // if current_version < 2 {
        //     sqlx::query(include_str!("../migrations/002_add_column.sql"))
        //         .execute(pool).await?;
        //     sqlx::query("INSERT INTO schema_version (version) VALUES (2)")
        //         .execute(pool).await?;
        // }
        
        tracing::info!(version = current_version, "database schema version");
        Ok(())
    }
    
    /// Trade history store.
    pub fn trades(&self) -> TradeStore {
        TradeStore::new(self.pool.clone())
    }
    
    /// Open orders store.
    pub fn orders(&self) -> OrderStore {
        OrderStore::new(self.pool.clone())
    }
    
    /// Positions store.
    pub fn positions(&self) -> PositionStore {
        PositionStore::new(self.pool.clone())
    }
    
    /// App state store.
    pub fn state(&self) -> AppStateStore {
        AppStateStore::new(self.pool.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_connect_creates_schema() {
        let db = Database::in_memory().await.unwrap();
        
        // Check tables exist
        let count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name='trades'"
        ).fetch_one(&db.pool).await.unwrap();
        assert_eq!(count, 1);
    }
}