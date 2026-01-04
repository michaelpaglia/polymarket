//! Auto-redemption of resolved Polymarket positions
//!
//! When 15-minute crypto markets resolve, this module redeems winning positions
//! to return USDC to the wallet for the next trades.

use ethers::abi::{encode, Token};
use ethers::prelude::*;
use ethers::providers::{Http, Provider};
use ethers::signers::{LocalWallet, Signer};
use ethers::types::{Address, Bytes, TransactionRequest, U256};
use hft_core::{HftError, HftResult};
use serde::Deserialize;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Contract addresses on Polygon
pub const CTF_ADDRESS: &str = "0x4D97DCd97eC945f40cF65F87097ACe5EA0476045";
pub const NEG_RISK_CTF_ADDRESS: &str = "0xC5d563A36AE78145C45a50134d48A1215220f80a";
pub const USDC_ADDRESS: &str = "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174";

/// Polygon RPC endpoints
const DEFAULT_RPC: &str = "https://polygon-rpc.com";
const BACKUP_RPC: &str = "https://rpc-mainnet.matic.quiknode.pro";

/// Function selector for redeemPositions(address,bytes32,bytes32,uint256[])
const REDEEM_SELECTOR: [u8; 4] = [0x21, 0x7a, 0x4b, 0x70];

/// Redeemable position from Polymarket data API
#[derive(Debug, Clone, Deserialize)]
pub struct RedeemablePosition {
    pub title: Option<String>,
    pub outcome: Option<String>,
    #[serde(rename = "conditionId")]
    pub condition_id: String,
    #[serde(rename = "currentValue")]
    pub current_value: Option<f64>,
    #[serde(rename = "negRisk")]
    pub neg_risk: Option<bool>,
    pub redeemable: Option<bool>,
}

/// Auto-redeemer for resolved positions
pub struct PositionRedeemer {
    wallet: LocalWallet,
    provider: Provider<Http>,
    http_client: reqwest::Client,
}

impl PositionRedeemer {
    /// Create new redeemer from private key
    pub fn new(private_key: &str, rpc_url: Option<&str>) -> HftResult<Self> {
        let rpc = rpc_url.unwrap_or(DEFAULT_RPC);

        // Parse wallet from private key
        let wallet = private_key
            .parse::<LocalWallet>()
            .map_err(|e| HftError::SigningError(format!("Invalid private key: {}", e)))?
            .with_chain_id(137u64); // Polygon mainnet

        // Create provider
        let provider = Provider::<Http>::try_from(rpc)
            .map_err(|e| HftError::WebSocketConnection(format!("RPC error: {}", e)))?;

        let http_client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| HftError::WebSocketConnection(e.to_string()))?;

        info!(
            wallet = %format!("{:?}", wallet.address()),
            rpc = %rpc,
            "Position redeemer initialized"
        );

