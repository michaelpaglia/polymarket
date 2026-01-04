//! Types for Binance price data

use rust_decimal::prelude::ToPrimitive;
use rust_decimal::Decimal;
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};

/// Crypto asset type
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CryptoAsset {
    BTC,
    ETH,
}

impl CryptoAsset {
    /// Get Binance symbol for this asset
    pub fn binance_symbol(&self) -> &'static str {
        match self {
            CryptoAsset::BTC => "btcusdt",
            CryptoAsset::ETH => "ethusdt",
        }
    }

    /// Get aggTrade stream name
    pub fn agg_trade_stream(&self) -> String {
        format!("{}@aggTrade", self.binance_symbol())
    }

    /// Parse from Binance symbol
    pub fn from_symbol(symbol: &str) -> Option<Self> {
        let lower = symbol.to_lowercase();
        if lower.contains("btc") {
            Some(CryptoAsset::BTC)
        } else if lower.contains("eth") {
            Some(CryptoAsset::ETH)
        } else {
            None
        }
    }
}

impl std::fmt::Display for CryptoAsset {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CryptoAsset::BTC => write!(f, "BTC"),
            CryptoAsset::ETH => write!(f, "ETH"),
        }
    }
}

/// Binance aggTrade message
#[derive(Debug, Clone, Deserialize)]
pub struct AggTradeMessage {
    /// Event type (always "aggTrade")
    #[serde(rename = "e")]
    pub event_type: String,
    /// Event time (ms since epoch)
    #[serde(rename = "E")]
    pub event_time: u64,
    /// Symbol (e.g., "BTCUSDT")
    #[serde(rename = "s")]
    pub symbol: String,
    /// Aggregate trade ID
    #[serde(rename = "a")]
    pub agg_trade_id: u64,
    /// Price
    #[serde(rename = "p")]
    pub price: String,
    /// Quantity
    #[serde(rename = "q")]
    pub quantity: String,
    /// First trade ID
    #[serde(rename = "f")]
    pub first_trade_id: u64,
    /// Last trade ID
    #[serde(rename = "l")]
    pub last_trade_id: u64,
    /// Trade time (ms since epoch)
    #[serde(rename = "T")]
    pub trade_time: u64,
    /// Is buyer maker
    #[serde(rename = "m")]
    pub is_buyer_maker: bool,
}

/// Binance price tick (processed from aggTrade)
#[derive(Debug, Clone)]
pub struct BinanceTick {
    /// Asset type
    pub asset: CryptoAsset,
    /// Trade price
    pub price: Decimal,
    /// Trade quantity
    pub quantity: Decimal,
    /// Binance trade timestamp (ms)
    pub trade_time_ms: u64,
    /// Local receive timestamp (ns)
    pub received_ns: u64,
    /// Is buyer maker (true = sell aggressor, false = buy aggressor)
    pub is_sell: bool,
}

impl BinanceTick {
    /// Parse from aggTrade message
    pub fn from_agg_trade(msg: &AggTradeMessage, received_ns: u64) -> Option<Self> {
        let asset = CryptoAsset::from_symbol(&msg.symbol)?;
        let price = msg.price.parse::<Decimal>().ok()?;
        let quantity = msg.quantity.parse::<Decimal>().ok()?;

        Some(Self {
            asset,
            price,
            quantity,
            trade_time_ms: msg.trade_time,
            received_ns,
            is_sell: msg.is_buyer_maker,
        })
    }

    /// Get age in milliseconds
    pub fn age_ms(&self) -> u64 {
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        (now_ns - self.received_ns) / 1_000_000
    }
}

/// Rolling price window for momentum calculation
#[derive(Debug)]
pub struct PriceWindow {
    /// Asset being tracked
    pub asset: CryptoAsset,
    /// Rolling window of ticks (newest first)
    ticks: parking_lot::RwLock<VecDeque<BinanceTick>>,
    /// Maximum ticks to retain
    max_ticks: usize,
    /// Maximum age in milliseconds
    max_age_ms: u64,
    /// Latest price (atomic for fast reads)
    latest_price_scaled: AtomicU64,
    /// Last update timestamp (ns)
    last_update_ns: AtomicU64,
}

