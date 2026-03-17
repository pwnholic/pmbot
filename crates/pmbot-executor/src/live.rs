//! Live executor — submits real orders to Polymarket via the SDK CLOB client.

use std::str::FromStr;
use std::sync::Arc;

use anyhow::{Context, Result};
use async_trait::async_trait;
use rust_decimal::Decimal;
use tracing::{debug, info, warn};

use alloy::signers::k256::ecdsa::SigningKey;
use alloy::signers::local::LocalSigner;
use polymarket_client_sdk::auth::state::Authenticated;
use polymarket_client_sdk::auth::Normal;
use polymarket_client_sdk::clob::{
    self,
    types::{
        request::OrdersRequest, Amount, OrderStatusType, OrderType as SdkOrderType,
        Side as SdkSide,
    },
};
use polymarket_client_sdk::types::U256;

use pmbot_core::types::{MarketId, OpenOrder, OrderId, OrderType, Side, TokenId};

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

    /// Map SDK `Side` back to our `Side`.
    fn map_side_back(side: SdkSide) -> Side {
        match side {
            SdkSide::Buy => Side::Buy,
            SdkSide::Sell => Side::Sell,
            _ => Side::Buy, // fallback for future SDK variants
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

    /// Map SDK `OrderType` back to our `OrderType`.
    fn map_order_type_back(ot: &SdkOrderType) -> OrderType {
        match ot {
            SdkOrderType::GTC => OrderType::Gtc,
            SdkOrderType::GTD => OrderType::Gtd,
            SdkOrderType::FOK => OrderType::Fok,
            SdkOrderType::FAK => OrderType::Fak,
            _ => OrderType::Gtc, // fallback for future SDK variants
        }
    }

    /// Acquire a rate-limit token for order submission.
    async fn acquire_order_token(&self) {
        self.rate_limiter.lock().await.orders.acquire(1).await;
    }

    /// Acquire a rate-limit token for read (GET) requests.
    async fn acquire_read_token(&self) {
        self.rate_limiter.lock().await.reads.acquire(1).await;
    }

    /// Fetch the current USDC balance from Polymarket.
    ///
    /// This queries the CLOB API for the account's available USDC balance.
    pub async fn get_balance(&self) -> Result<Decimal> {
        self.acquire_read_token().await;
        
        let request = polymarket_client_sdk::clob::types::request::BalanceAllowanceRequest::default();
        
        let response = self
            .client
            .balance_allowance(request)
            .await
            .context("failed to fetch balance from CLOB API")?;
        
        info!(
            balance = %response.balance,
            "fetched live balance from Polymarket"
        );
        
        Ok(response.balance)
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
        post_only: bool,
    ) -> Result<OrderId> {
        self.acquire_order_token().await;

        let sdk_token = Self::parse_token_id(token_id)?;
        let sdk_side = Self::map_side(side);
        let sdk_ot = Self::map_order_type(order_type);

        let mut builder = self
            .client
            .limit_order()
            .token_id(sdk_token)
            .price(price)
            .size(size)
            .side(sdk_side)
            .order_type(sdk_ot);

        // post_only is only valid for GTC/GTD orders
        if post_only
            && matches!(order_type, OrderType::Gtc | OrderType::Gtd)
        {
            builder = builder.post_only(true);
        }

        let order = builder
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

        if !response.success {
            let err_msg = response
                .error_msg
                .unwrap_or_else(|| "unknown error".to_string());
            warn!(%token_id, %side, %price, %size, %err_msg, "limit order rejected by CLOB");
            anyhow::bail!("limit order rejected: {err_msg}");
        }

        let order_id = OrderId(response.order_id.to_string());
        info!(%order_id, %token_id, %side, %price, %size, status = %response.status, "limit order placed");
        Ok(order_id)
    }

    async fn submit_market(
        &self,
        token_id: &TokenId,
        side: Side,
        size: Decimal,
    ) -> Result<OrderId> {
        self.acquire_order_token().await;

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

        if !response.success {
            let err_msg = response
                .error_msg
                .unwrap_or_else(|| "unknown error".to_string());
            warn!(%token_id, %side, %size, %err_msg, "market order rejected by CLOB");
            anyhow::bail!("market order rejected: {err_msg}");
        }

        let order_id = OrderId(response.order_id.to_string());
        info!(%order_id, %token_id, %side, %size, status = %response.status, "market order placed");
        Ok(order_id)
    }

    async fn cancel(&self, order_id: &OrderId) -> Result<()> {
        self.acquire_order_token().await;

        self.client
            .cancel_order(&order_id.0)
            .await
            .context("failed to cancel order")?;
        debug!(%order_id, "order cancelled");
        Ok(())
    }

    async fn cancel_all(&self) -> Result<()> {
        self.acquire_order_token().await;

        self.client
            .cancel_all_orders()
            .await
            .context("failed to cancel all orders")?;
        info!("all orders cancelled");
        Ok(())
    }

    async fn get_open_orders(&self) -> Result<Vec<OpenOrder>> {
        self.acquire_read_token().await;

        let request = OrdersRequest::default();
        let mut all_orders = Vec::new();
        let mut cursor: Option<String> = None;

        loop {
            let page = self
                .client
                .orders(&request, cursor)
                .await
                .context("failed to fetch open orders")?;

            for sdk_order in &page.data {
                // Only include live (open) orders
                if sdk_order.status != OrderStatusType::Live {
                    continue;
                }

                let filled = sdk_order.size_matched;
                let remaining = sdk_order.original_size - filled;

                all_orders.push(OpenOrder {
                    order_id: OrderId(sdk_order.id.clone()),
                    market_id: MarketId(format!("{:?}", sdk_order.market)),
                    token_id: TokenId(sdk_order.asset_id.to_string()),
                    side: Self::map_side_back(sdk_order.side),
                    price: sdk_order.price,
                    size: remaining,
                    filled,
                    order_type: Self::map_order_type_back(&sdk_order.order_type),
                });
            }

            // If we got fewer than `limit` results, we've reached the last page
            if page.count < page.limit || page.next_cursor.is_empty() {
                break;
            }

            // Rate-limit subsequent pages
            self.acquire_read_token().await;
            cursor = Some(page.next_cursor);
        }

        debug!(count = all_orders.len(), "fetched open orders from CLOB");
        Ok(all_orders)
    }
}
