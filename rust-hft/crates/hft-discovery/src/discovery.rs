//! Market discovery implementation
//!
//! Discovers 15-minute crypto up/down markets from Polymarket.
//! Uses the /events endpoint with tag_slug=15M to find active markets.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use hft_core::{CryptoAsset, CryptoMarket, MarketId, TokenId};
use parking_lot::Mutex;
use rust_decimal::Decimal;
use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;
use tracing::{debug, error, info, warn};

/// Discovery configuration
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    /// Gamma API URL
    pub gamma_url: String,
    /// Refresh interval (seconds)
    pub refresh_interval_secs: u64,
    /// Minimum liquidity (USD)
    pub min_liquidity_usd: f64,
    /// Minimum time to start (seconds) - must have time before market starts
    pub min_time_to_start_secs: i64,
    /// Maximum time to start (seconds) - don't trade markets starting too far out
    pub max_time_to_start_secs: i64,
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            gamma_url: "https://gamma-api.polymarket.com".to_string(),
            refresh_interval_secs: 30,
            min_liquidity_usd: 100.0,   // Lower threshold for 15M markets
            min_time_to_start_secs: 60, // At least 1 minute before start
            max_time_to_start_secs: 20 * 60, // 20 minutes max until start
        }
    }
}

/// Price snapshot for momentum tracking
#[derive(Debug, Clone)]
struct PriceSnapshot {
    up_price: f64,
    down_price: f64,
    timestamp: DateTime<Utc>,
}

/// Price momentum tracker for a single market
struct PriceMomentum {
    /// Recent price snapshots (newest at back)
    history: VecDeque<PriceSnapshot>,
    /// Maximum history size
    max_size: usize,
}

impl PriceMomentum {
    fn new() -> Self {
        Self {
            history: VecDeque::with_capacity(20),
            max_size: 20, // Keep ~10 minutes of data at 30s refresh
        }
    }

    fn record(&mut self, up_price: f64, down_price: f64) {
        self.history.push_back(PriceSnapshot {
            up_price,
            down_price,
            timestamp: Utc::now(),
        });

        while self.history.len() > self.max_size {
            self.history.pop_front();
        }
    }

    /// Calculate price momentum (direction and strength)
    /// Returns (direction, strength_bps)
    /// direction: true = price moving up (favoring UP outcome), false = moving down
    fn momentum(&self) -> Option<(bool, i32)> {
        if self.history.len() < 2 {
            return None;
        }

        let oldest = self.history.front()?;
        let newest = self.history.back()?;

        // Check UP price change (negative change = UP is getting cheaper = more bullish)
        let up_change = newest.up_price - oldest.up_price;
        let down_change = newest.down_price - oldest.down_price;

        // If UP price is decreasing (cheaper to buy UP), that's bullish momentum
        // If DOWN price is decreasing (cheaper to buy DOWN), that's bearish momentum
        let direction_up = up_change < down_change;

        // Strength is the magnitude of the stronger signal
        let strength_bps = ((up_change.abs().max(down_change.abs())) * 10000.0) as i32;

        if strength_bps > 0 {
            Some((direction_up, strength_bps))
        } else {
            None
        }
    }

    /// Get recent price velocity (change per minute)
    fn velocity_per_min(&self) -> Option<f64> {
        if self.history.len() < 2 {
            return None;
        }

        let oldest = self.history.front()?;
        let newest = self.history.back()?;

        let time_diff = (newest.timestamp - oldest.timestamp).num_seconds() as f64;
        if time_diff < 10.0 {
            return None;
        }

        let up_change = newest.up_price - oldest.up_price;
        Some(up_change * 60.0 / time_diff) // Change per minute
    }
}

/// Market discovery service
pub struct CryptoMarketDiscovery {
    config: DiscoveryConfig,
    client: reqwest::Client,
    /// Active markets by market ID
    markets: DashMap<String, Arc<CryptoMarket>>,
    /// Cached outcome prices (up_price, down_price) by market ID
    prices: DashMap<String, (f64, f64)>,
    /// Price momentum trackers by market ID
    momentum: Mutex<std::collections::HashMap<String, PriceMomentum>>,
}

impl CryptoMarketDiscovery {
    /// Create new discovery service
    pub fn new(config: DiscoveryConfig) -> Self {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .expect("Failed to create HTTP client");

        Self {
            config,
            client,
            markets: DashMap::new(),
            prices: DashMap::new(),
            momentum: Mutex::new(std::collections::HashMap::new()),
        }
    }

