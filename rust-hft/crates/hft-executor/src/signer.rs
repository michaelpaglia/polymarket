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

/// Order data for signing (camelCase to match Python py_clob_client)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderData {
    pub salt: u64,  // Integer in JSON, not string!
    pub maker: String,
    pub signer: String,
    pub taker: String,
    pub token_id: String,
    pub maker_amount: String,
    pub taker_amount: String,
    pub expiration: String,
    pub nonce: String,
    pub fee_rate_bps: String,
    pub side: String,  // "BUY" or "SELL" string, not int
    pub signature_type: u8,
}

/// Order with signature embedded (matches Python SignedOrder.dict() format)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OrderWithSignature {
    pub salt: u64,  // Integer in JSON, not string!
    pub maker: String,
    pub signer: String,
    pub taker: String,
    pub token_id: String,
    pub maker_amount: String,
    pub taker_amount: String,
    pub expiration: String,
    pub nonce: String,
    pub fee_rate_bps: String,
    pub side: String,
    pub signature_type: u8,
    pub signature: String,
}

/// Signed order ready for submission (matches Python's order_to_json format)
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SignedOrder {
    pub order: OrderWithSignature,
    pub owner: String,
    pub order_type: String,
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
        let signer = self.address_hex();
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

        let salt = generate_salt();
        let nonce = generate_nonce();
        let expiration = generate_expiration(300); // 5 minutes

        // Side as string ("BUY" or "SELL")
        let side_str = match side {
            Side::Buy => "BUY".to_string(),
            Side::Sell => "SELL".to_string(),
        };

        OrderData {
            salt,
            maker,
            signer,
            taker,
            token_id: token_id.to_string(),
            maker_amount,
            taker_amount,
            expiration,
            nonce,
            fee_rate_bps: fee_rate_bps.to_string(),
            side: side_str,
            signature_type: 0, // EOA signature
        }
    }

    /// Sign order (async for compatibility with ethers)
    /// api_key is the owner field required by the CLOB API
    pub async fn sign_order(&self, order: &OrderData, api_key: &str) -> HftResult<SignedOrder> {
        // Create EIP-712 typed data hash
        let order_hash = self.compute_order_hash(order)?;

        // Sign the hash
        let signature = self
            .wallet
            .sign_hash(order_hash.into())
            .map_err(|e| HftError::SigningError(e.to_string()))?;

        let sig_hex = format!("0x{}", hex::encode(signature.to_vec()));

        // Create OrderWithSignature (signature embedded in order)
        let order_with_sig = OrderWithSignature {
            salt: order.salt.clone(),
            maker: order.maker.clone(),
            signer: order.signer.clone(),
            taker: order.taker.clone(),
            token_id: order.token_id.clone(),
            maker_amount: order.maker_amount.clone(),
            taker_amount: order.taker_amount.clone(),
            expiration: order.expiration.clone(),
            nonce: order.nonce.clone(),
            fee_rate_bps: order.fee_rate_bps.clone(),
            side: order.side.clone(),
            signature_type: order.signature_type,
            signature: sig_hex,
        };

        Ok(SignedOrder {
            order: order_with_sig,
            owner: api_key.to_string(),
            order_type: "GTC".to_string(),
        })
    }

    /// Compute EIP-712 order hash
    /// Matches py_order_utils Order struct: salt, maker, signer, taker, tokenId, makerAmount,
    /// takerAmount, expiration, nonce, feeRateBps, side, signatureType
    fn compute_order_hash(&self, order: &OrderData) -> HftResult<[u8; 32]> {
        // EIP-712 domain separator
        let domain_separator = self.compute_domain_separator()?;

        // Order type hash - must match py_order_utils Order struct exactly
        let type_hash = Keccak256::digest(
            b"Order(uint256 salt,address maker,address signer,address taker,uint256 tokenId,uint256 makerAmount,uint256 takerAmount,uint256 expiration,uint256 nonce,uint256 feeRateBps,uint8 side,uint8 signatureType)",
        );

        // Encode order struct fields in exact order
        let salt = U256::from(order.salt);
        let maker = parse_address(&order.maker)?;
        let signer = parse_address(&order.signer)?;
        let taker = parse_address(&order.taker)?;
        let token_id = parse_u256(&order.token_id)?;
        let maker_amount = parse_u256(&order.maker_amount)?;
        let taker_amount = parse_u256(&order.taker_amount)?;
        let expiration = parse_u256(&order.expiration)?;
        let nonce = parse_u256(&order.nonce)?;
        let fee_rate_bps = parse_u256(&order.fee_rate_bps)?;

        let struct_hash = Keccak256::digest(
            [
                type_hash.as_slice(),
                &encode_u256(salt),
                &encode_address(maker),
                &encode_address(signer),
                &encode_address(taker),
                &encode_u256(token_id),
                &encode_u256(maker_amount),
                &encode_u256(taker_amount),
                &encode_u256(expiration),
                &encode_u256(nonce),
                &encode_u256(fee_rate_bps),
                &encode_side(&order.side),
                &encode_u8(order.signature_type),
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

fn generate_salt() -> u64 {
    use rand::Rng;
    // Python uses random.randint(0, 2^32-1) for full entropy
    rand::thread_rng().gen_range(0..=0xFFFF_FFFF)
}

fn generate_nonce() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before UNIX epoch")
        .as_nanos();
    timestamp.to_string()
}

fn generate_expiration(seconds: u64) -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let expiry = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system time before UNIX epoch")
        .as_secs()
        + seconds;
    expiry.to_string()
}

fn decimal_to_usdc_units(amount: Decimal) -> String {
    // USDC has 6 decimals
    let units = amount * Decimal::from(1_000_000);
    // split() always yields at least one element
    units
        .to_string()
        .split('.')
        .next()
        .expect("split always has first element")
        .to_string()
}

fn decimal_to_share_units(amount: Decimal) -> String {
    // Shares have 6 decimals
    let units = amount * Decimal::from(1_000_000);
    // split() always yields at least one element
    units
        .to_string()
        .split('.')
        .next()
        .expect("split always has first element")
        .to_string()
}

fn parse_address(s: &str) -> HftResult<Address> {
    Address::from_str(s).map_err(|e| HftError::SigningError(format!("Invalid address: {}", e)))
}

fn parse_u256(s: &str) -> HftResult<U256> {
    if let Some(stripped) = s.strip_prefix("0x") {
        U256::from_str_radix(stripped, 16)
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

fn encode_side(side: &str) -> [u8; 32] {
    // BUY = 0, SELL = 1 for EIP-712 encoding
    let val = if side == "BUY" { 0u8 } else { 1u8 };
    encode_u8(val)
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
