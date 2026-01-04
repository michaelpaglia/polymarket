//! Market discovery implementation
//!
//! Discovers 15-minute crypto up/down markets from Polymarket.
//! Uses the /events endpoint with tag_slug=15M to find active markets.

use chrono::{DateTime, Utc};
use dashmap::DashMap;
use hft_core::{CryptoAsset, CryptoMarket, MarketId, TokenId};
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

/// Market discovery service
pub struct CryptoMarketDiscovery {
    config: DiscoveryConfig,
    client: reqwest::Client,
    /// Active markets by market ID
    markets: DashMap<String, Arc<CryptoMarket>>,
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
        }
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

    /// Parse a single market from an event's markets[] array
    fn parse_market_from_event(
        &self,
        market: &serde_json::Value,
        asset: CryptoAsset,
        now: DateTime<Utc>,
    ) -> Option<CryptoMarket> {
        let slug = market.get("slug")?.as_str()?;
        let question = market.get("question")?.as_str()?;

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

        // Check time constraints - we want markets that haven't started yet
        let secs_to_start = (start_time - now).num_seconds();

        // Skip if already started or about to start
        if secs_to_start < self.config.min_time_to_start_secs {
            debug!(slug = %slug, secs_to_start = secs_to_start, "Market starting too soon");
            return None;
        }

        // Skip if too far in the future
        if secs_to_start > self.config.max_time_to_start_secs {
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

        info!(
            slug = %slug,
            asset = ?asset,
            start_time = %start_time,
            end_time = %end_time,
            liquidity = liquidity,
            "Discovered 15M market"
        );

        Some(CryptoMarket {
            market_id: MarketId::new(slug),
            asset,
            up_token_id: TokenId::new(up_token_id),
            down_token_id: TokenId::new(down_token_id),
            strike_price: None,
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
