"""Auto-redeem resolved Polymarket positions."""
import sys
sys.path.insert(0, '.')

import httpx
from web3 import Web3
from eth_account import Account

# For newer web3 versions
try:
    from web3.middleware import geth_poa_middleware
except ImportError:
    from web3.middleware import ExtraDataToPOAMiddleware as geth_poa_middleware

from src.config import get_settings

# Contract addresses on Polygon
CTF_ADDRESS = "0x4D97DCd97eC945f40cF65F87097ACe5EA0476045"
NEG_RISK_CTF_ADDRESS = "0xC5d563A36AE78145C45a50134d48A1215220f80a"
USDC_ADDRESS = "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174"

# ABI for redeemPositions
REDEEM_ABI = [{
    "constant": False,
    "inputs": [
        {"name": "collateralToken", "type": "address"},
        {"name": "parentCollectionId", "type": "bytes32"},
        {"name": "conditionId", "type": "bytes32"},
        {"name": "indexSets", "type": "uint256[]"}
    ],
    "name": "redeemPositions",
    "outputs": [],
    "payable": False,
    "stateMutability": "nonpayable",
    "type": "function"
}]

def main():
    settings = get_settings()

    if not settings.polymarket.private_key:
        print("ERROR: No private key configured")
        return

    # Setup Web3
    rpc_url = "https://polygon-rpc.com"  # Public Polygon RPC
    w3 = Web3(Web3.HTTPProvider(rpc_url))
    w3.middleware_onion.inject(geth_poa_middleware, layer=0)

    account = Account.from_key(settings.polymarket.private_key)
    wallet_address = account.address
    print(f"Wallet: {wallet_address}")

    # Get positions from API
    url = f"https://data-api.polymarket.com/positions?user={wallet_address.lower()}"
    resp = httpx.get(url, timeout=30)
    positions = resp.json()

    # Find redeemable positions with value > 0
    redeemable = [p for p in positions if p.get('redeemable') and float(p.get('currentValue', 0)) > 0]

    if not redeemable:
        print("No positions to redeem.")
        return

    print(f"\nFound {len(redeemable)} redeemable position(s):\n")

    for pos in redeemable:
        title = pos.get('title', 'Unknown')[:50]
        outcome = pos.get('outcome')
        condition_id = pos.get('conditionId')
        value = float(pos.get('currentValue', 0))
        neg_risk = pos.get('negativeRisk', False)

        print(f"  {title}...")
        print(f"    Outcome: {outcome} | Value: ${value:.2f}")
        print(f"    Condition ID: {condition_id}")
        print(f"    Negative Risk: {neg_risk}")

        # Determine index set based on outcome
        # For binary markets: YES=1, NO=2
        if outcome.lower() == 'yes':
            index_sets = [1]
        else:
            index_sets = [2]

        # Choose contract based on neg_risk
        ctf_address = NEG_RISK_CTF_ADDRESS if neg_risk else CTF_ADDRESS
        ctf = w3.eth.contract(
            address=w3.to_checksum_address(ctf_address),
            abi=REDEEM_ABI
        )

        try:
            # Build transaction
            txn = ctf.functions.redeemPositions(
                w3.to_checksum_address(USDC_ADDRESS),
                bytes.fromhex("00" * 32),  # parentCollectionId = 0x0
                bytes.fromhex(condition_id[2:] if condition_id.startswith('0x') else condition_id),
                index_sets
            ).build_transaction({
                'from': wallet_address,
                'nonce': w3.eth.get_transaction_count(wallet_address),
                'gas': 300000,
                'gasPrice': w3.eth.gas_price,
                'chainId': 137  # Polygon
            })

            # Sign and send
            signed_txn = w3.eth.account.sign_transaction(txn, settings.polymarket.private_key)
            tx_hash = w3.eth.send_raw_transaction(signed_txn.raw_transaction)
            print(f"    TX Hash: {w3.to_hex(tx_hash)}")

            # Wait for confirmation
            receipt = w3.eth.wait_for_transaction_receipt(tx_hash, timeout=120)
            if receipt['status'] == 1:
                print(f"    SUCCESS: Redeemed ${value:.2f}")
            else:
                print(f"    FAILED: Transaction reverted")

        except Exception as e:
            print(f"    ERROR: {e}")

        print()

    # Check new balance
    from src.trading.client import PolymarketClient
    client = PolymarketClient(settings.polymarket)
    client.authenticate()
    new_balance = client.get_balance()
    print(f"New USDC Balance: ${new_balance:.2f}")

if __name__ == "__main__":
    main()
