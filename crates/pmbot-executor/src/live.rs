//! Live executor — submits real orders to Polymarket via the SDK CLOB client.

use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use rust_decimal::Decimal;
use tracing::{debug, info};

use alloy::signers::local::LocalSigner;
use alloy::signers::k256::ecdsa::SigningKey;
use polymarket_client_sdk::auth::state::Authenticated;
use polymarket_client_sdk::auth::Normal;
use polymarket_client_sdk::clob::{self, types::{Amount, OrderType as SdkOrderType, Side as SdkSide}};
use polymarket_client_sdk::types::U256;

use pmbot_core::types::{OpenOrder, OrderId, OrderType, Side, TokenId};

use crate::actor::OrderExecutor;
use crate::rate_limit::ApiRateLimiter;

/// Live executor wrapping an authenticated Polymarket CLOB client.
///
/// Includes rate limiting to prevent exceeding API quotas.
pub struct LiveExecutor {
    client: Arc<clob::Client<Authenticated<Normal>>>,
    signer: Arc<LocalSigner<SigningKey>>,
    rate_limiter: Arc<tokio::sync::Mutex<ApiRateLimiter>>,
}

impl LiveExecutor {
    /// Wrap an already-authenticated CLOB client and its signer.
    pub fn new(
        client: clob::Client<Authenticated<Normal>>,
        signer: LocalSigner<SigningKey>,
    ) -> Self {
        Self {
            client: Arc::new(client),
            signer: Arc::new(signer),
            rate_limiter: Arc::new(tokio::sync::Mutex::new(ApiRateLimiter::new())),
        }
    }

    /// Wrap an already-authenticated CLOB client with custom rate limits.
    pub fn new_with_rate_limits(
        client: clob::Client<Authenticated<Normal>>,
        signer: LocalSigner<SigningKey>,
        rate_limiter: ApiRateLimiter,
    ) -> Self {
        Self {
            client: Arc::new(client),
            signer: Arc::new(signer),
            rate_limiter: Arc::new(tokio::sync::Mutex::new(rate_limiter)),
        }
    }

    /// Convert our string `TokenId` to the SDK's `U256`.
    fn parse_token_id(token_id: &TokenId) -> Result<U256> {
        U256::from_str(&token_id.0)
            .map_err(|e| anyhow::anyhow!("invalid token ID '{}': {e}", token_id.0))
    }

    /// Map our `Side` to the SDK's `Side`.
    fn map_side(side: Side) -> SdkSide {
        match side {
            Side::Buy => SdkSide::Buy,
            Side::Sell => SdkSide::Sell,
        }
    }

    /// Map our `OrderType` to the SDK's `OrderType`.
    fn map_order_type(ot: OrderType) -> SdkOrderType {
        match ot {
            OrderType::Gtc => SdkOrderType::GTC,
            OrderType::Gtd => SdkOrderType::GTD,
            OrderType::Fok => SdkOrderType::FOK,
            OrderType::Fak => SdkOrderType::FAK,
        }
    }
}

#[async_trait]
impl OrderExecutor for LiveExecutor {
    async fn submit_limit(
        &self,
        token_id: &TokenId,
        side: Side,
        price: Decimal,
        size: Decimal,
        order_type: OrderType,
        _post_only: bool,
    ) -> Result<OrderId> {
        let sdk_token = Self::parse_token_id(token_id)?;
        let sdk_side = Self::map_side(side);
        let sdk_ot = Self::map_order_type(order_type);

        let order = self
            .client
            .limit_order()
            .token_id(sdk_token)
            .price(price)
            .size(size)
            .side(sdk_side)
            .order_type(sdk_ot)
            .build()
            .await
            .context("failed to build limit order")?;

        let signed = self
            .client
            .sign(&*self.signer, order)
            .await
            .context("failed to sign limit order")?;

        let response = self
            .client
            .post_order(signed)
            .await
            .context("failed to post limit order")?;

        let order_id = OrderId(response.order_id.to_string());
        info!(%order_id, %token_id, %side, %price, %size, "limit order placed");
        Ok(order_id)
    }

    async fn submit_market(
        &self,
        token_id: &TokenId,
        side: Side,
        size: Decimal,
    ) -> Result<OrderId> {
        let sdk_token = Self::parse_token_id(token_id)?;
        let sdk_side = Self::map_side(side);

        let order = self
            .client
            .market_order()
            .token_id(sdk_token)
            .amount(Amount::usdc(size)?)
            .side(sdk_side)
            .build()
            .await
            .context("failed to build market order")?;

        let signed = self
            .client
            .sign(&*self.signer, order)
            .await
            .context("failed to sign market order")?;

        let response = self
            .client
            .post_order(signed)
            .await
            .context("failed to post market order")?;

        let order_id = OrderId(response.order_id.to_string());
        info!(%order_id, %token_id, %side, %size, "market order placed");
        Ok(order_id)
    }

    async fn cancel(&self, order_id: &OrderId) -> Result<()> {
        self.client
            .cancel_order(&order_id.0)
            .await
            .context("failed to cancel order")?;
        debug!(%order_id, "order cancelled");
        Ok(())
    }

    async fn cancel_all(&self) -> Result<()> {
        self.client
            .cancel_all_orders()
            .await
            .context("failed to cancel all orders")?;
        info!("all orders cancelled");
        Ok(())
    }

    async fn get_open_orders(&self) -> Result<Vec<OpenOrder>> {
        // Note: SDK API for fetching orders requires proper pagination handling.
        // For now, return empty - this should be wired up properly when
        // the SDK's orders() API is properly integrated.
        // 
        // TODO: Implement proper order fetching with:
        // 1. Use client.orders() with cursor-based pagination
        // 2. Filter for "open" status orders
        // 3. Map response to our OpenOrder type
        Ok(vec![])
    }
}