    /// Get cached up price for a market
    pub fn get_market_up_price(&self, market_id: &str) -> f64 {
        self.prices.get(market_id).map(|p| p.0).unwrap_or(0.5)
    }

    /// Get cached down price for a market
    pub fn get_market_down_price(&self, market_id: &str) -> f64 {
        self.prices.get(market_id).map(|p| p.1).unwrap_or(0.5)
    }

    /// Get price momentum for a market
    /// Returns (direction, strength_bps) where direction true = UP favored
    pub fn get_market_momentum(&self, market_id: &str) -> Option<(bool, i32)> {
        let momentum = self.momentum.lock();
        momentum.get(market_id)?.momentum()
    }

    /// Get price velocity (change per minute) for a market
    pub fn get_market_velocity(&self, market_id: &str) -> Option<f64> {
        let momentum = self.momentum.lock();
        momentum.get(market_id)?.velocity_per_min()
    }

    /// Record a price update for momentum tracking
    fn record_price(&self, market_id: &str, up_price: f64, down_price: f64) {
        // Update price cache
        self.prices.insert(market_id.to_string(), (up_price, down_price));

        // Update momentum tracker
        let mut momentum = self.momentum.lock();
        momentum
            .entry(market_id.to_string())
            .or_insert_with(PriceMomentum::new)
            .record(up_price, down_price);
    }

    /// Get all active markets
    pub fn get_active_markets(&self) -> Vec<Arc<CryptoMarket>> {
        self.markets
            .iter()
            .filter(|m| !m.value().is_expired())
            .map(|m| m.value().clone())
            .collect()
    }

    /// Get markets for a specific asset
    pub fn get_markets_for_asset(&self, asset: CryptoAsset) -> Vec<Arc<CryptoMarket>> {
        let all: Vec<_> = self.markets.iter().map(|m| m.value().clone()).collect();
        debug!(
            asset = ?asset,
            total_markets = all.len(),
            "get_markets_for_asset called"
        );
        self.markets
            .iter()
            .filter(|m| m.value().asset == asset && !m.value().is_expired())
            .map(|m| m.value().clone())
            .collect()
    }

    /// Get market by ID
    pub fn get_market(&self, market_id: &str) -> Option<Arc<CryptoMarket>> {
        self.markets.get(market_id).map(|m| m.value().clone())
    }

    /// Number of active markets
    pub fn active_count(&self) -> usize {
        self.markets
            .iter()
            .filter(|m| !m.value().is_expired())
            .count()
    }

    /// Discover 15M crypto markets using the events API with tag_slug=15M
    pub async fn discover(&self) -> Result<Vec<CryptoMarket>, DiscoveryError> {
        // Use the events endpoint with 15M tag to find crypto up/down markets
        let url = format!(
            "{}/events?tag_slug=15M&closed=false&active=true&limit=50",
            self.config.gamma_url
        );

        debug!(url = %url, "Fetching 15M crypto events");

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| DiscoveryError::Http(e.to_string()))?;

        if !response.status().is_success() {
            return Err(DiscoveryError::Http(format!(
                "HTTP {}: {}",
                response.status(),
                response.status().canonical_reason().unwrap_or("")
            )));
        }

        let events: Vec<serde_json::Value> = response
            .json()
            .await
            .map_err(|e| DiscoveryError::Parse(e.to_string()))?;

        debug!(events_count = events.len(), "Received 15M events");

        let mut all_markets = Vec::new();
        let now = Utc::now();

        for event in events {
            // Parse the event to extract crypto markets
            if let Some(markets) = self.parse_event(&event, now) {
                all_markets.extend(markets);
            }
        }

        // Update cache
        for market in &all_markets {
            self.markets
                .insert(market.market_id.0.clone(), Arc::new(market.clone()));
        }

        // Clean expired markets
        self.markets.retain(|_, m| !m.is_expired());

        info!(
            discovered = all_markets.len(),
            active = self.markets.len(),
            "15M market discovery complete"
        );

