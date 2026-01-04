//! EIP-712 order signing for Polymarket

use ethers::signers::{LocalWallet, Signer};
use ethers::types::{Address, U256};
use hft_core::{HftError, HftResult, Side};
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use sha3::{Digest, Keccak256};
use std::str::FromStr;

/// Polymarket CLOB contract address on Polygon
pub const CLOB_CONTRACT: &str = "0x4bFb41d5B3570DeFd03C39a9A4D8dE6Bd8B8982E";

/// USDC contract address on Polygon
pub const USDC_CONTRACT: &str = "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174";

/// Chain ID for Polygon mainnet
pub const POLYGON_CHAIN_ID: u64 = 137;

/// Order data for signing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrderData {
    pub maker: String,
    pub taker: String,
    pub token_id: String,
    pub maker_amount: String,
    pub taker_amount: String,
    pub side: u8,
    pub fee_rate_bps: String,
    pub nonce: String,
    pub expiration: String,
    pub signature_type: u8,
}

/// Signed order ready for submission
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignedOrder {
    pub order: OrderData,
    pub signature: String,
}

/// Order signer for Polymarket
pub struct OrderSigner {
    wallet: LocalWallet,
    chain_id: u64,
}

impl OrderSigner {
    /// Create new order signer from private key
    pub fn new(private_key: &str, chain_id: u64) -> HftResult<Self> {
        let wallet = private_key
            .parse::<LocalWallet>()
            .map_err(|e| HftError::SigningError(e.to_string()))?
            .with_chain_id(chain_id);

        Ok(Self { wallet, chain_id })
    }

    /// Get wallet address
    pub fn address(&self) -> Address {
        self.wallet.address()
    }

    /// Get address as hex string
    pub fn address_hex(&self) -> String {
        format!("{:?}", self.wallet.address())
    }

    /// Create order data
    pub fn create_order(
        &self,
        token_id: &str,
        side: Side,
        price: Decimal,
        size: Decimal,
        fee_rate_bps: u32,
    ) -> OrderData {
        let maker = self.address_hex();
        let taker = "0x0000000000000000000000000000000000000000".to_string();

        // Calculate amounts based on side
        // For BUY: maker_amount = USD to spend, taker_amount = shares to receive
        // For SELL: maker_amount = shares to sell, taker_amount = USD to receive
        let (maker_amount, taker_amount) = match side {
            Side::Buy => {
                let usd_amount = price * size;
                let shares = size;
                (
                    decimal_to_usdc_units(usd_amount),
                    decimal_to_share_units(shares),
                )
            }
            Side::Sell => {
                let shares = size;
                let usd_amount = price * size;
                (
                    decimal_to_share_units(shares),
                    decimal_to_usdc_units(usd_amount),
                )
            }
        };

        let nonce = generate_nonce();
        let expiration = generate_expiration(300); // 5 minutes

        OrderData {
            maker,
            taker,
            token_id: token_id.to_string(),
            maker_amount,
            taker_amount,
            side: side as u8,
            fee_rate_bps: fee_rate_bps.to_string(),
            nonce,
            expiration,
            signature_type: 0, // EOA signature
        }
    }

    /// Sign order (async for compatibility with ethers)
    pub async fn sign_order(&self, order: &OrderData) -> HftResult<SignedOrder> {
        // Create EIP-712 typed data hash
        let order_hash = self.compute_order_hash(order)?;

        // Sign the hash
        let signature = self
            .wallet
            .sign_hash(order_hash.into())
            .map_err(|e| HftError::SigningError(e.to_string()))?;

        Ok(SignedOrder {
            order: order.clone(),
            signature: format!("0x{}", hex::encode(signature.to_vec())),
        })
    }

