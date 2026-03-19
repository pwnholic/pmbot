//! Gamma API discovery — fetches live markets from Polymarket's Gamma service.

use anyhow::{Context, Result};
use async_trait::async_trait;
use rust_decimal::Decimal;
use tracing::info;

use polymarket_client_sdk::gamma::{self, types::request::MarketsRequest};

use pmbot_core::types::{MarketId, MarketInfo, TokenId};

use crate::discovery::{DiscoveryFilters, MarketDiscovery};

/// Market discovery backed by the Polymarket Gamma API.
pub struct GammaDiscovery {
    client: gamma::Client,
}

impl GammaDiscovery {
    pub fn new() -> Self {
        Self {
            client: gamma::Client::default(),
        }
    }

    /// Generate candidate slugs for BTC up/down 5-minute markets.
    ///
    /// The slug pattern is: `btc-updown-5m-{unix_timestamp}`
    /// where the timestamp is a 5-minute boundary (divisible by 300).
    ///
    /// We generate slugs for the current window plus several upcoming windows
    /// to ensure we always have tradeable markets queued up.
    /// Does NOT include past boundary slugs to avoid re-selecting expired markets.
    fn generate_btc_5m_slugs(lookahead_windows: usize) -> Vec<String> {
        let now = chrono::Utc::now().timestamp() as u64;
        let boundary_secs = 300u64; // 5 minutes

        // Current 5-min boundary
        let current_boundary = (now / boundary_secs) * boundary_secs;

        let mut slugs = Vec::new();

        // Current window + lookahead windows (no past boundaries)
        for i in 0..=(lookahead_windows) {
            let ts = current_boundary + (i as u64 * boundary_secs);
            slugs.push(format!("btc-updown-5m-{ts}"));
        }

        slugs
    }
}

impl Default for GammaDiscovery {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MarketDiscovery for GammaDiscovery {
    async fn discover(&self, filters: &DiscoveryFilters) -> Result<Vec<MarketInfo>> {
        let mut all_markets = Vec::new();
        let mut seen_ids = std::collections::HashSet::new();
        
        // 1. Discover by categories
        for category in &filters.categories {
            let request = MarketsRequest::builder()
                .limit(100)
                .liquidity_num_min(filters.min_liquidity)
                .volume_num_min(filters.min_volume)
                .closed(!filters.active_only)
                .build();
            
            let markets = self.client
                .markets(&request)
                .await
                .context("failed to fetch markets from Gamma API")?;
            
            for market in markets {
                if seen_ids.insert(market.id.clone()) {
                    if let Ok(info) = map_gamma_market(&market) {
                        // Filter by category (match against market tags if available)
                        if let Some(ref tags) = market.tags {
                            if tags.iter().any(|t| {
                                t.id.to_lowercase() == category.to_lowercase()
                            }) {
                                all_markets.push(info);
                                continue;
                            }
                        }
                        // If no tags, include anyway (API already filters)
                        all_markets.push(info);
                    }
                }
            }
        }
        
        // 2. Discover by search queries
        for query in &filters.search_queries {
            let request = MarketsRequest::builder()
                .slug(vec![query.clone()])
                .limit(50)
                .closed(!filters.active_only)
                .build();
            
            let markets = self.client
                .markets(&request)
                .await
                .context("failed to fetch markets from Gamma API")?;
            
            for market in markets {
                if seen_ids.insert(market.id.clone()) {
                    if let Ok(info) = map_gamma_market(&market) {
                        all_markets.push(info);
                    }
                }
            }
        }
        
        // 3. Apply exclude_tags filter
        if !filters.exclude_tags.is_empty() {
            all_markets.retain(|m| {
                // Check if market question/slug contains any excluded tags
                let text = format!("{} {}", m.question.to_lowercase(), m.slug.to_lowercase());
                !filters.exclude_tags.iter().any(|tag| text.contains(&tag.to_lowercase()))
            });
        }
        
        // 4. Filter by liquidity and volume
        all_markets.retain(|m| {
            m.liquidity >= filters.min_liquidity && m.volume >= filters.min_volume
        });
        
        // 5. Filter by active_only
        if filters.active_only {
            all_markets.retain(|m| m.active);
        }
        
        info!(count = all_markets.len(), "discovered markets from Gamma API");
        
        Ok(all_markets)
    }
}

/// Map a Gamma API `Market` response to our internal `MarketInfo`.
fn map_gamma_market(
    market: &polymarket_client_sdk::gamma::types::response::Market,
) -> Result<MarketInfo> {
    let condition_id = market
        .condition_id
        .map(|c| format!("{c:?}"))
        .unwrap_or_default();

    // Token IDs from the CLOB token_ids field.
    let token_ids: Vec<TokenId> = market
        .clob_token_ids
        .as_ref()
        .map(|ids| ids.iter().map(|id| TokenId(id.to_string())).collect())
        .unwrap_or_default();

    let outcomes: Vec<String> = market.outcomes.clone().unwrap_or_default();

    let end_date = market.end_date;

    let liquidity = market.liquidity.unwrap_or(Decimal::ZERO);
    let volume = market.volume.unwrap_or(Decimal::ZERO);
    let active = market.active.unwrap_or(false);
    let neg_risk = market.neg_risk.unwrap_or(false);
    
    // Category from tags (first tag as category)
    let category = market
        .tags
        .as_ref()
        .and_then(|tags| tags.first().map(|t| t.id.clone()))
        .unwrap_or_default();
    
    // All tags
    let tags = market
        .tags
        .as_ref()
        .map(|tags| tags.iter().map(|t| t.id.clone()).collect())
        .unwrap_or_default();
    
    // Outcome prices: try to parse from market.outcome_prices if available
    // For now, use empty HashMap as SDK may not expose this directly
    let outcome_prices: std::collections::HashMap<String, Decimal> = std::collections::HashMap::new();

    Ok(MarketInfo {
        id: MarketId(market.id.clone()),
        question: market.question.clone().unwrap_or_default(),
        slug: market.slug.clone().unwrap_or_default(),
        outcomes,
        token_ids,
        outcome_prices,
        condition_id,
        neg_risk,
        active,
        end_date,
        liquidity,
        volume,
        category,
        tags,
    })
}
