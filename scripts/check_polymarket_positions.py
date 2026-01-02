"""Check actual positions on Polymarket (not bot's local tracking)."""
import sys
sys.path.insert(0, '.')

import httpx
from src.config import get_settings
from src.trading.client import PolymarketClient

settings = get_settings()
client = PolymarketClient(settings.polymarket)

print("--- Authenticating with Polymarket ---")
if client.authenticate():
    print("Authentication successful!")
else:
    print("Authentication failed!")
    sys.exit(1)

print(f"\n--- Account Balance ---")
balance = client.get_balance()
print(f"USDC Balance: ${balance:.2f}")

# Get the wallet address from the private key
from eth_account import Account
wallet_address = Account.from_key(settings.polymarket.private_key).address
print(f"Wallet Address: {wallet_address}")

# Fetch positions from Polymarket Data API
print(f"\n--- Fetching Positions from Polymarket API ---")
try:
    # The correct endpoint for positions
    url = f"https://data-api.polymarket.com/positions?user={wallet_address.lower()}"

    response = httpx.get(url, timeout=30)
    if response.status_code == 200:
        positions = response.json()

        if not positions:
            print("No open positions found on Polymarket.")
        else:
            print(f"Found {len(positions)} position(s):\n")

            total_value = 0.0
            total_pnl = 0.0
            redeemable_value = 0.0

            for pos in positions:
                # Get market info from data-api format
                title = pos.get('title', 'Unknown market')
                outcome = pos.get('outcome', 'unknown')
                size = float(pos.get('size', 0))
                avg_price = float(pos.get('avgPrice', 0))
                cur_price = float(pos.get('curPrice', 0))
                current_value = float(pos.get('currentValue', 0))
                cash_pnl = float(pos.get('cashPnl', 0))
                pct_pnl = float(pos.get('percentPnl', 0))
                redeemable = pos.get('redeemable', False)
                end_date = pos.get('endDate', '')

                total_value += current_value
                total_pnl += cash_pnl
                if redeemable:
                    redeemable_value += current_value

                status = "[RESOLVED - REDEEMABLE]" if redeemable else "[OPEN]"
                print(f"  {status}")
                print(f"    Market: {title[:60]}...")
                print(f"    Outcome: {outcome}")
                print(f"    Size: {size:.2f} shares @ ${avg_price:.3f} avg")
                print(f"    Current: ${cur_price:.3f} | Value: ${current_value:.2f}")
                print(f"    P&L: ${cash_pnl:+.2f} ({pct_pnl:+.1f}%)")
                print(f"    End Date: {end_date}")
                print()

            print(f"Total Position Value: ${total_value:.2f}")
            print(f"Total P&L: ${total_pnl:+.2f}")
            if redeemable_value > 0:
                print(f"\n*** REDEEMABLE: ${redeemable_value:.2f} - Go to Polymarket to claim! ***")
    else:
        print(f"API Error: {response.status_code} - {response.text[:200]}")

except Exception as e:
    print(f"Error fetching positions: {e}")

# Get recent trades
print(f"\n--- Recent Trades ---")
try:
    trades = client.get_trades()
    if trades:
        print(f"Found {len(trades)} recent trade(s)")
        for trade in trades[:5]:  # Show last 5
            print(f"  {trade}")
    else:
        print("No recent trades found")
except Exception as e:
    print(f"Error fetching trades: {e}")

# Also show what the local bot thinks
print(f"\n--- Bot's Local Position Tracking ---")
from src.trading.positions import PositionTracker
tracker = PositionTracker()

print(f"Total positions tracked locally: {len(tracker.positions)}")
paper_count = sum(1 for p in tracker.positions.values() if p.is_paper)
live_count = sum(1 for p in tracker.positions.values() if not p.is_paper)
print(f"Paper positions: {paper_count}")
print(f"Live positions: {live_count}")

if tracker.positions:
    print("\nLocal positions:")
    for pos_id, pos in tracker.positions.items():
        status = "PAPER" if pos.is_paper else "LIVE"
        print(f"  [{status}] {pos.market_question[:50]}... | ${pos.size_usd:.2f}")
