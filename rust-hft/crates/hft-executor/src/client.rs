//! Order execution client for Polymarket CLOB

use crate::signer::{OrderSigner, SignedOrder};
use chrono::Utc;
use dashmap::DashMap;
use hft_core::{
    ArbitrageExecution, ArbitrageOpportunity, ExecutionStatus, HftError, HftResult, OrderType, Side,
};
use reqwest::Client;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// CLOB API base URL
pub const CLOB_URL: &str = "https://clob.polymarket.com";

/// Order executor configuration
#[derive(Debug, Clone)]
pub struct ExecutorConfig {
    /// CLOB API URL
    pub clob_url: String,
    /// Order timeout in milliseconds
    pub order_timeout_ms: u64,
    /// Maximum retries for failed orders
    pub max_retries: u32,
    /// Fee rate in basis points
    pub fee_rate_bps: u32,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            clob_url: CLOB_URL.to_string(),
            order_timeout_ms: 5000,
            max_retries: 2,
            fee_rate_bps: 0, // No fees for takers
        }
    }
}

/// API credentials
#[derive(Debug, Clone)]
pub struct ApiCredentials {
    pub api_key: String,
    pub api_secret: String,
    pub api_passphrase: String,
}

/// Order response from CLOB
#[derive(Debug, Clone, Deserialize)]
pub struct OrderResponse {
    pub order_id: Option<String>,
    pub status: Option<String>,
    pub error: Option<String>,
    pub error_code: Option<i32>,
}

/// Order status
#[derive(Debug, Clone, Deserialize)]
pub struct OrderStatus {
    pub order_id: String,
    pub status: String,
    pub filled_amount: Option<String>,
    pub remaining_amount: Option<String>,
}

/// Pending order tracking
#[derive(Debug, Clone)]
struct PendingOrder {
    order_id: String,
    token_id: String,
    side: Side,
    submitted_at: Instant,
}

/// Order executor
pub struct OrderExecutor {
    config: ExecutorConfig,
    signer: OrderSigner,
    credentials: ApiCredentials,
    http_client: Client,
    pending_orders: DashMap<String, PendingOrder>,
    orders_submitted: AtomicU64,
    orders_filled: AtomicU64,
    orders_failed: AtomicU64,
}

impl OrderExecutor {
    /// Create new order executor
    pub fn new(
        config: ExecutorConfig,
        private_key: &str,
        credentials: ApiCredentials,
        chain_id: u64,
    ) -> HftResult<Self> {
        let signer = OrderSigner::new(private_key, chain_id)?;

        let http_client = Client::builder()
            .timeout(std::time::Duration::from_millis(config.order_timeout_ms))
            .tcp_nodelay(true)
            .build()
            .map_err(|e| HftError::Internal(e.to_string()))?;

        Ok(Self {
            config,
            signer,
            credentials,
            http_client,
            pending_orders: DashMap::new(),
            orders_submitted: AtomicU64::new(0),
            orders_filled: AtomicU64::new(0),
            orders_failed: AtomicU64::new(0),
        })
    }

    /// Get maker address
    pub fn address(&self) -> String {
        self.signer.address_hex()
    }

    /// Execute arbitrage trade (buy both YES and NO)
    pub async fn execute_arbitrage(
        &self,
        opportunity: &ArbitrageOpportunity,
    ) -> HftResult<ArbitrageExecution> {
        let execution_id = Uuid::new_v4().to_string();
        let start = Instant::now();

        info!(
            execution_id = %execution_id,
            market_id = %opportunity.market_id,
            yes_price = %opportunity.yes_price,
            no_price = %opportunity.no_price,
            spread_bps = opportunity.profit_bps,
            "Executing arbitrage"
        );

        // Calculate share amounts
        let yes_shares = opportunity.max_size_usd / opportunity.yes_price;
        let no_shares = opportunity.max_size_usd / opportunity.no_price;

        // Create and sign orders
        let yes_order = self
            .create_signed_order(
                &opportunity.yes_token_id.0,
                Side::Buy,
                opportunity.yes_price,
                yes_shares,
            )
            .await?;

        let no_order = self
            .create_signed_order(
                &opportunity.no_token_id.0,
                Side::Buy,
                opportunity.no_price,
                no_shares,
            )
            .await?;

        // Submit orders concurrently
        let (yes_result, no_result) =
            tokio::join!(self.submit_order(&yes_order), self.submit_order(&no_order),);

        let execution_time = start.elapsed();

        let yes_filled = yes_result.is_ok();
        let no_filled = no_result.is_ok();

        let status = match (yes_filled, no_filled) {
            (true, true) => ExecutionStatus::Success,
            (true, false) | (false, true) => ExecutionStatus::PartialFill,
            (false, false) => ExecutionStatus::Failed,
        };

        if status == ExecutionStatus::Success {
            self.orders_filled.fetch_add(2, Ordering::Relaxed);
            info!(
                execution_id = %execution_id,
                execution_us = execution_time.as_micros(),
                "Arbitrage executed successfully"
            );
        } else {
            self.orders_failed.fetch_add(
                if yes_filled || no_filled { 1 } else { 2 },
                Ordering::Relaxed,
            );
            warn!(
                execution_id = %execution_id,
                yes_ok = yes_filled,
                no_ok = no_filled,
                "Arbitrage partially failed"
            );
        }

        let total_cost = opportunity.max_size_usd * Decimal::TWO;
        let expected_profit = opportunity.spread * opportunity.max_size_usd;

        Ok(ArbitrageExecution {
            execution_id,
            opportunity: opportunity.clone(),
            yes_order_id: yes_result.ok().flatten(),
            no_order_id: no_result.ok().flatten(),
            yes_filled,
            no_filled,
            yes_fill_price: if yes_filled {
                Some(opportunity.yes_price)
            } else {
                None
            },
            no_fill_price: if no_filled {
                Some(opportunity.no_price)
            } else {
                None
            },
            total_cost_usd: total_cost,
            expected_profit_usd: expected_profit,
            execution_time_us: execution_time.as_micros() as u64,
            executed_at: Utc::now(),
            status,
        })
    }

