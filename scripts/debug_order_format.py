#!/usr/bin/env python3
"""Debug script to show what Python order format looks like"""

import json
import os
import sys

# Add parent to path to use project venv
sys.path.insert(0, os.path.dirname(os.path.dirname(os.path.abspath(__file__))))

from py_clob_client.client import ClobClient
from py_clob_client.clob_types import OrderArgs
from dotenv import load_dotenv

load_dotenv()

CLOB_HOST = "https://clob.polymarket.com"
CHAIN_ID = 137

def main():
    private_key = os.getenv("POLYMARKET_PRIVATE_KEY")
    if not private_key:
        print("POLYMARKET_PRIVATE_KEY not set")
        return

    # Initialize client with L1 auth only (no API creds needed for order creation)
    client = ClobClient(
        host=CLOB_HOST,
        chain_id=CHAIN_ID,
        key=private_key,
    )

    # Create a test order (won't be submitted)
    order_args = OrderArgs(
        token_id="94900551111414304030821160772835773497454672642220061553895509690316026970543",
        side="BUY",
        price=0.475,
        size=2.11,
        fee_rate_bps=0,
    )

    try:
        # Create order
        signed_order = client.create_order(order_args)

        # Show the raw dict
        print("=== signed_order.dict() ===")
        order_dict = signed_order.dict()
        print(json.dumps(order_dict, indent=2))

        # Show what order_to_json produces
        from py_clob_client.utilities import order_to_json
        from py_clob_client.clob_types import OrderType

        full_payload = order_to_json(signed_order, "test_api_key", OrderType.GTC)
        print("\n=== Full order_to_json payload ===")
        print(json.dumps(full_payload, indent=2))

        # Show compact version (what actually gets sent)
        print("\n=== Compact JSON (what gets sent) ===")
        compact = json.dumps(full_payload, separators=(",", ":"), ensure_ascii=False)
        print(compact)

        # Show field types
        print("\n=== Field types in order dict ===")
        for key, value in order_dict.items():
            print(f"  {key}: {type(value).__name__} = {repr(value)[:50]}")

    except Exception as e:
        print(f"Error: {e}")
        import traceback
        traceback.print_exc()

if __name__ == "__main__":
    main()
