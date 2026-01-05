"""Debug why available capital is $0."""
import sys
sys.path.insert(0, '.')

from src.config import get_settings
from src.trading.client import PolymarketClient
from src.trading.positions import PositionTracker

settings = get_settings()

# Get real balance
client = PolymarketClient(settings.polymarket)
client.authenticate()
balance = client.get_balance()

# Simulate what main.py does
python_allocation = balance * settings.risk.balance_allocation_pct
print(f"Balance: ${balance:.2f}")
print(f"Allocation %: {settings.risk.balance_allocation_pct:.0%}")
print(f"Python max_exposure_usd: ${python_allocation:.2f}")

# Load position tracker
tracker = PositionTracker()

# Show all positions and their values
print(f"\n--- Positions in tracker ---")
for pos_id, pos in tracker.positions.items():
    status = "LIVE" if not pos.is_paper else "PAPER"
    print(f"  {status}: {pos.market_question[:40]}...")
    print(f"    size_usd (invested): ${pos.size_usd:.2f}")
    print(f"    current_value_usd: ${pos.current_value_usd:.2f}")
    print(f"    is_paper: {pos.is_paper}")

print(f"\n--- Capital Calculation ---")
print(f"max_exposure_usd (from config): ${tracker.max_exposure_usd:.2f}")
print(f"max_exposure_usd (should be): ${python_allocation:.2f}")

# Simulate setting the correct max
tracker.max_exposure_usd = python_allocation
print(f"\nAfter setting max_exposure to allocation:")
print(f"  max_exposure_usd: ${tracker.max_exposure_usd:.2f}")
print(f"  live_exposure_usd: ${tracker.live_exposure_usd:.2f}")
print(f"  available_capital: ${tracker.available_capital_usd:.2f}")

if tracker.live_exposure_usd > python_allocation:
    print(f"\n*** PROBLEM FOUND ***")
    print(f"Your live position value (${tracker.live_exposure_usd:.2f}) > allocation (${python_allocation:.2f})")
    print(f"This is why available = $0")
    print(f"\nThe position was opened when you had more capital.")
    print(f"Now your 50% allocation can't cover the existing position.")
