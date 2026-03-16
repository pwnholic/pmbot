//! Gamma API discovery — fetches live markets from Polymarket's Gamma service.

use anyhow::{Context, Result};
use async_trait::async_trait;
use rust_decimal::Decimal;
use tracing::{debug, info};

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
}

impl Default for GammaDiscovery {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl MarketDiscovery for GammaDiscovery {
    async fn discover(&self, filters: &DiscoveryFilters) -> Result<Vec<MarketInfo>> {
        let request = MarketsRequest::builder()
            .liquidity_num_min(filters.min_liquidity)
            .volume_num_min(filters.min_volume)
            .closed(false)
            .build();

        let raw_markets = self
            .client
            .markets(&request)
            .await
            .context("failed to fetch markets from Gamma API")?;

        let mut results = Vec::new();

        for market in &raw_markets {
            let info = match map_gamma_market(market) {
                Ok(info) => info,
                Err(e) => {
                    debug!(id = %market.id, error = %e, "skipping malformed market");
                    continue;
                }
            };

            // Apply remaining filters not handled by API query params.
            if filters.active_only && !info.active {
                continue;
            }

            results.push(info);
        }

        info!(count = results.len(), "discovered markets from Gamma API");
        Ok(results)
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

    let outcomes: Vec<String> = market
        .outcomes
        .clone()
        .unwrap_or_default();

    let end_date = market.end_date;

    let liquidity = market.liquidity.unwrap_or(Decimal::ZERO);
    let volume = market.volume.unwrap_or(Decimal::ZERO);
    let active = market.active.unwrap_or(false);
    let neg_risk = false; // Gamma Market struct doesn't expose neg_risk directly

    Ok(MarketInfo {
        id: MarketId(market.id.clone()),
        question: market.question.clone().unwrap_or_default(),
        slug: market.slug.clone().unwrap_or_default(),
        outcomes,
        token_ids,
        condition_id,
        neg_risk,
        active,
        end_date,
        liquidity,
        volume,
    })
}
