//! Coinbase-based test binary for local development
//!
//! Uses Coinbase WebSocket (works in US) instead of Binance for testing.

use anyhow::Result;
use hft_binance::{CoinbaseClient, CoinbaseConfig, CryptoAsset};
use rust_decimal::Decimal;
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tracing::{info, warn};

/// Simple terminal dashboard
struct Dashboard {
    btc_price: Option<Decimal>,
    eth_price: Option<Decimal>,
    btc_change_1s: i32,
    eth_change_1s: i32,
    messages: u64,
    connected: bool,
}

impl Dashboard {
    fn new() -> Self {
        Self {
            btc_price: None,
            eth_price: None,
            btc_change_1s: 0,
            eth_change_1s: 0,
            messages: 0,
            connected: false,
        }
    }

    fn update(&mut self, coinbase: &CoinbaseClient) {
        self.connected = coinbase.is_connected();
        self.messages = coinbase.messages_received();

        if let Some(btc) = coinbase.latest_price(CryptoAsset::BTC) {
            self.btc_price = Some(btc);
        }
        if let Some(eth) = coinbase.latest_price(CryptoAsset::ETH) {
            self.eth_price = Some(eth);
        }

        if let Some(m) = coinbase.get_momentum(CryptoAsset::BTC) {
            self.btc_change_1s = m.change_1s_bps;
        }
        if let Some(m) = coinbase.get_momentum(CryptoAsset::ETH) {
            self.eth_change_1s = m.change_1s_bps;
        }
    }

    fn render(&self) {
        // Clear screen and move cursor home
        print!("\x1B[2J\x1B[H");

        println!("╔═══════════════════════════════════════════════════════════════╗");
        println!("║         CRYPTO LATENCY HFT - TEST MODE (COINBASE)            ║");
        println!("╠═══════════════════════════════════════════════════════════════╣");

        let status = if self.connected {
            "● CONNECTED"
        } else {
            "○ CONNECTING..."
        };
        println!("║  Status: {:54} ║", status);
        println!("║  Messages: {:52} ║", self.messages);
        println!("╠═══════════════════════════════════════════════════════════════╣");

        let btc_str = self
            .btc_price
            .map(|p| format!("${:.2}", p))
            .unwrap_or_else(|| "---".to_string());
        let eth_str = self
            .eth_price
            .map(|p| format!("${:.2}", p))
            .unwrap_or_else(|| "---".to_string());

        let btc_change = format_change(self.btc_change_1s);
        let eth_change = format_change(self.eth_change_1s);

        println!(
            "║  BTC/USD: {:14}  (1s: {:>8})                        ║",
            btc_str, btc_change
        );
        println!(
            "║  ETH/USD: {:14}  (1s: {:>8})                        ║",
            eth_str, eth_change
        );
        println!("╠═══════════════════════════════════════════════════════════════╣");
        println!("║  Press Ctrl+C to exit                                         ║");
        println!("╚═══════════════════════════════════════════════════════════════╝");

        io::stdout().flush().ok();
    }
}

fn format_change(bps: i32) -> String {
    if bps > 0 {
        format!("+{:.2}%", bps as f64 / 100.0)
    } else if bps < 0 {
        format!("{:.2}%", bps as f64 / 100.0)
    } else {
        "0.00%".to_string()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    // Initialize logging (minimal)
    tracing_subscriber::fmt()
        .with_env_filter("warn")
        .with_target(false)
        .init();

    info!("Starting Coinbase price feed test...");

    let config = CoinbaseConfig::default();
    let client = Arc::new(CoinbaseClient::new(config));

    // Start WebSocket connection
    let client_for_ws = client.clone();
    let running = Arc::new(AtomicBool::new(true));
    let running_for_ws = running.clone();

    let ws_handle = tokio::spawn(async move {
        while running_for_ws.load(Ordering::SeqCst) {
            if let Err(e) = client_for_ws.connect().await {
                warn!(error = %e, "Coinbase connection error, retrying...");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
        }
    });

    // Handle Ctrl+C
    let running_for_signal = running.clone();
    let client_for_signal = client.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        running_for_signal.store(false, Ordering::SeqCst);
        client_for_signal.stop();
    });

    // Dashboard update loop
    let mut dashboard = Dashboard::new();

    while running.load(Ordering::SeqCst) {
        dashboard.update(&client);
        dashboard.render();
        tokio::time::sleep(Duration::from_millis(250)).await;
    }

    ws_handle.abort();
    println!("\nShutdown complete.");

    Ok(())
}
