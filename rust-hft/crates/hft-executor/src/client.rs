//! Order execution client for Polymarket CLOB

use crate::signer::{OrderSigner, SignedOrder};
use chrono::Utc;
use dashmap::DashMap;
use ethers::signers::{LocalWallet, Signer};
use hft_core::{
    ArbitrageExecution, ArbitrageOpportunity, ExecutionStatus, HftError, HftResult, Side,
};
use base64::{engine::general_purpose::URL_SAFE, Engine};
use hmac::{Hmac, Mac};
use reqwest::{Client, Proxy};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

type HmacSha256 = Hmac<Sha256>;

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
    /// Optional HTTP proxy URL for order submission (format: http://user:pass@host:port)
    pub proxy_url: Option<String>,
}

impl Default for ExecutorConfig {
    fn default() -> Self {
        Self {
            clob_url: CLOB_URL.to_string(),
            order_timeout_ms: 5000,
            max_retries: 2,
            fee_rate_bps: 0, // No fees for takers
            proxy_url: None,
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
#[allow(dead_code)]
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

/// Response from derive API credentials endpoint
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeriveCredsResponse {
    api_key: String,
    secret: String,
    passphrase: String,
}

impl OrderExecutor {
    /// Create new order executor with auto-derived credentials (like Python py_clob_client)
    pub async fn new_with_derived_creds(
        config: ExecutorConfig,
        private_key: &str,
        chain_id: u64,
    ) -> HftResult<Self> {
        let signer = OrderSigner::new(private_key, chain_id)?;

        // Derive credentials from private key (like Python's create_or_derive_api_creds)
        let credentials = Self::derive_api_credentials(&config.clob_url, private_key).await?;

        let mut client_builder = Client::builder()
            .timeout(std::time::Duration::from_millis(config.order_timeout_ms))
            .tcp_nodelay(true);

        // Add proxy if configured
        if let Some(proxy_url) = &config.proxy_url {
            let proxy = Proxy::all(proxy_url)
                .map_err(|e| HftError::Internal(format!("Invalid proxy URL: {}", e)))?;
            client_builder = client_builder.proxy(proxy);
            info!(proxy = %proxy_url.split('@').last().unwrap_or(proxy_url), "Using HTTP proxy for orders");
        }

        let http_client = client_builder
            .build()
            .map_err(|e| HftError::Internal(e.to_string()))?;

        info!(
            address = %signer.address_hex(),
            "Order executor initialized with derived credentials"
        );

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

    /// Derive API credentials from private key (matches py_clob_client behavior)
    /// Uses EIP-712 typed data signing with POLY_* headers
    async fn derive_api_credentials(
        clob_url: &str,
        private_key: &str,
    ) -> HftResult<ApiCredentials> {
        let wallet: LocalWallet = private_key
            .parse::<LocalWallet>()
            .map_err(|e| HftError::SigningError(format!("Invalid private key: {}", e)))?
            .with_chain_id(137u64); // Polygon mainnet

        let address = format!("{:?}", wallet.address());

        // Create timestamp for signing (Unix seconds)
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before UNIX epoch")
            .as_secs();

        let nonce: u64 = 0;

        // Build EIP-712 typed data for ClobAuth
        // Domain: ClobAuthDomain v1, chainId=137
        // Message: "This message attests that I control the given wallet"
        let msg_to_sign = "This message attests that I control the given wallet";

        // Build the typed data hash manually matching py_clob_client
        use sha3::{Digest, Keccak256};

        // Domain separator
        let domain_type_hash =
            Keccak256::digest(b"EIP712Domain(string name,string version,uint256 chainId)");
        let name_hash = Keccak256::digest(b"ClobAuthDomain");
        let version_hash = Keccak256::digest(b"1");

        let mut domain_data = Vec::new();
        domain_data.extend_from_slice(&domain_type_hash);
        domain_data.extend_from_slice(&name_hash);
        domain_data.extend_from_slice(&version_hash);
        // chainId as uint256
        let mut chain_id_bytes = [0u8; 32];
        chain_id_bytes[31] = 137u8; // Polygon chainId = 137
        domain_data.extend_from_slice(&chain_id_bytes);

        let domain_separator = Keccak256::digest(&domain_data);

        // Message type hash
        let type_hash = Keccak256::digest(
            b"ClobAuth(address address,string timestamp,uint256 nonce,string message)",
        );

        // Encode the message struct
        let timestamp_str = timestamp.to_string();
        let timestamp_hash = Keccak256::digest(timestamp_str.as_bytes());
        let message_hash = Keccak256::digest(msg_to_sign.as_bytes());

        // address as bytes32 (left-padded)
        let mut addr_bytes = [0u8; 32];
        addr_bytes[12..32].copy_from_slice(wallet.address().as_bytes());

        // nonce as uint256
        let mut nonce_bytes = [0u8; 32];
        nonce_bytes[24..32].copy_from_slice(&nonce.to_be_bytes());

        let mut struct_data = Vec::new();
        struct_data.extend_from_slice(&type_hash);
        struct_data.extend_from_slice(&addr_bytes);
        struct_data.extend_from_slice(&timestamp_hash);
        struct_data.extend_from_slice(&nonce_bytes);
        struct_data.extend_from_slice(&message_hash);

        let struct_hash = Keccak256::digest(&struct_data);

        // Final hash: keccak256("\x19\x01" || domain_separator || struct_hash)
        let mut final_data = Vec::new();
        final_data.push(0x19);
        final_data.push(0x01);
        final_data.extend_from_slice(&domain_separator);
        final_data.extend_from_slice(&struct_hash);

        let digest = Keccak256::digest(&final_data);

        // Sign the hash - convert to [u8; 32] for H256
        let mut hash_bytes = [0u8; 32];
        hash_bytes.copy_from_slice(&digest);
        let signature = wallet
            .sign_hash(hash_bytes.into())
            .map_err(|e| HftError::SigningError(format!("Failed to sign: {}", e)))?;

        let sig_hex = format!("0x{}", hex::encode(signature.to_vec()));

        // Call derive endpoint with GET and POLY_* headers
        let client = Client::new();
        let url = format!("{}/auth/derive-api-key", clob_url);

        let response = client
            .get(&url)
            .header("POLY_ADDRESS", &address)
            .header("POLY_SIGNATURE", &sig_hex)
            .header("POLY_TIMESTAMP", timestamp.to_string())
            .header("POLY_NONCE", nonce.to_string())
            .send()
            .await
            .map_err(|e| HftError::HttpError(format!("Derive creds request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(HftError::HttpError(format!(
                "Derive creds failed ({}): {}",
                status, body
            )));
        }

        let creds: DeriveCredsResponse = response
            .json()
            .await
            .map_err(|e| HftError::Internal(format!("Failed to parse credentials: {}", e)))?;

        info!(
            address = %address,
            api_key_prefix = &creds.api_key[..8.min(creds.api_key.len())],
            "API credentials derived successfully"
        );

        Ok(ApiCredentials {
            api_key: creds.api_key,
            api_secret: creds.secret,
            api_passphrase: creds.passphrase,
        })
    }

    /// Create new order executor with explicit credentials
    pub fn new(
        config: ExecutorConfig,
        private_key: &str,
        credentials: ApiCredentials,
        chain_id: u64,
    ) -> HftResult<Self> {
        let signer = OrderSigner::new(private_key, chain_id)?;

        let mut client_builder = Client::builder()
            .timeout(std::time::Duration::from_millis(config.order_timeout_ms))
            .tcp_nodelay(true);

        // Add proxy if configured
        if let Some(proxy_url) = &config.proxy_url {
            let proxy = Proxy::all(proxy_url)
                .map_err(|e| HftError::Internal(format!("Invalid proxy URL: {}", e)))?;
            client_builder = client_builder.proxy(proxy);
            info!(proxy = %proxy_url.split('@').last().unwrap_or(proxy_url), "Using HTTP proxy for orders");
        }

        let http_client = client_builder
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

        self.signer.sign_order(&order, &self.credentials.api_key).await
    }

    /// Build HMAC signature for Level 2 auth (like Python's build_hmac_signature)
    fn build_hmac_signature(&self, timestamp: i64, method: &str, request_path: &str, body: &str) -> HftResult<String> {
        // Decode base64 secret
        let secret_bytes = URL_SAFE
            .decode(&self.credentials.api_secret)
            .map_err(|e| HftError::SigningError(format!("Failed to decode API secret: {}", e)))?;

        // Build message: timestamp + method + path + body
        let message = format!("{}{}{}{}", timestamp, method, request_path, body);

        // HMAC-SHA256
        let mut mac = HmacSha256::new_from_slice(&secret_bytes)
            .map_err(|e| HftError::SigningError(format!("Failed to create HMAC: {}", e)))?;
        mac.update(message.as_bytes());
        let result = mac.finalize();

        // Base64 encode
        Ok(URL_SAFE.encode(result.into_bytes()))
    }

    /// Submit order to CLOB
    async fn submit_order(&self, order: &SignedOrder) -> HftResult<Option<String>> {
        self.orders_submitted.fetch_add(1, Ordering::Relaxed);

        let request_path = "/order";
        let url = format!("{}{}", self.config.clob_url, request_path);

        // Serialize body for HMAC
        let body = serde_json::to_string(order)
            .map_err(|e| HftError::Internal(format!("Failed to serialize order: {}", e)))?;

        // Log the order payload to debug format issues
        info!(payload = %body, "Order payload being submitted");

        // Get timestamp
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before UNIX epoch")
            .as_secs() as i64;

        // Build HMAC signature
        let hmac_sig = self.build_hmac_signature(timestamp, "POST", request_path, &body)?;

        let response = self
            .http_client
            .post(&url)
            .header("POLY_ADDRESS", self.signer.address_hex())
            .header("POLY_SIGNATURE", &hmac_sig)
            .header("POLY_TIMESTAMP", timestamp.to_string())
            .header("POLY_API_KEY", &self.credentials.api_key)
            .header("POLY_PASSPHRASE", &self.credentials.api_passphrase)
            .header("Content-Type", "application/json")
            .body(body)
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
        let request_path = format!("/order/{}", order_id);
        let url = format!("{}{}", self.config.clob_url, request_path);

        // Get timestamp for HMAC
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before UNIX epoch")
            .as_secs() as i64;

        // Build HMAC signature (empty body for GET)
        let hmac_sig = self.build_hmac_signature(timestamp, "GET", &request_path, "")?;

        let response = self
            .http_client
            .get(&url)
            .header("POLY_ADDRESS", self.signer.address_hex())
            .header("POLY_SIGNATURE", &hmac_sig)
            .header("POLY_TIMESTAMP", timestamp.to_string())
            .header("POLY_API_KEY", &self.credentials.api_key)
            .header("POLY_PASSPHRASE", &self.credentials.api_passphrase)
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
        let request_path = format!("/order/{}", order_id);
        let url = format!("{}{}", self.config.clob_url, request_path);

        // Get timestamp for HMAC
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time before UNIX epoch")
            .as_secs() as i64;

        // Build HMAC signature (empty body for DELETE)
        let hmac_sig = self.build_hmac_signature(timestamp, "DELETE", &request_path, "")?;

        let response = self
            .http_client
            .delete(&url)
            .header("POLY_ADDRESS", self.signer.address_hex())
            .header("POLY_SIGNATURE", &hmac_sig)
            .header("POLY_TIMESTAMP", timestamp.to_string())
            .header("POLY_API_KEY", &self.credentials.api_key)
            .header("POLY_PASSPHRASE", &self.credentials.api_passphrase)
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

    /// Create and submit a single order (convenience method for crypto latency trading)
    pub async fn create_and_submit_order(
        &self,
        token_id: &str,
        side: Side,
        price: Decimal,
        size: Decimal,
    ) -> HftResult<Option<String>> {
        let signed_order = self
            .create_signed_order(token_id, side, price, size)
            .await?;
        self.submit_order(&signed_order).await
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