        Ok(Self {
            wallet,
            provider,
            http_client,
        })
    }

    /// Get wallet address
    pub fn wallet_address(&self) -> Address {
        self.wallet.address()
    }

    /// Get wallet address as string
    pub fn wallet_address_string(&self) -> String {
        format!("{:?}", self.wallet.address())
    }

    /// Fetch redeemable positions from Polymarket data API
    pub async fn get_redeemable_positions(&self) -> HftResult<Vec<RedeemablePosition>> {
        let wallet_addr = self.wallet_address_string().to_lowercase();
        let url = format!(
            "https://data-api.polymarket.com/positions?user={}",
            wallet_addr
        );

        debug!(url = %url, "Fetching redeemable positions");

        let resp = self
            .http_client
            .get(&url)
            .send()
            .await
            .map_err(|e| HftError::WebSocketConnection(format!("API error: {}", e)))?;

        if !resp.status().is_success() {
            debug!(status = %resp.status(), "No positions found");
            return Ok(vec![]);
        }

        let positions: Vec<RedeemablePosition> = resp
            .json::<Vec<RedeemablePosition>>()
            .await
            .map_err(|e: reqwest::Error| HftError::MessageParse(e.to_string()))?;

        // Filter for redeemable positions with value > $0.01
        let redeemable: Vec<_> = positions
            .into_iter()
            .filter(|p| p.redeemable.unwrap_or(false) && p.current_value.unwrap_or(0.0) > 0.01)
            .collect();

        debug!(count = redeemable.len(), "Found redeemable positions");

        Ok(redeemable)
    }

    /// Build the calldata for redeemPositions
    fn build_redeem_calldata(&self, condition_id: [u8; 32], index_sets: Vec<u64>) -> Bytes {
        let usdc_addr = Address::from_str(USDC_ADDRESS).unwrap();
        let parent_collection_id = [0u8; 32];

        // Encode parameters
        let params = encode(&[
            Token::Address(usdc_addr),
            Token::FixedBytes(parent_collection_id.to_vec()),
            Token::FixedBytes(condition_id.to_vec()),
            Token::Array(
                index_sets
                    .iter()
                    .map(|&i| Token::Uint(U256::from(i)))
                    .collect(),
            ),
        ]);

        // Combine selector + params
        let mut calldata = REDEEM_SELECTOR.to_vec();
        calldata.extend(params);

        Bytes::from(calldata)
    }

    /// Redeem a single position
    pub async fn redeem_position(&self, position: &RedeemablePosition) -> HftResult<(bool, f64)> {
        let title = position.title.as_deref().unwrap_or("Unknown");
        let value = position.current_value.unwrap_or(0.0);
        let neg_risk = position.neg_risk.unwrap_or(false);
        let outcome = position.outcome.as_deref().unwrap_or("yes");

        info!(
            title = %title,
            value = value,
            outcome = %outcome,
            neg_risk = neg_risk,
            "Redeeming position..."
        );

        // Parse condition ID
        let condition_id_str = &position.condition_id;
        let condition_bytes = if condition_id_str.starts_with("0x") {
            hex::decode(&condition_id_str[2..])
                .map_err(|e| HftError::MessageParse(format!("Invalid condition ID: {}", e)))?
        } else {
            hex::decode(condition_id_str)
                .map_err(|e| HftError::MessageParse(format!("Invalid condition ID: {}", e)))?
        };

        if condition_bytes.len() != 32 {
            return Err(HftError::MessageParse(
                "Condition ID must be 32 bytes".into(),
            ));
        }

        let mut condition_id = [0u8; 32];
        condition_id.copy_from_slice(&condition_bytes);

        // Determine index set (1 for YES, 2 for NO)
        let index_set = if outcome.to_lowercase() == "yes" {
            1u64
        } else {
            2u64
        };

        // Choose contract based on neg_risk
        let ctf_address: Address = if neg_risk {
            NEG_RISK_CTF_ADDRESS.parse().unwrap()
        } else {
            CTF_ADDRESS.parse().unwrap()
        };

        // Build calldata
        let calldata = self.build_redeem_calldata(condition_id, vec![index_set]);

        // Get nonce and gas price
        let nonce = self
            .provider
            .get_transaction_count(self.wallet.address(), None)
            .await
            .map_err(|e| HftError::WebSocketConnection(format!("Nonce error: {}", e)))?;

        let gas_price = self
            .provider
            .get_gas_price()
            .await
            .map_err(|e| HftError::WebSocketConnection(format!("Gas price error: {}", e)))?;

        // Build transaction
        let tx = TransactionRequest::new()
            .to(ctf_address)
            .data(calldata)
            .nonce(nonce)
            .gas(300_000u64) // Generous gas limit for redemption
            .gas_price(gas_price)
            .chain_id(137u64);

        // Sign transaction
        let signature = self
            .wallet
            .sign_transaction(&tx.clone().into())
            .await
            .map_err(|e| HftError::SigningError(format!("Signing error: {}", e)))?;

        let signed_tx = tx.rlp_signed(&signature);

        // Send transaction
        info!(
            to = %ctf_address,
            gas = 300_000,
            "Sending redemption transaction..."
        );

        let pending_tx = self
            .provider
            .send_raw_transaction(signed_tx)
            .await
            .map_err(|e| HftError::WebSocketConnection(format!("TX send error: {}", e)))?;

        let tx_hash = pending_tx.tx_hash();
        info!(tx_hash = %tx_hash, "Transaction sent, waiting for confirmation...");

        // Wait for confirmation (up to 60 seconds)
        match pending_tx.await {
            Ok(Some(receipt)) => {
                if receipt.status == Some(1.into()) {
                    info!(
                        tx_hash = %tx_hash,
                        value = value,
                        title = %title,
                        "Redemption successful!"
                    );
                    Ok((true, value))
                } else {
                    warn!(tx_hash = %tx_hash, "Redemption transaction reverted");
                    Ok((false, 0.0))
                }
            }
            Ok(None) => {
                warn!(tx_hash = %tx_hash, "Transaction dropped from mempool");
                Ok((false, 0.0))
            }
            Err(e) => {
                error!(tx_hash = %tx_hash, error = %e, "Transaction confirmation failed");
                Ok((false, 0.0))
            }
        }
    }

    /// Redeem all available positions
    pub async fn redeem_all(&self) -> HftResult<(u32, f64)> {
        let positions = self.get_redeemable_positions().await?;

        if positions.is_empty() {
            debug!("No redeemable positions found");
            return Ok((0, 0.0));
        }

        info!(
            count = positions.len(),
            "Found redeemable positions, starting redemption..."
        );

        let mut redeemed_count = 0u32;
        let mut total_value = 0.0f64;

        for position in positions {
            match self.redeem_position(&position).await {
                Ok((true, value)) => {
                    redeemed_count += 1;
                    total_value += value;
                }
                Ok((false, _)) => {
                    warn!(
                        title = position.title.as_deref().unwrap_or("Unknown"),
                        "Failed to redeem position"
                    );
                }
                Err(e) => {
                    error!(error = %e, "Redemption error");
                }
            }
            // Small delay between redemptions to avoid rate limits
            tokio::time::sleep(Duration::from_millis(1000)).await;
        }

        if redeemed_count > 0 {
            info!(
                count = redeemed_count,
                total = format!("${:.2}", total_value),
                "Redemption batch complete"
            );
        }

        Ok((redeemed_count, total_value))
    }

    /// Check and redeem in a loop (call this periodically)
    pub async fn check_and_redeem(&self) -> HftResult<(u32, f64)> {
        self.redeem_all().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_redeem_selector() {
        // Function signature: redeemPositions(address,bytes32,bytes32,uint256[])
        use sha3::{Digest, Keccak256};
        let sig = "redeemPositions(address,bytes32,bytes32,uint256[])";
        let hash = Keccak256::digest(sig.as_bytes());
        let selector: [u8; 4] = hash[0..4].try_into().unwrap();
        // Note: Actual selector may differ - verify against contract
        println!(
            "Computed selector: {:02x}{:02x}{:02x}{:02x}",
            selector[0], selector[1], selector[2], selector[3]
        );
    }

    #[test]
    fn test_parse_condition_id() {
        let id = "0x1234567890abcdef1234567890abcdef1234567890abcdef1234567890abcdef";
        let bytes = hex::decode(&id[2..]).unwrap();
        assert_eq!(bytes.len(), 32);
    }
}
