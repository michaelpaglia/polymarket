"""Interactive setup for Polymarket Bot."""

import os
from pathlib import Path


def setup() -> None:
    """Interactive setup wizard for the bot."""
    print("\n" + "=" * 60)
    print("  POLYMARKET BOT SETUP")
    print("=" * 60 + "\n")

    env_path = Path(__file__).parent.parent / ".env"

    # Check if .env exists
    if env_path.exists():
        print(f"Found existing .env file at {env_path}")
        response = input("Overwrite? (y/N): ").strip().lower()
        if response != "y":
            print("Setup cancelled.")
            return

    print("\nEnter your API keys (press Enter to skip optional ones):\n")

    # Polymarket credentials
    print("[POLYMARKET - Required for trading]")
    private_key = input("  Private Key (0x...): ").strip()
    funder_address = input("  Wallet Address (0x...): ").strip()

    # LLM
    print("\n[GEMINI - Required for signal analysis]")
    google_api_key = input("  Google API Key: ").strip()

    # News APIs
    print("\n[NEWS APIs - At least one recommended]")
    newsapi_key = input("  NewsAPI Key (newsapi.org): ").strip()
    gnews_key = input("  GNews Key (gnews.io): ").strip()

    # X/Twitter
    print("\n[X/TWITTER - Highly recommended for edge]")
    grok_key = input("  Grok API Key (x.ai): ").strip()

    # Optional
    print("\n[OPTIONAL]")
    polygon_rpc = input("  Polygon RPC URL: ").strip()

    # Write .env file
    env_content = f"""# Polymarket Credentials
POLYMARKET_PRIVATE_KEY={private_key}
POLYMARKET_FUNDER_ADDRESS={funder_address}

# LLM APIs (Gemini)
GOOGLE_API_KEY={google_api_key}

# News APIs
NEWSAPI_KEY={newsapi_key}
GNEWS_API_KEY={gnews_key}
GROK_API_KEY={grok_key}

# Optional: Polygon RPC for faster transactions
POLYGON_RPC_URL={polygon_rpc}
"""

    with open(env_path, "w") as f:
        f.write(env_content)

    print(f"\n[OK] Configuration saved to {env_path}")
    print("\nTo start the bot:")
    print("  Paper trading:  python -m src.main")
    print("  Live trading:   python -m src.main --live")
    print()


if __name__ == "__main__":
    setup()
