//! App state store for key-value state persistence.
//!
//! Stores arbitrary key-value pairs for app state like last shutdown time.

use sqlx::{SqlitePool, Row};
use chrono::{DateTime, Utc};
use anyhow::Result;
use tracing::instrument;

/// Store for app state.
pub struct AppStateStore {
    pool: SqlitePool,
}

impl AppStateStore {
    /// Create a new app state store.
    pub fn new(pool: SqlitePool) -> Self {
        Self { pool }
    }
    
    /// Set a key-value pair.
    #[instrument(skip(self))]
    pub async fn set(&self, key: &str, value: &str) -> Result<()> {
        let now = Utc::now().to_rfc3339();
        sqlx::query(
            r#"INSERT OR REPLACE INTO app_state (key, value, updated_at)
               VALUES (?, ?, ?)"#
        )
        .bind(key)
        .bind(value)
        .bind(&now)
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    /// Get a value by key.
    #[instrument(skip(self))]
    pub async fn get(&self, key: &str) -> Result<Option<String>> {
        let result = sqlx::query(
            "SELECT value FROM app_state WHERE key = ?"
        )
        .bind(key)
        .fetch_optional(&self.pool)
        .await?;
        
        Ok(result.map(|r| r.get("value")))
    }
    
    /// Delete a key.
    #[instrument(skip(self))]
    pub async fn delete(&self, key: &str) -> Result<()> {
        sqlx::query(
            "DELETE FROM app_state WHERE key = ?"
        )
        .bind(key)
        .execute(&self.pool)
        .await?;
        
        Ok(())
    }
    
    /// Record shutdown time.
    pub async fn record_shutdown(&self) -> Result<()> {
        self.set("last_shutdown", &Utc::now().to_rfc3339()).await
    }
    
    /// Get last shutdown time.
    pub async fn get_last_shutdown(&self) -> Result<Option<DateTime<Utc>>> {
        let value = self.get("last_shutdown").await?;
        match value {
            Some(s) => Ok(Some(s.parse()?)),
            None => Ok(None),
        }
    }
    
    /// Record startup time.
    pub async fn record_startup(&self) -> Result<()> {
        self.set("last_startup", &Utc::now().to_rfc3339()).await
    }
    
    /// Get last startup time.
    pub async fn get_last_startup(&self) -> Result<Option<DateTime<Utc>>> {
        let value = self.get("last_startup").await?;
        match value {
            Some(s) => Ok(Some(s.parse()?)),
            None => Ok(None),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Database;

    #[tokio::test]
    async fn test_set_and_get() {
        let db = Database::in_memory().await.unwrap();
        let store = db.state();
        
        store.set("test_key", "test_value").await.unwrap();
        
        let value = store.get("test_key").await.unwrap();
        assert_eq!(value, Some("test_value".to_string()));
        
        store.delete("test_key").await.unwrap();
        
        let value = store.get("test_key").await.unwrap();
        assert_eq!(value, None);
    }

    #[tokio::test]
    async fn test_shutdown_time() {
        let db = Database::in_memory().await.unwrap();
        let store = db.state();
        
        let before = Utc::now();
        store.record_shutdown().await.unwrap();
        
        let last_shutdown = store.get_last_shutdown().await.unwrap();
        assert!(last_shutdown.is_some());
        assert!(last_shutdown.unwrap() >= before);
    }
}