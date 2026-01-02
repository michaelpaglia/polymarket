"""Diagnostic script to check balance and capital calculations."""
import sys
sys.path.insert(0, '.')
from src.config import get_settings
from src.trading.client import PolymarketClient
from src.trading.positions import PositionTracker

# Load settings
settings = get_settings()
# Force non-paper mode for testing
original_paper = settings.paper_trading
settings.paper_trading = False

print(f'Paper trading mode (config): {original_paper}')
print(f'Paper trading mode (forced for test): {settings.paper_trading}')
print(f'Max portfolio exposure from config: ${settings.risk.max_portfolio_exposure_usd}')
print(f'Balance allocation pct: {settings.risk.balance_allocation_pct}')

# Create Polymarket client (needs PolymarketSettings, not full Settings)
print('\n--- Checking Polymarket Balance ---')
client = PolymarketClient(settings.polymarket)
print(f'Has private key: {bool(settings.polymarket.private_key)}')
print(f'Client authenticated: {client._authenticated}')

# Authenticate if not already
if not client._authenticated and settings.polymarket.private_key:
    auth_result = client.authenticate()
    print(f'Authentication result: {auth_result}')
    print(f'Client authenticated after auth(): {client._authenticated}')

balance = client.get_balance()
print(f'Polymarket balance: ${balance:.2f}')

if balance > 0:
    python_allocation = balance * settings.risk.balance_allocation_pct
    print(f'Python allocation ({settings.risk.balance_allocation_pct:.0%}): ${python_allocation:.2f}')
else:
    print('WARNING: Balance is 0!')

# Check position tracker
print('\n--- Checking Position Tracker ---')
tracker = PositionTracker()
print(f'Position tracker max_exposure_usd: ${tracker.max_exposure_usd:.2f}')
print(f'Number of positions: {len(tracker.positions)}')
print(f'Live exposure: ${tracker.live_exposure_usd:.2f}')
print(f'Paper exposure: ${tracker.paper_exposure_usd:.2f}')
print(f'Available capital: ${tracker.available_capital_usd:.2f}')

print('\n--- Position Details ---')
for pos_id, pos in tracker.positions.items():
    is_paper = "PAPER" if pos.is_paper else "LIVE"
    question = pos.market_question[:50] if pos.market_question else "Unknown"
    print(f'  {pos_id}: {is_paper} | Value: ${pos.current_value_usd:.2f} | {question}...')

# Key diagnosis
print('\n--- DIAGNOSIS ---')
print(f'max_exposure_usd = ${tracker.max_exposure_usd:.2f}')
print(f'live_exposure_usd = ${tracker.live_exposure_usd:.2f}')
print(f'available = max - live = ${tracker.max_exposure_usd:.2f} - ${tracker.live_exposure_usd:.2f} = ${tracker.available_capital_usd:.2f}')

if tracker.available_capital_usd == 0:
    if tracker.max_exposure_usd == 0:
        print('\nPROBLEM: max_exposure_usd is 0! Balance not being fetched or set correctly.')
    elif tracker.live_exposure_usd >= tracker.max_exposure_usd:
        print(f'\nPROBLEM: Live exposure (${tracker.live_exposure_usd:.2f}) >= max allowed (${tracker.max_exposure_usd:.2f})')
        print('This means the position tracker was initialized with a lower max than expected.')
        print(f'Expected max should be: ${python_allocation:.2f} ({settings.risk.balance_allocation_pct:.0%} of balance)' if balance > 0 else 'Expected max unknown - balance is 0')

# Simulate what happens in live mode
print('\n--- SIMULATING LIVE MODE ---')
if balance > 0:
    python_allocation = balance * settings.risk.balance_allocation_pct
    # This is what main.py does: tracker.max_exposure_usd = python_allocation
    simulated_max = python_allocation
    print(f'In LIVE mode, max_exposure_usd would be set to: ${simulated_max:.2f}')
    print(f'Current live exposure: ${tracker.live_exposure_usd:.2f}')
    simulated_available = max(0, simulated_max - tracker.live_exposure_usd)
    print(f'Simulated available capital: ${simulated_available:.2f}')

    if simulated_available < 5.0:
        print(f'\n*** LOW CAPITAL WARNING ***')
        print(f'With balance ${balance:.2f} and {settings.risk.balance_allocation_pct:.0%} allocation (${python_allocation:.2f}),')
        print(f'available capital is ${simulated_available:.2f}.')
        print(f'\nOptions:')
        print(f'  1. Deposit more USDC to your Polymarket account')
        print(f'  2. Adjust --allocation flag when running bot')
        print(f'  3. Close existing live positions to free up capital')
