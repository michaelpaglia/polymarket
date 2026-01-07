#!/usr/bin/env python3
"""Analyze recent Polymarket trades"""

import os
from dotenv import load_dotenv
from py_clob_client.client import ClobClient
from py_clob_client.clob_types import ApiCreds
from datetime import datetime

load_dotenv()

client = ClobClient(
    host='https://clob.polymarket.com',
    chain_id=137,
    key=os.getenv('POLYMARKET_PRIVATE_KEY'),
    creds=ApiCreds(
        api_key=os.getenv('POLY_API_KEY') or '',
        api_secret=os.getenv('POLY_API_SECRET') or '',
        api_passphrase=os.getenv('POLY_API_PASSPHRASE') or '',
    )
)

if not os.getenv('POLY_API_KEY'):
    creds = client.derive_api_key()
    client.set_api_creds(creds)

trades = client.get_trades()

# Calculate totals
total_cost = 0
wins = 0
losses = 0

print(f"Found {len(trades)} trades\n")
print("=" * 80)
print(f"{'Time':10} | {'Side':4} | {'Price':6} | {'Size':8} | {'Cost':8} | {'Outcome'}")
print("=" * 80)

for trade in trades[:30]:  # Last 30 trades
    try:
        price = float(trade.get('price', 0))
        size = float(trade.get('size', 0))
        cost = price * size
        total_cost += cost

        ts = int(trade.get('match_time', 0))
        dt = datetime.fromtimestamp(ts) if ts > 0 else None
        time_str = dt.strftime('%H:%M:%S') if dt else str(ts)

        side = trade.get('side', '')
        outcome = trade.get('outcome', 'pending')

        # Track wins/losses
        if outcome == 'Yes':
            wins += 1
        elif outcome == 'No':
            losses += 1

        marker = ""
        if cost > 5:
            marker = " <-- BIG"

        print(f"{time_str:10} | {side:4} | ${price:.2f}  | {size:8.2f} | ${cost:7.2f} | {outcome}{marker}")
    except Exception as e:
        print(f"Error: {e}")

print("=" * 80)
print(f"\nTotal cost: ${total_cost:.2f}")
print(f"Wins: {wins}, Losses: {losses}")