    /// Create and sign a single order
    async fn create_signed_order(
        &self,
        token_id: &str,
        side: Side,
        price: Decimal,
        size: Decimal,
    ) -> HftResult<SignedOrder> {
        let order = self
            .signer
            .create_order(token_id, side, price, size, self.config.fee_rate_bps);

        self.signer.sign_order(&order).await
    }

    /// Submit order to CLOB
    async fn submit_order(&self, order: &SignedOrder) -> HftResult<Option<String>> {
        self.orders_submitted.fetch_add(1, Ordering::Relaxed);

        let url = format!("{}/order", self.config.clob_url);

        let response = self
            .http_client
            .post(&url)
            .header("POLY-ADDRESS", self.signer.address_hex())
            .header("POLY-API-KEY", &self.credentials.api_key)
            .header("POLY-API-SECRET", &self.credentials.api_secret)
            .header("POLY-API-PASSPHRASE", &self.credentials.api_passphrase)
            .header("Content-Type", "application/json")
            .json(order)
            .send()
            .await
            .map_err(|e| HftError::HttpError(e.to_string()))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| HftError::HttpError(e.to_string()))?;

        if status.is_success() {
            let order_resp: OrderResponse = serde_json::from_str(&body)
                .map_err(|e| HftError::Internal(format!("Failed to parse response: {}", e)))?;

            if let Some(order_id) = order_resp.order_id {
                debug!(order_id = %order_id, "Order submitted successfully");
                return Ok(Some(order_id));
            }

            if let Some(error) = order_resp.error {
                return Err(HftError::OrderRejected {
                    reason: error,
                    code: order_resp.error_code,
                });
            }

            Ok(None)
        } else if status.as_u16() == 429 {
            // Rate limited
            Err(HftError::RateLimited {
                retry_after_ms: 1000,
            })
        } else {
            error!(status = %status, body = %body, "Order rejected");
            Err(HftError::OrderRejected {
                reason: body,
                code: Some(status.as_u16() as i32),
            })
        }
    }

    /// Get order status
    pub async fn get_order_status(&self, order_id: &str) -> HftResult<OrderStatus> {
        let url = format!("{}/order/{}", self.config.clob_url, order_id);

        let response = self
            .http_client
            .get(&url)
            .header("POLY-ADDRESS", self.signer.address_hex())
            .header("POLY-API-KEY", &self.credentials.api_key)
            .header("POLY-API-SECRET", &self.credentials.api_secret)
            .header("POLY-API-PASSPHRASE", &self.credentials.api_passphrase)
            .send()
            .await
            .map_err(|e| HftError::HttpError(e.to_string()))?;

        if response.status().is_success() {
            response
                .json()
                .await
                .map_err(|e| HftError::Internal(e.to_string()))
        } else {
            Err(HftError::HttpError(format!(
                "Failed to get order status: {}",
                response.status()
            )))
        }
    }

    /// Cancel order
    pub async fn cancel_order(&self, order_id: &str) -> HftResult<()> {
        let url = format!("{}/order/{}", self.config.clob_url, order_id);

        let response = self
            .http_client
            .delete(&url)
            .header("POLY-ADDRESS", self.signer.address_hex())
            .header("POLY-API-KEY", &self.credentials.api_key)
            .header("POLY-API-SECRET", &self.credentials.api_secret)
            .header("POLY-API-PASSPHRASE", &self.credentials.api_passphrase)
            .send()
            .await
            .map_err(|e| HftError::HttpError(e.to_string()))?;

        if response.status().is_success() {
            Ok(())
        } else {
            Err(HftError::HttpError(format!(
                "Failed to cancel order: {}",
                response.status()
            )))
        }
    }

    /// Get statistics
    pub fn stats(&self) -> ExecutorStats {
        ExecutorStats {
            orders_submitted: self.orders_submitted.load(Ordering::Relaxed),
            orders_filled: self.orders_filled.load(Ordering::Relaxed),
            orders_failed: self.orders_failed.load(Ordering::Relaxed),
            pending_count: self.pending_orders.len(),
        }
    }
}

/// Executor statistics
#[derive(Debug, Clone, Serialize)]
pub struct ExecutorStats {
    pub orders_submitted: u64,
    pub orders_filled: u64,
    pub orders_failed: u64,
    pub pending_count: usize,
}
