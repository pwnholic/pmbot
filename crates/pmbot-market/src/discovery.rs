use std::collections::HashMap;
use anyhow::Result;
use async_trait::async_trait;
use rust_decimal::Decimal;

use pmbot_core::types::MarketInfo;

/// Filters for market discovery.
#[derive(Debug, Clone)]
pub struct DiscoveryFilters {
    pub min_liquidity: Decimal,
    pub min_volume: Decimal,
    /// Categories to search: ["politics", "sports", "crypto", "finance"]
    pub categories: Vec<String>,
    /// Tags per category: {"politics": ["geopolitics", "election"], "sports": ["NFL", "NBA"]}
    pub tags: HashMap<String, Vec<String>>,
    /// Free-text search queries
    pub search_queries: Vec<String>,
    /// Tags to exclude from results
    pub exclude_tags: Vec<String>,
    pub active_only: bool,
}

impl Default for DiscoveryFilters {
    fn default() -> Self {
        Self {
            min_liquidity: Decimal::ZERO,
            min_volume: Decimal::ZERO,
            categories: vec!["crypto".into()],
            tags: HashMap::new(),
            search_queries: Vec::new(),
            exclude_tags: Vec::new(),
            active_only: true,
        }
    }
}

/// Trait for discovering available markets.
#[async_trait]
pub trait MarketDiscovery: Send + Sync {
    /// Discover markets matching the given filters.
    async fn discover(&self, filters: &DiscoveryFilters) -> Result<Vec<MarketInfo>>;
}

/// Mock discovery implementation for testing.
#[derive(Debug, Clone)]
pub struct MockDiscovery {
    markets: Vec<MarketInfo>,
}

impl MockDiscovery {
    /// Create a mock discovery that returns the given canned markets.
    pub fn new(markets: Vec<MarketInfo>) -> Self {
        Self { markets }
    }
}

#[async_trait]
impl MarketDiscovery for MockDiscovery {
    async fn discover(&self, filters: &DiscoveryFilters) -> Result<Vec<MarketInfo>> {
        let results: Vec<MarketInfo> = self
            .markets
            .iter()
            .filter(|m| {
                if filters.active_only && !m.active {
                    return false;
                }
                if m.liquidity < filters.min_liquidity {
                    return false;
                }
                if m.volume < filters.min_volume {
                    return false;
                }
                true
            })
            .cloned()
            .collect();

        Ok(results)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use pmbot_core::types::{MarketId, TokenId};
    use rust_decimal_macros::dec;
    use std::collections::HashMap;

    fn make_market(id: &str, liquidity: Decimal, volume: Decimal, active: bool) -> MarketInfo {
        let mut outcome_prices = HashMap::new();
        outcome_prices.insert("Yes".to_string(), dec!(0.5));
        outcome_prices.insert("No".to_string(), dec!(0.5));
        
        MarketInfo {
            id: MarketId(id.into()),
            question: format!("Market {id}?"),
            slug: id.into(),
            outcomes: vec!["Yes".into(), "No".into()],
            token_ids: vec![TokenId(format!("{id}-yes")), TokenId(format!("{id}-no"))],
            outcome_prices,
            condition_id: format!("cond-{id}"),
            neg_risk: false,
            active,
            end_date: Some(Utc::now() + chrono::Duration::hours(1)),
            liquidity,
            volume,
            category: "Test".into(),
            tags: vec!["test".into()],
        }
    }

    #[tokio::test]
    async fn test_mock_discovery_returns_all() {
        let markets = vec![
            make_market("m1", dec!(10000), dec!(50000), true),
            make_market("m2", dec!(5000), dec!(20000), true),
        ];
        let disc = MockDiscovery::new(markets);

        let results = disc
            .discover(&DiscoveryFilters::default())
            .await
            .unwrap();
        assert_eq!(results.len(), 2);
    }

    #[tokio::test]
    async fn test_mock_discovery_filters_by_liquidity() {
        let markets = vec![
            make_market("m1", dec!(10000), dec!(50000), true),
            make_market("m2", dec!(1000), dec!(50000), true),
        ];
        let disc = MockDiscovery::new(markets);

        let filters = DiscoveryFilters {
            min_liquidity: dec!(5000),
            ..Default::default()
        };
        let results = disc.discover(&filters).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].id.0, "m1");
    }

    #[tokio::test]
    async fn test_mock_discovery_filters_inactive() {
        let markets = vec![
            make_market("m1", dec!(10000), dec!(50000), true),
            make_market("m2", dec!(10000), dec!(50000), false),
        ];
        let disc = MockDiscovery::new(markets);

        let filters = DiscoveryFilters {
            active_only: true,
            ..Default::default()
        };
        let results = disc.discover(&filters).await.unwrap();
        assert_eq!(results.len(), 1);
    }
}
