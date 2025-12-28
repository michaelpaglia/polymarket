#!/usr/bin/env python3
"""Check orderbook and slippage for a position before selling."""

import os
import sys
import argparse
from pathlib import Path

# Add project root to path
PROJECT_ROOT = Path(__file__).parent.parent
sys.path.insert(0, str(PROJECT_ROOT))

from dotenv import load_dotenv
load_dotenv()

from py_clob_client.client import ClobClient
from py_clob_client.clob_types import BalanceAllowanceParams, AssetType


def main():
    parser = argparse.ArgumentParser(description="Check position orderbook and slippage")
    parser.add_argument("--token-id", help="Token ID to check (auto-detects from recent trades if not provided)")
    parser.add_argument("--shares", type=float, help="Number of shares held")
    parser.add_argument("--entry-price", type=float, help="Entry price")
    parser.add_argument("--side", choices=["YES", "NO"], help="Position side (YES or NO)")
    args = parser.parse_args()

    private_key = os.environ.get("POLYMARKET_PRIVATE_KEY")
    if not private_key:
        print("ERROR: POLYMARKET_PRIVATE_KEY not set")
        return

    client = ClobClient(
        host="https://clob.polymarket.com",
        key=private_key,
        chain_id=137,
    )

    # Authenticate
    creds = client.create_or_derive_api_creds()
    client.set_api_creds(creds)

    # Get current balance
    params = BalanceAllowanceParams(asset_type=AssetType.COLLATERAL)
    result = client.get_balance_allowance(params)
    balance = int(result.get("balance", 0)) / 1_000_000
    print(f"\nCurrent USDC balance: ${balance:.2f}")

    # If no token ID provided, find from recent trades
    if not args.token_id:
        print("\nFetching recent trades to find position...")
        trades = client.get_trades()
        if not trades:
            print("No recent trades found. Please provide --token-id manually.")
            return

        # Get most recent trade
        trade = trades[0]
        token_id = trade.get("asset_id")
        shares = float(trade.get("size", 0))
        entry_price = float(trade.get("price", 0))
        side = trade.get("outcome", "Unknown")

        print(f"Found recent trade:")
        print(f"  Token ID: {token_id[:20]}...")
        print(f"  Side: {side}")
        print(f"  Shares: {shares}")
        print(f"  Entry price: {entry_price}")
    else:
        token_id = args.token_id
        shares = args.shares or 0
        entry_price = args.entry_price or 0
        side = args.side or "Unknown"

    if not token_id:
        print("ERROR: Could not determine token ID")
        return

    SHARES_HELD = shares
    ENTRY_PRICE = entry_price
    TOKEN_ID = token_id
    POSITION_SIDE = side

    # Get orderbook for the position's token
    print(f"\nChecking orderbook for {POSITION_SIDE} token: {TOKEN_ID[:30]}...")
    orderbook = client.get_order_book(TOKEN_ID)

    # Also get the midpoint to understand fair value
    midpoint_data = client.get_midpoint(TOKEN_ID)
    midpoint = float(midpoint_data.get("mid", 0)) if midpoint_data else 0

    print("\n" + "="*60)
    print(f"ORDERBOOK ANALYSIS ({POSITION_SIDE} POSITION)")
    print("="*60)
    print(f"Midpoint (fair value): {midpoint:.4f} ({midpoint*100:.2f}%)")

    # Show bids (what we'd sell into)
    print("\nBIDS (we sell into these):")
    total_bid_size = 0
    best_bid = 0
    if hasattr(orderbook, 'bids') and orderbook.bids:
        best_bid = float(orderbook.bids[0].price)
        for i, bid in enumerate(orderbook.bids[:5]):
            size = float(bid.size)
            price = float(bid.price)
            total_bid_size += size
            print(f"  {i+1}. {price:.4f} ({price*100:.2f}%) - Size: {size:.2f}")
    else:
        print("  NO BIDS - Cannot sell!")

    # Show asks
    print("\nASKS (current offer prices):")
    if hasattr(orderbook, 'asks') and orderbook.asks:
        for i, ask in enumerate(orderbook.asks[:5]):
            size = float(ask.size)
            price = float(ask.price)
            print(f"  {i+1}. {price:.4f} ({price*100:.2f}%) - Size: {size:.2f}")
    else:
        print("  No asks")

    # Calculate spread
    if best_bid > 0 and midpoint > 0:
        spread_pct = (midpoint - best_bid) / midpoint * 100
        print(f"\nSpread from midpoint: {spread_pct:.1f}%")
        if spread_pct > 10:
            print("  [!] WARNING: Very wide spread! Low liquidity market.")

    # Calculate slippage for selling our shares
    print("\n" + "="*60)
    print(f"SELL ANALYSIS ({POSITION_SIDE} SHARES)")
    print("="*60)
    print(f"Shares to sell: {SHARES_HELD:.2f}")
    print(f"Entry price: {ENTRY_PRICE:.4f} ({ENTRY_PRICE*100:.2f}%)")
    print(f"Entry cost: ${SHARES_HELD * ENTRY_PRICE:.2f}")
    print(f"Fair value (midpoint): ${SHARES_HELD * midpoint:.2f}")

    if hasattr(orderbook, 'bids') and orderbook.bids:
        # Simulate selling into bids
        shares_remaining = SHARES_HELD
        total_proceeds = 0
        levels_hit = 0

        for bid in orderbook.bids:
            if shares_remaining <= 0:
                break

            size = float(bid.size)
            price = float(bid.price)
            fill = min(shares_remaining, size)
            total_proceeds += fill * price
            shares_remaining -= fill
            levels_hit += 1

        if shares_remaining > 0:
            print(f"\n[!] WARNING: Only {SHARES_HELD - shares_remaining:.2f} of {SHARES_HELD:.2f} shares can be sold!")
            print(f"    {shares_remaining:.2f} shares have NO BIDS")

        filled_shares = SHARES_HELD - shares_remaining
        avg_sell_price = total_proceeds / filled_shares if filled_shares > 0 else 0
        slippage = (ENTRY_PRICE - avg_sell_price) / ENTRY_PRICE * 100 if ENTRY_PRICE > 0 else 0
        pnl = total_proceeds - (SHARES_HELD * ENTRY_PRICE)
        pnl_at_resolution = SHARES_HELD * 1.0 - (SHARES_HELD * ENTRY_PRICE)  # If position wins

        print(f"\n--- If selling NOW ---")
        print(f"Estimated proceeds: ${total_proceeds:.2f}")
        print(f"Average sell price: {avg_sell_price:.4f} ({avg_sell_price*100:.2f}%)")
        print(f"Slippage from entry: {slippage:.2f}%")
        print(f"Estimated P&L: ${pnl:+.2f}")
        print(f"Levels to fill: {levels_hit}")

        print(f"\n--- If holding until resolution ---")
        print(f"If {POSITION_SIDE} wins: ${SHARES_HELD:.2f} (P&L: ${pnl_at_resolution:+.2f})")
        print(f"If {POSITION_SIDE} loses: $0.00 (P&L: ${-SHARES_HELD * ENTRY_PRICE:.2f})")

        # Recommendation
        print("\n" + "="*60)
        print("RECOMMENDATION")
        print("="*60)
        if avg_sell_price < 0.01 and midpoint > 0.90:
            print("[!] Market is illiquid! Best bid is far below fair value.")
            print(f"    Midpoint suggests {POSITION_SIDE} @ {midpoint*100:.1f}%")
            print(f"    Consider HOLDING until resolution instead of selling at ${total_proceeds:.2f}")
        elif avg_sell_price > ENTRY_PRICE * 0.95:
            print("[OK] Liquidity looks good. Can exit near entry price.")
    else:
        print("\n[X] CANNOT SELL - No bids in orderbook!")
        print("    This market may have resolved or have no liquidity.")
        print(f"\n    If market resolves to {POSITION_SIDE}: You get ${SHARES_HELD:.2f}")
        print(f"    If market resolves opposite: You get $0")


if __name__ == "__main__":
    main()
