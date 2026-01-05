//! Test binary for auto-redemption
//!
//! Tests the redemption flow with real wallet credentials.

use anyhow::Result;
use hft_executor::PositionRedeemer;
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
    // Load environment variables
    if dotenvy::dotenv().is_err() {
        let _ = dotenvy::from_filename("../../.env");
    }

    // Initialize logging
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive("hft=info".parse().unwrap())
                .add_directive("redeem_test=info".parse().unwrap()),
        )
        .with_target(false)
        .init();

    info!("╔═══════════════════════════════════════════════════════════════╗");
    info!("║           AUTO-REDEMPTION TEST                                ║");
    info!("╚═══════════════════════════════════════════════════════════════╝");

    // Get private key from environment
    let private_key = std::env::var("POLYMARKET_PRIVATE_KEY")
        .or_else(|_| std::env::var("POLYGON_PRIVATE_KEY"))
        .or_else(|_| std::env::var("PRIVATE_KEY"))
        .expect("POLYMARKET_PRIVATE_KEY env var required");

    // Optional custom RPC
    let rpc_url = std::env::var("POLYGON_RPC_URL").ok();

    // Create redeemer
    let redeemer = PositionRedeemer::new(&private_key, rpc_url.as_deref())?;

    info!("Wallet: {}", redeemer.wallet_address_string());
    info!("");

    // Check for redeemable positions
    info!("Checking for redeemable positions...");

    match redeemer.get_redeemable_positions().await {
        Ok(positions) => {
            if positions.is_empty() {
                info!("No redeemable positions found.");
                info!("");
                info!("This could mean:");
                info!("  - No positions have resolved yet");
                info!("  - All resolved positions already redeemed");
                info!("  - No positions in this wallet");
                return Ok(());
            }

            info!("Found {} redeemable position(s):", positions.len());
            info!("");

            for (i, pos) in positions.iter().enumerate() {
                info!(
                    "  {}. {} - {} outcome: ${:.2}",
                    i + 1,
                    pos.title.as_deref().unwrap_or("Unknown"),
                    pos.outcome.as_deref().unwrap_or("?"),
                    pos.current_value.unwrap_or(0.0)
                );
            }

            info!("");
            info!("Starting redemption...");
            info!("");

            // Actually redeem
            match redeemer.redeem_all().await {
                Ok((count, total)) => {
                    info!("");
                    info!("╔═══════════════════════════════════════════════════════════════╗");
                    info!("║  REDEMPTION COMPLETE                                          ║");
                    info!("╠═══════════════════════════════════════════════════════════════╣");
                    info!("║  Positions redeemed: {:42} ║", count);
                    info!("║  Total value: ${:44.2} ║", total);
                    info!("╚═══════════════════════════════════════════════════════════════╝");
                }
                Err(e) => {
                    error!("Redemption failed: {}", e);
                }
            }
        }
        Err(e) => {
            error!("Failed to fetch positions: {}", e);
        }
    }

    Ok(())
}