const PRICE_SCALE: u64 = 1_000_000_000; // 9 decimal places for crypto

impl PriceWindow {
    /// Create new price window
    pub fn new(asset: CryptoAsset, max_ticks: usize, max_age_ms: u64) -> Self {
        Self {
            asset,
            ticks: parking_lot::RwLock::new(VecDeque::with_capacity(max_ticks)),
            max_ticks,
            max_age_ms,
            latest_price_scaled: AtomicU64::new(0),
            last_update_ns: AtomicU64::new(0),
        }
    }

    /// Add a new tick
    pub fn add_tick(&self, tick: BinanceTick) {
        // Update atomic price
        if let Some(scaled) = self.decimal_to_scaled(tick.price) {
            self.latest_price_scaled.store(scaled, Ordering::Release);
            self.last_update_ns
                .store(tick.received_ns, Ordering::Release);
        }

        // Add to rolling window
        let mut ticks = self.ticks.write();
        ticks.push_front(tick);

        // Trim by count
        while ticks.len() > self.max_ticks {
            ticks.pop_back();
        }

        // Trim by age
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let cutoff_ns = now_ns - (self.max_age_ms * 1_000_000);

        while let Some(back) = ticks.back() {
            if back.received_ns < cutoff_ns {
                ticks.pop_back();
            } else {
                break;
            }
        }
    }

    /// Get latest price (fast, lock-free)
    pub fn latest_price(&self) -> Option<Decimal> {
        let scaled = self.latest_price_scaled.load(Ordering::Acquire);
        if scaled == 0 {
            None
        } else {
            Some(self.scaled_to_decimal(scaled))
        }
    }

    /// Get last update timestamp
    pub fn last_update_ns(&self) -> u64 {
        self.last_update_ns.load(Ordering::Acquire)
    }

    /// Get price N milliseconds ago
    pub fn price_at_age_ms(&self, age_ms: u64) -> Option<Decimal> {
        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;
        let target_ns = now_ns - (age_ms * 1_000_000);

        let ticks = self.ticks.read();

        // Find closest tick to target time
        for tick in ticks.iter() {
            if tick.received_ns <= target_ns {
                return Some(tick.price);
            }
        }

        // Return oldest if all are newer
        ticks.back().map(|t| t.price)
    }

    /// Get high/low in window
    pub fn high_low(&self) -> Option<(Decimal, Decimal)> {
        let ticks = self.ticks.read();
        if ticks.is_empty() {
            return None;
        }

        let mut high = ticks[0].price;
        let mut low = ticks[0].price;

        for tick in ticks.iter() {
            if tick.price > high {
                high = tick.price;
            }
            if tick.price < low {
                low = tick.price;
            }
        }

        Some((high, low))
    }

    /// Calculate momentum metrics
    pub fn calculate_momentum(&self) -> Option<PriceMomentum> {
        let current = self.latest_price()?;
        let price_1s = self.price_at_age_ms(1000);
        let price_5s = self.price_at_age_ms(5000);
        let price_1min = self.price_at_age_ms(60000);

        let change_1s_bps = price_1s.map(|p| self.calculate_change_bps(current, p));
        let change_5s_bps = price_5s.map(|p| self.calculate_change_bps(current, p));
        let change_1min_bps = price_1min.map(|p| self.calculate_change_bps(current, p));

        let (high, low) = self.high_low().unwrap_or((current, current));
        let volatility = if low > Decimal::ZERO {
            ((high - low) / low).to_f64().unwrap_or(0.0)
        } else {
            0.0
        };

        // Determine direction based on short-term momentum
        let direction = change_1s_bps.and_then(|change| {
            if change >= 10 {
                Some(Direction::Up)
            } else if change <= -10 {
                Some(Direction::Down)
            } else {
                None
            }
        });

        // Calculate strength (0.0 to 1.0)
        let strength = change_1s_bps
            .map(|c| (c.abs() as f64 / 50.0).min(1.0))
            .unwrap_or(0.0);

        Some(PriceMomentum {
            asset: self.asset,
            current_price: current,
            change_1s_bps: change_1s_bps.unwrap_or(0),
            change_5s_bps: change_5s_bps.unwrap_or(0),
            change_1min_bps: change_1min_bps.unwrap_or(0),
            volatility_1min: volatility,
            direction,
            strength,
            last_update_ns: self.last_update_ns(),
        })
    }