        Ok(all_markets)
    }

    /// Run discovery loop
    pub async fn run_discovery_loop(self: Arc<Self>) {
        let interval = Duration::from_secs(self.config.refresh_interval_secs);

        loop {
            match self.discover().await {
                Ok(markets) => {
                    debug!(count = markets.len(), "Discovery cycle complete");
                }
                Err(e) => {
                    error!(error = %e, "Discovery failed");
                }
            }

            tokio::time::sleep(interval).await;
        }
    }

    /// Parse an event JSON to extract CryptoMarkets
    /// Events from /events?tag_slug=15M contain nested markets[] array
    fn parse_event(
        &self,
        event: &serde_json::Value,
        now: DateTime<Utc>,
    ) -> Option<Vec<CryptoMarket>> {
        // Get the series slug to determine asset type
        let series = event.get("series")?.as_array()?;
        if series.is_empty() {
            return None;
        }

        let series_slug = series[0].get("slug")?.as_str()?;

        // Determine asset from series slug (e.g., "btc-up-or-down-15m", "eth-up-or-down-15m")
        let asset = if series_slug.starts_with("btc") {
            CryptoAsset::BTC
        } else if series_slug.starts_with("eth") {
            CryptoAsset::ETH
        } else {
            // Skip non-BTC/ETH assets for now (XRP, SOL, etc.)
            debug!(series = %series_slug, "Skipping non-BTC/ETH asset");
            return None;
        };

        // Get the nested markets array
        let markets_arr = event.get("markets")?.as_array()?;

        let mut result = Vec::new();

        for market in markets_arr {
            if let Some(crypto_market) = self.parse_market_from_event(market, asset, now) {
                result.push(crypto_market);
            }
        }

        if result.is_empty() {
            None
        } else {
            Some(result)
        }
    }

    /// Extract strike price from question text
    /// Format: "Will the price of Bitcoin be above $97,193.81 at 9:00 PM UTC?"
    fn extract_strike_price(question: &str) -> Option<Decimal> {
        // Look for dollar sign followed by number with optional commas and decimals
        // Pattern: $[digits,]digits[.digits]
        let dollar_pos = question.find('$')?;
        let after_dollar = &question[dollar_pos + 1..];

        // Extract the number portion (digits, commas, and decimal point)
        let num_str: String = after_dollar
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
            .filter(|c| *c != ',') // Remove commas
            .collect();

        num_str.parse::<Decimal>().ok()
    }

    /// Parse a single market from an event's markets[] array
    fn parse_market_from_event(
        &self,
        market: &serde_json::Value,
        asset: CryptoAsset,
        now: DateTime<Utc>,
    ) -> Option<CryptoMarket> {
        let slug = market.get("slug")?.as_str()?;
        let question = market.get("question")?.as_str()?;

        // Extract strike price from question
        let strike_price = Self::extract_strike_price(question);
        if strike_price.is_some() {
            debug!(
                slug = %slug,
                strike = ?strike_price,
                "Extracted strike price from question"
            );
        }

        // Check if closed
        let closed = market
            .get("closed")
            .and_then(|v| v.as_bool())
            .unwrap_or(true);
        if closed {
            return None;
        }

        // Check if accepting orders
        let accepting_orders = market
            .get("acceptingOrders")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        if !accepting_orders {
            debug!(slug = %slug, "Market not accepting orders");
            return None;
        }

        // Parse token IDs from clobTokenIds JSON string
        let token_ids_str = market.get("clobTokenIds")?.as_str()?;
        let token_ids: Vec<String> = serde_json::from_str(token_ids_str).ok()?;

        if token_ids.len() != 2 {
            return None;
        }

        // Parse event start time (when the 15-min window begins)
        // Skip markets without eventStartTime as they're not valid 15M markets
        let start_time_str = match market.get("eventStartTime").and_then(|v| v.as_str()) {
            Some(s) => s,
            None => {
                debug!(slug = %slug, "Skipping market without eventStartTime");
                return None;
            }
        };
        let start_time = DateTime::parse_from_rfc3339(start_time_str)
            .ok()?
            .with_timezone(&Utc);

        // Parse end time (when it resolves)
        let end_date_str = market.get("endDate")?.as_str()?;
        let end_time = DateTime::parse_from_rfc3339(end_date_str)
            .ok()?
            .with_timezone(&Utc);

        // Check time constraints
        let secs_to_start = (start_time - now).num_seconds();
        let secs_to_end = (end_time - now).num_seconds();

        // Skip if already ended
        if secs_to_end <= 0 {
            debug!(slug = %slug, "Market already ended");
            return None;
        }

        // Accept markets that are:
        // 1. Currently active (started but not ended) - secs_to_start < 0 && secs_to_end > 0
        // 2. Starting soon (within max_time_to_start_secs)
        let is_active = secs_to_start <= 0 && secs_to_end > 0;
        let is_upcoming = secs_to_start > 0 && secs_to_start <= self.config.max_time_to_start_secs;

        if !is_active && !is_upcoming {
            debug!(slug = %slug, secs_to_start = secs_to_start, "Market too far in future");
            return None;
        }

        // Check liquidity
        let liquidity = market
            .get("liquidityNum")
            .or_else(|| market.get("liquidity"))
            .and_then(|v| {
                v.as_f64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .unwrap_or(0.0);

        if liquidity < self.config.min_liquidity_usd {
            debug!(slug = %slug, liquidity = liquidity, "Insufficient liquidity");
            return None;
        }

        // Parse outcomes to determine Up/Down token mapping
        // Outcomes are typically ["Up", "Down"]
        let outcomes_str = market.get("outcomes")?.as_str()?;
        let outcomes: Vec<String> = serde_json::from_str(outcomes_str).ok()?;

        if outcomes.len() != 2 {
            return None;
        }

        // First outcome is "Up", second is "Down"
        let (up_token_id, down_token_id) = if outcomes[0].to_lowercase() == "up" {
            (token_ids[0].clone(), token_ids[1].clone())
        } else {
            (token_ids[1].clone(), token_ids[0].clone())
        };

        // Get current outcome prices (market's view of Up/Down probability)
        let outcome_prices_str = market.get("outcomePrices").and_then(|v| v.as_str());
        let outcome_prices: Option<(f64, f64)> = outcome_prices_str.and_then(|s| {
            let prices: Vec<String> = serde_json::from_str(s).ok()?;
            if prices.len() == 2 {
                let up = prices[0].parse::<f64>().ok()?;
                let down = prices[1].parse::<f64>().ok()?;
                Some((up, down))
            } else {
                None
            }
        });

        let status = if is_active { "ACTIVE" } else { "upcoming" };

        // Store prices in cache and record for momentum tracking
        if let Some((up, down)) = outcome_prices {
            self.record_price(slug, up, down);
        }

        info!(
            slug = %slug,
            asset = ?asset,
            status = status,
            strike = ?strike_price,
            secs_to_start = secs_to_start,
            secs_to_end = secs_to_end,
            liquidity = liquidity,
            up_price = outcome_prices.map(|p| p.0),
            down_price = outcome_prices.map(|p| p.1),
            "Discovered 15M market"
        );

        Some(CryptoMarket {
            market_id: MarketId::new(slug),
            asset,
            up_token_id: TokenId::new(up_token_id),
            down_token_id: TokenId::new(down_token_id),
            strike_price, // Extracted from question (e.g., "$97,193.81")
            start_time,   // When window starts
            end_time,
            discovered_at: now,
            question: question.to_string(),
        })
    }
}

/// Discovery errors
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("HTTP error: {0}")]
    Http(String),
    #[error("Parse error: {0}")]
    Parse(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_series_slug_parsing() {
        // BTC series slugs
        assert!("btc-up-or-down-15m".starts_with("btc"));
        assert!("btc-updown-15m-1234567890".starts_with("btc"));

        // ETH series slugs
        assert!("eth-up-or-down-15m".starts_with("eth"));
        assert!("eth-updown-15m-1234567890".starts_with("eth"));

        // Other assets (should be skipped)
        assert!(!"sol-up-or-down-15m".starts_with("btc"));
        assert!(!"sol-up-or-down-15m".starts_with("eth"));
    }

    #[test]
    fn test_outcome_token_mapping() {
        let outcomes = vec!["Up".to_string(), "Down".to_string()];
        let token_ids = vec!["token-up".to_string(), "token-down".to_string()];

        // First outcome is "Up", so first token is Up token
        let first_lower = outcomes[0].to_lowercase();
        let (up_token, down_token) = if first_lower == "up" {
            (token_ids[0].clone(), token_ids[1].clone())
        } else {
            (token_ids[1].clone(), token_ids[0].clone())
        };

        assert_eq!(up_token, "token-up");
        assert_eq!(down_token, "token-down");
    }

    #[test]
    fn test_config_defaults() {
        let config = DiscoveryConfig::default();
        assert_eq!(config.min_time_to_start_secs, 60);
        assert_eq!(config.max_time_to_start_secs, 20 * 60);
        assert_eq!(config.min_liquidity_usd, 100.0);
    }
}
