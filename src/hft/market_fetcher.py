"""
Market Fetcher - Automatically subscribe to ALL Polymarket markets.

Fetches all open markets from Polymarket API and subscribes them
to the Rust HFT server for arbitrage monitoring.
"""

import asyncio
import json
import httpx
from typing import Optional
from dataclasses import dataclass


@dataclass
class Market:
    """Polymarket market with token IDs."""
    market_id: str
    slug: str
    question: str
    yes_token_id: str
    no_token_id: str
    liquidity: float
    volume_24h: float


async def fetch_all_markets(
    min_liquidity: float = 1000.0,
    min_volume_24h: float = 100.0,
) -> list[Market]:
    """
    Fetch all open markets from Polymarket.

    Args:
        min_liquidity: Minimum liquidity in USD (filter out tiny markets)
        min_volume_24h: Minimum 24h volume in USD

    Returns:
        List of Market objects with token IDs
    """
    markets = []
    offset = 0
    limit = 100

    async with httpx.AsyncClient(timeout=30.0) as client:
        while True:
            url = f"https://gamma-api.polymarket.com/markets?closed=false&limit={limit}&offset={offset}"

            try:
                response = await client.get(url)
                response.raise_for_status()
                data = response.json()
            except Exception as e:
                print(f"Error fetching markets at offset {offset}: {e}")
                break

            if not data:
                break

            for m in data:
                try:
                    # Parse token IDs
                    token_ids = json.loads(m.get("clobTokenIds", "[]"))
                    if len(token_ids) != 2:
                        continue

                    # Parse liquidity and volume
                    liquidity = float(m.get("liquidityNum", 0) or 0)
                    volume_24h = float(m.get("volume24hr", 0) or 0)

                    # Filter by liquidity and volume
                    if liquidity < min_liquidity:
                        continue
                    if volume_24h < min_volume_24h:
                        continue

                    # Check if orderbook is enabled
                    if not m.get("enableOrderBook", False):
                        continue

                    markets.append(Market(
                        market_id=m.get("slug", m.get("id", "")),
                        slug=m.get("slug", ""),
                        question=m.get("question", "")[:100],
                        yes_token_id=token_ids[0],
                        no_token_id=token_ids[1],
                        liquidity=liquidity,
                        volume_24h=volume_24h,
                    ))
                except (KeyError, ValueError, json.JSONDecodeError) as e:
                    continue

            offset += limit

            # Stop if we got fewer than limit (end of data)
            if len(data) < limit:
                break

    # Sort by liquidity (most liquid first)
    markets.sort(key=lambda m: m.liquidity, reverse=True)

    return markets


async def subscribe_markets_to_hft(
    markets: list[Market],
    hft_url: str = "http://127.0.0.1:8080",
    batch_size: int = 50,
) -> dict:
    """
    Subscribe markets to the HFT server.

    Args:
        markets: List of markets to subscribe
        hft_url: Base URL of HFT server
        batch_size: How many to subscribe at once

    Returns:
        Dict with subscription stats
    """
    stats = {
        "total": len(markets),
        "subscribed": 0,
        "failed": 0,
        "errors": [],
    }

    async with httpx.AsyncClient(timeout=10.0) as client:
        for i, market in enumerate(markets):
            try:
                payload = {
                    "market_id": market.market_id,
                    "yes_token_id": market.yes_token_id,
                    "no_token_id": market.no_token_id,
                }

                response = await client.post(
                    f"{hft_url}/api/v1/markets/subscribe",
                    json=payload,
                )

                if response.status_code == 200:
                    stats["subscribed"] += 1
                else:
                    stats["failed"] += 1
                    stats["errors"].append(f"{market.market_id}: {response.status_code}")

            except Exception as e:
                stats["failed"] += 1
                stats["errors"].append(f"{market.market_id}: {e}")

            # Progress logging
            if (i + 1) % 50 == 0:
                print(f"Subscribed {i + 1}/{len(markets)} markets...")

    return stats


async def connect_and_subscribe_all(
    hft_url: str = "http://127.0.0.1:8080",
    min_liquidity: float = 1000.0,
    min_volume_24h: float = 100.0,
    top_n: Optional[int] = None,
) -> dict:
    """
    Fetch all markets and subscribe to HFT server.

    Args:
        hft_url: Base URL of HFT server
        min_liquidity: Minimum liquidity filter
        min_volume_24h: Minimum 24h volume filter
        top_n: Only subscribe to top N markets by liquidity (None = all)

    Returns:
        Dict with stats
    """
    print("Fetching all open markets from Polymarket...")
    markets = await fetch_all_markets(
        min_liquidity=min_liquidity,
        min_volume_24h=min_volume_24h,
    )

    print(f"Found {len(markets)} markets meeting criteria")

    if top_n:
        markets = markets[:top_n]
        print(f"Limiting to top {top_n} by liquidity")

    # First connect to WebSocket
    print("Connecting to WebSocket...")
    async with httpx.AsyncClient(timeout=30.0) as client:
        try:
            response = await client.post(
                f"{hft_url}/api/v1/connect",
                json={"markets": []},
            )
            if response.status_code == 200:
                print("WebSocket connected")
            else:
                print(f"WebSocket connection failed: {response.status_code}")
        except Exception as e:
            print(f"WebSocket connection error: {e}")

    # Subscribe to markets
    print(f"Subscribing to {len(markets)} markets...")
    stats = await subscribe_markets_to_hft(markets, hft_url)

    print()
    print("=" * 50)
    print("Subscription Complete")
    print("=" * 50)
    print(f"Total markets: {stats['total']}")
    print(f"Subscribed: {stats['subscribed']}")
    print(f"Failed: {stats['failed']}")

    if stats["errors"]:
        print(f"\nFirst 5 errors:")
        for err in stats["errors"][:5]:
            print(f"  {err}")

    return stats


async def main():
    """Main entry point."""
    import argparse

    parser = argparse.ArgumentParser(description="Subscribe all markets to HFT server")
    parser.add_argument("--url", default="http://127.0.0.1:8080", help="HFT server URL")
    parser.add_argument("--min-liquidity", type=float, default=1000.0, help="Min liquidity USD")
    parser.add_argument("--min-volume", type=float, default=100.0, help="Min 24h volume USD")
    parser.add_argument("--top", type=int, default=None, help="Only top N markets")

    args = parser.parse_args()

    await connect_and_subscribe_all(
        hft_url=args.url,
        min_liquidity=args.min_liquidity,
        min_volume_24h=args.min_volume,
        top_n=args.top,
    )


if __name__ == "__main__":
    asyncio.run(main())