    fn calculate_change_bps(&self, current: Decimal, previous: Decimal) -> i32 {
        if previous.is_zero() {
            return 0;
        }
        let change = (current - previous) / previous * Decimal::from(10000);
        change.round().to_i32().unwrap_or(0)
    }

    fn decimal_to_scaled(&self, price: Decimal) -> Option<u64> {
        let scaled = price * Decimal::from(PRICE_SCALE);
        scaled.to_string().parse::<u64>().ok()
    }

    fn scaled_to_decimal(&self, scaled: u64) -> Decimal {
        Decimal::from(scaled) / Decimal::from(PRICE_SCALE)
    }
}

/// Direction of price movement
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Up,
    Down,
}

impl std::fmt::Display for Direction {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Direction::Up => write!(f, "UP"),
            Direction::Down => write!(f, "DOWN"),
        }
    }
}

/// Price momentum metrics
#[derive(Debug, Clone, Serialize)]
pub struct PriceMomentum {
    /// Asset type
    pub asset: CryptoAsset,
    /// Current price
    pub current_price: Decimal,
    /// Change over 1 second in basis points
    pub change_1s_bps: i32,
    /// Change over 5 seconds in basis points
    pub change_5s_bps: i32,
    /// Change over 1 minute in basis points
    pub change_1min_bps: i32,
    /// Volatility over 1 minute (high-low / low)
    pub volatility_1min: f64,
    /// Detected direction (if momentum is strong enough)
    pub direction: Option<Direction>,
    /// Momentum strength (0.0 to 1.0)
    pub strength: f64,
    /// Last update timestamp (ns)
    pub last_update_ns: u64,
}

/// Binance WebSocket configuration
#[derive(Debug, Clone)]
pub struct BinanceConfig {
    /// WebSocket base URL
    pub ws_url: String,
    /// Symbols to subscribe (e.g., ["btcusdt@aggTrade", "ethusdt@aggTrade"])
    pub streams: Vec<String>,
    /// Reconnect delay in milliseconds
    pub reconnect_delay_ms: u64,
    /// Ping interval in milliseconds
    pub ping_interval_ms: u64,
    /// Price window size (number of ticks)
    pub window_size: usize,
    /// Price window max age (ms)
    pub window_max_age_ms: u64,
}

impl Default for BinanceConfig {
    fn default() -> Self {
        Self {
            // Use port 443 instead of 9443 for proxy compatibility
            ws_url: "wss://stream.binance.com:443/ws".to_string(),
            streams: vec![
                "btcusdt@aggTrade".to_string(),
                "ethusdt@aggTrade".to_string(),
            ],
            reconnect_delay_ms: 1000,
            ping_interval_ms: 30000,
            window_size: 1000,
            window_max_age_ms: 60000, // 1 minute
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn test_crypto_asset_symbol() {
        assert_eq!(CryptoAsset::BTC.binance_symbol(), "btcusdt");
        assert_eq!(CryptoAsset::ETH.binance_symbol(), "ethusdt");
    }

    #[test]
    fn test_crypto_asset_from_symbol() {
        assert_eq!(CryptoAsset::from_symbol("BTCUSDT"), Some(CryptoAsset::BTC));
        assert_eq!(CryptoAsset::from_symbol("ethusdt"), Some(CryptoAsset::ETH));
        assert_eq!(CryptoAsset::from_symbol("XRPUSDT"), None);
    }

    #[test]
    fn test_price_window() {
        let window = PriceWindow::new(CryptoAsset::BTC, 100, 60000);

        let now_ns = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos() as u64;

        // Add some ticks
        window.add_tick(BinanceTick {
            asset: CryptoAsset::BTC,
            price: dec!(50000),
            quantity: dec!(1.5),
            trade_time_ms: now_ns / 1_000_000,
            received_ns: now_ns,
            is_sell: false,
        });

        assert_eq!(window.latest_price(), Some(dec!(50000)));
    }
}
