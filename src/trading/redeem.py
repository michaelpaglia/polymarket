"""Auto-redemption of resolved Polymarket positions."""

import httpx
from web3 import Web3
from eth_account import Account
from typing import Optional

from src.utils.logging import get_logger

# For newer web3 versions
try:
    from web3.middleware import geth_poa_middleware
except ImportError:
    from web3.middleware import ExtraDataToPOAMiddleware as geth_poa_middleware

logger = get_logger(__name__)

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


class PositionRedeemer:
    """Handles automatic redemption of resolved positions."""

    def __init__(self, private_key: str, rpc_url: str = "https://polygon-rpc.com"):
        self.private_key = private_key
        self.rpc_url = rpc_url
        self._w3: Optional[Web3] = None

    @property
    def w3(self) -> Web3:
        """Get or create Web3 instance."""
        if self._w3 is None:
            self._w3 = Web3(Web3.HTTPProvider(self.rpc_url))
            self._w3.middleware_onion.inject(geth_poa_middleware, layer=0)
        return self._w3

    @property
    def wallet_address(self) -> str:
        """Get wallet address from private key."""
        return Account.from_key(self.private_key).address

    def get_redeemable_positions(self) -> list[dict]:
        """Fetch positions that can be redeemed."""
        try:
            url = f"https://data-api.polymarket.com/positions?user={self.wallet_address.lower()}"
            resp = httpx.get(url, timeout=30)
            if resp.status_code != 200:
                return []

            positions = resp.json()
            # Filter for redeemable positions with value > 0
            return [
                p for p in positions
                if p.get('redeemable') and float(p.get('currentValue', 0)) > 0
            ]
        except Exception as e:
            logger.error(f"Failed to fetch redeemable positions: {e}")
            return []

    def redeem_position(self, position: dict) -> tuple[bool, float, str]:
        """
        Redeem a single position.

        Returns:
            Tuple of (success, amount_redeemed, message)
        """
        title = position.get('title', 'Unknown')[:50]
        outcome = position.get('outcome', '')
        condition_id = position.get('conditionId', '')
        value = float(position.get('currentValue', 0))
        neg_risk = position.get('negativeRisk', False)

        if not condition_id:
            return False, 0.0, "No condition ID"

        # Determine index set based on outcome
        if outcome.lower() == 'yes':
            index_sets = [1]
        else:
            index_sets = [2]

        # Choose contract based on neg_risk
        ctf_address = NEG_RISK_CTF_ADDRESS if neg_risk else CTF_ADDRESS
        ctf = self.w3.eth.contract(
            address=self.w3.to_checksum_address(ctf_address),
            abi=REDEEM_ABI
        )

        try:
            # Build transaction
            txn = ctf.functions.redeemPositions(
                self.w3.to_checksum_address(USDC_ADDRESS),
                bytes.fromhex("00" * 32),
                bytes.fromhex(condition_id[2:]) if condition_id.startswith('0x') else bytes.fromhex(condition_id),
                index_sets
            ).build_transaction({
                'from': self.wallet_address,
                'nonce': self.w3.eth.get_transaction_count(self.wallet_address),
                'gas': 300000,
                'gasPrice': self.w3.eth.gas_price,
                'chainId': 137
            })

            # Sign and send
            signed_txn = self.w3.eth.account.sign_transaction(txn, self.private_key)
            tx_hash = self.w3.eth.send_raw_transaction(signed_txn.raw_transaction)
            tx_hex = self.w3.to_hex(tx_hash)

            logger.info(f"Redeem TX sent: {tx_hex} for {title}")

            # Wait for confirmation
            receipt = self.w3.eth.wait_for_transaction_receipt(tx_hash, timeout=120)
            if receipt['status'] == 1:
                logger.info(f"Redeemed ${value:.2f} from {title}")
                return True, value, f"TX: {tx_hex}"
            else:
                return False, 0.0, "Transaction reverted"

        except Exception as e:
            logger.error(f"Redemption failed for {title}: {e}")
            return False, 0.0, str(e)

    def redeem_all(self) -> tuple[int, float]:
        """
        Redeem all available positions.

        Returns:
            Tuple of (count_redeemed, total_value_redeemed)
        """
        positions = self.get_redeemable_positions()
        if not positions:
            return 0, 0.0

        count = 0
        total = 0.0

        for pos in positions:
            success, value, msg = self.redeem_position(pos)
            if success:
                count += 1
                total += value
            else:
                logger.warning(f"Failed to redeem: {msg}")

        return count, total
