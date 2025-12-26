"""Interactive setup for Polymarket Trading System."""

import os
from pathlib import Path


def setup() -> None:
    """Interactive setup wizard for the trading system."""
    print("\n" + "=" * 60)
    print("  POLYMARKET TRADING SYSTEM SETUP")
    print("=" * 60)
    print()
    print("This wizard configures both trading modules:")
    print("  1. Sentiment Bot (Python) - News/Twitter analysis")
    print("  2. HFT Arbitrage (Rust) - YES/NO mispricing")
    print()

    env_path = Path(__file__).parent.parent / ".env"

    # Check if .env exists
    if env_path.exists():
        print(f"Found existing .env file at {env_path}")
        response = input("Overwrite? (y/N): ").strip().lower()
        if response != "y":
            print("Setup cancelled.")
            return

    print("\nEnter your credentials (press Enter to skip optional ones):\n")

    # Polymarket credentials
    print("=" * 40)
    print("[POLYMARKET - Required for trading]")
    print("=" * 40)
    print("Export private key from MetaMask: Settings > Security > Export")
    private_key = input("  Private Key (0x...): ").strip()
    funder_address = input("  Wallet Address (0x...): ").strip()

    # API credentials for HFT
    print()
    print("[POLYMARKET API - Required for HFT module]")
    print("Get these from: https://polymarket.com/settings/api")
    api_key = input("  API Key: ").strip()
    api_secret = input("  API Secret: ").strip()
    api_passphrase = input("  API Passphrase: ").strip()

    # LLM
    print()
    print("=" * 40)
    print("[GOOGLE GEMINI - Required for sentiment bot]")
    print("=" * 40)
    print("Get key from: https://aistudio.google.com/")
    google_api_key = input("  Google API Key: ").strip()

    # News APIs
    print()
    print("=" * 40)
    print("[NEWS APIs - At least one recommended]")
    print("=" * 40)
    print("NewsAPI: https://newsapi.org/ (free: 100 req/day)")
    newsapi_key = input("  NewsAPI Key: ").strip()
    print("GNews: https://gnews.io/ (free: 100 req/day)")
    gnews_key = input("  GNews Key: ").strip()

    # X/Twitter
    print()
    print("=" * 40)
    print("[GROK/X - Highly recommended for edge]")
    print("=" * 40)
    print("Get key from: https://x.ai/")
    print("This enables real-time X/Twitter search for alpha.")
    grok_key = input("  Grok API Key: ").strip()

    # HFT settings
    print()
    print("=" * 40)
    print("[HFT SETTINGS - Optional]")
    print("=" * 40)
    hft_capital = input("  HFT Capital USD (default: 5000): ").strip() or "5000"

    # Optional
    print()
    print("[OPTIONAL]")
    polygon_rpc = input("  Polygon RPC URL (for faster txs): ").strip()

    # Write .env file
    env_content = f"""# ============================================
# Polymarket Trading System Configuration
# ============================================

# Polymarket Wallet
POLYMARKET_PRIVATE_KEY={private_key}
POLYMARKET_FUNDER_ADDRESS={funder_address}

# Polymarket API (for HFT module)
POLYMARKET_API_KEY={api_key}
POLYMARKET_API_SECRET={api_secret}
POLYMARKET_API_PASSPHRASE={api_passphrase}

# LLM APIs
GOOGLE_API_KEY={google_api_key}

# News APIs
NEWSAPI_KEY={newsapi_key}
GNEWS_API_KEY={gnews_key}

# X/Twitter via Grok
GROK_API_KEY={grok_key}

# HFT Settings
HFT_CAPITAL_USD={hft_capital}

# Optional: Polygon RPC for faster transactions
POLYGON_RPC_URL={polygon_rpc}
"""

    with open(env_path, "w") as f:
        f.write(env_content)

    print()
    print("=" * 60)
    print("  SETUP COMPLETE")
    print("=" * 60)
    print()
    print(f"Configuration saved to {env_path}")
    print()
    print("Next steps:")
    print()
    print("  1. Build the Rust HFT module (first time only):")
    print("     cd rust-hft && cargo build --release && cd ..")
    print()
    print("  2. Run both modules:")
    print("     python scripts/run_all.py")
    print()
    print("  Or run individually:")
    print("     python -m src.main          # Sentiment bot only")
    print("     cd rust-hft && cargo run    # HFT only")
    print()
    print("  Paper trading is enabled by default. Edit config.yaml to change.")
    print()


if __name__ == "__main__":
    setup()