    /// Compute EIP-712 order hash
    fn compute_order_hash(&self, order: &OrderData) -> HftResult<[u8; 32]> {
        // EIP-712 domain separator
        let domain_separator = self.compute_domain_separator()?;

        // Order type hash
        let type_hash = Keccak256::digest(
            b"Order(address maker,address taker,uint256 tokenId,uint256 makerAmount,uint256 taker\
              Amount,uint8 side,uint256 feeRateBps,uint256 nonce,uint256 expiration)",
        );

        // Encode order struct
        let maker = parse_address(&order.maker)?;
        let taker = parse_address(&order.taker)?;
        let token_id = parse_u256(&order.token_id)?;
        let maker_amount = parse_u256(&order.maker_amount)?;
        let taker_amount = parse_u256(&order.taker_amount)?;

        let struct_hash = Keccak256::digest(
            [
                type_hash.as_slice(),
                &encode_address(maker),
                &encode_address(taker),
                &encode_u256(token_id),
                &encode_u256(maker_amount),
                &encode_u256(taker_amount),
                &encode_u8(order.side),
                &encode_u256(parse_u256(&order.fee_rate_bps)?),
                &encode_u256(parse_u256(&order.nonce)?),
                &encode_u256(parse_u256(&order.expiration)?),
            ]
            .concat(),
        );

        // Final hash = keccak256("\x19\x01" || domain_separator || struct_hash)
        let final_hash = Keccak256::digest(
            [
                &[0x19, 0x01],
                domain_separator.as_slice(),
                struct_hash.as_slice(),
            ]
            .concat(),
        );

        let mut result = [0u8; 32];
        result.copy_from_slice(&final_hash);
        Ok(result)
    }

    /// Compute EIP-712 domain separator
    fn compute_domain_separator(&self) -> HftResult<[u8; 32]> {
        let type_hash = Keccak256::digest(
            b"EIP712Domain(string name,string version,uint256 chainId,address verifyingContract)",
        );

        let name_hash = Keccak256::digest(b"Polymarket CTF Exchange");
        let version_hash = Keccak256::digest(b"1");
        let chain_id = U256::from(self.chain_id);
        let contract = parse_address(CLOB_CONTRACT)?;

        let domain_hash = Keccak256::digest(
            [
                type_hash.as_slice(),
                name_hash.as_slice(),
                version_hash.as_slice(),
                &encode_u256(chain_id),
                &encode_address(contract),
            ]
            .concat(),
        );

        let mut result = [0u8; 32];
        result.copy_from_slice(&domain_hash);
        Ok(result)
    }
}

// Helper functions

fn generate_nonce() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    timestamp.to_string()
}

fn generate_expiration(seconds: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let expiry = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs()
        + seconds;
    expiry.to_string()
}

fn decimal_to_usdc_units(amount: Decimal) -> String {
    // USDC has 6 decimals
    let units = amount * Decimal::from(1_000_000);
    units.to_string().split('.').next().unwrap().to_string()
}

fn decimal_to_share_units(amount: Decimal) -> String {
    // Shares have 6 decimals
    let units = amount * Decimal::from(1_000_000);
    units.to_string().split('.').next().unwrap().to_string()
}

fn parse_address(s: &str) -> HftResult<Address> {
    Address::from_str(s).map_err(|e| HftError::SigningError(format!("Invalid address: {}", e)))
}

fn parse_u256(s: &str) -> HftResult<U256> {
    if s.starts_with("0x") {
        U256::from_str_radix(&s[2..], 16)
            .map_err(|e| HftError::SigningError(format!("Invalid U256: {}", e)))
    } else {
        U256::from_dec_str(s).map_err(|e| HftError::SigningError(format!("Invalid U256: {}", e)))
    }
}

fn encode_address(addr: Address) -> [u8; 32] {
    let mut result = [0u8; 32];
    result[12..32].copy_from_slice(addr.as_bytes());
    result
}

fn encode_u256(val: U256) -> [u8; 32] {
    let mut result = [0u8; 32];
    val.to_big_endian(&mut result);
    result
}

fn encode_u8(val: u8) -> [u8; 32] {
    let mut result = [0u8; 32];
    result[31] = val;
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_decimal_to_usdc() {
        assert_eq!(decimal_to_usdc_units(dec!(100)), "100000000");
        assert_eq!(decimal_to_usdc_units(dec!(0.5)), "500000");
        assert_eq!(decimal_to_usdc_units(dec!(1.234567)), "1234567");
    }

    #[test]
    fn test_generate_nonce() {
        let nonce1 = generate_nonce();
        let nonce2 = generate_nonce();
        assert_ne!(nonce1, nonce2);
    }
}
