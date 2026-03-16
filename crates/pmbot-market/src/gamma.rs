//! Gamma API discovery — fetches live markets from Polymarket's Gamma service.

use anyhow::{Context, Result};
use async_trait::async_trait;
use rust_decimal::Decimal;
use tracing::{debug, info, warn};

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
    fn generate_btc_5m_slugs(lookahead_windows: usize) -> Vec<String> {
        let now = chrono::Utc::now().timestamp() as u64;
        let boundary_secs = 300u64; // 5 minutes

        // Current 5-min boundary
        let current_boundary = (now / boundary_secs) * boundary_secs;

        let mut slugs = Vec::new();

        // Include 1 past window (might still be open/tradeable) + current + lookahead
        for i in 0..=(lookahead_windows + 1) {
            let ts = current_boundary + (i as u64 * boundary_secs);
            // Also include the boundary that started before now
            if i == 0 {
                // The window that is currently in-progress
                slugs.push(format!("btc-updown-5m-{ts}"));
                // Also the previous one (might still be accepting orders near the end)
                if current_boundary >= boundary_secs {
                    slugs.push(format!("btc-updown-5m-{}", current_boundary - boundary_secs));
                }
            } else {
                slugs.push(format!("btc-updown-5m-{ts}"));
            }
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
        // Check if we should use slug-based discovery for BTC 5-minute markets.
        let is_btc_5m = filters.keyword.to_lowercase().contains("btc")
            && (filters.market_type.contains("5m") || filters.market_type.contains("5min"));

        let raw_markets = if is_btc_5m {
            // Slug-based discovery: compute candidate slugs and query directly.
            let slugs = Self::generate_btc_5m_slugs(6); // current + 6 upcoming windows

            info!(
                slug_count = slugs.len(),
                first = %slugs.first().unwrap_or(&String::new()),
                "querying Gamma API with computed BTC 5m slugs"
            );

            let request = MarketsRequest::builder()
                .slug(slugs)
                .closed(false)
                .build();

            self.client
                .markets(&request)
                .await
                .context("failed to fetch BTC 5m markets from Gamma API")?
        } else {
            // Fallback: generic filtered query for other market types.
            let request = MarketsRequest::builder()
                .limit(200)
                .liquidity_num_min(filters.min_liquidity)
                .volume_num_min(filters.min_volume)
                .closed(false)
                .build();

            let markets = self
                .client
                .markets(&request)
                .await
                .context("failed to fetch markets from Gamma API")?;

            markets
        };

        debug!(raw_count = raw_markets.len(), "raw markets from Gamma API");

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

            // For generic (non-slug) discovery, apply keyword and market_type filters
            if !is_btc_5m {
                // Filter by market_type (e.g., "5min", "15min") in slug
                if !filters.market_type.is_empty()
                    && !filters.market_type.to_lowercase().contains("all")
                {
                    let slug_lower = info.slug.to_lowercase();
                    let market_type_lower = filters.market_type.to_lowercase();
                    if !slug_lower.contains(&market_type_lower) {
                        continue;
                    }
                }

                // Filter by keyword (e.g., "BTC", "ETH")
                if !filters.keyword.is_empty() {
                    let slug_lower = info.slug.to_lowercase();
                    let question_lower = info.question.to_lowercase();
                    let keyword_lower = filters.keyword.to_lowercase();
                    if !slug_lower.contains(&keyword_lower)
                        && !question_lower.contains(&keyword_lower)
                    {
                        continue;
                    }
                }
            }

            // Skip markets with very low liquidity (accepting orders but nearly empty)
            if info.liquidity < filters.min_liquidity {
                debug!(
                    slug = %info.slug,
                    liquidity = %info.liquidity,
                    min = %filters.min_liquidity,
                    "skipping low-liquidity market"
                );
                continue;
            }

            results.push(info);
        }

        info!(count = results.len(), "discovered markets from Gamma API");

        if results.is_empty() && is_btc_5m {
            warn!(
                "no BTC 5m markets found - this may mean markets are between creation cycles"
            );
        }

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

    let outcomes: Vec<String> = market.outcomes.clone().unwrap_or_default();

    let end_date = market.end_date;

    let liquidity = market.liquidity.unwrap_or(Decimal::ZERO);
    let volume = market.volume.unwrap_or(Decimal::ZERO);
    let active = market.active.unwrap_or(false);
    let neg_risk = market.neg_risk.unwrap_or(false);

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
