"""Polymarket CLOB client wrapper."""

from dataclasses import dataclass
from typing import Any

from py_clob_client.client import ClobClient
from py_clob_client.clob_types import (
    AssetType,
    BalanceAllowanceParams,
    MarketOrderArgs,
    OpenOrderParams,
    OrderArgs,
    OrderType,
)
from py_clob_client.order_builder.constants import BUY, SELL

from src.config import PolymarketSettings
from src.utils.logging import get_logger

logger = get_logger(__name__)

# USDC contract on Polygon
USDC_ADDRESS = "0x2791Bca1f2de4661ED88A30C99A7a9449Aa84174"


@dataclass
class OrderResult:
    """Result of an order placement."""

    success: bool
    order_id: str | None
    message: str
    filled_amount: float = 0.0
    average_price: float = 0.0


class PolymarketClient:
    """
    Wrapper around py-clob-client for Polymarket trading.

    Provides a simplified interface for:
    - Fetching market data
    - Placing limit and market orders
    - Managing positions
    """

    def __init__(self, settings: PolymarketSettings) -> None:
        """
        Initialize the Polymarket client.

        Args:
            settings: Polymarket configuration settings
        """
        self.settings = settings
        self._client: ClobClient | None = None
        self._authenticated = False

    @property
    def client(self) -> ClobClient:
        """Get or create the CLOB client."""
        if self._client is None:
            # ClobClient accepts Optional[str] for key
            key = self.settings.private_key if self.settings.private_key else None
            self._client = ClobClient(
                self.settings.host,
                key=key,  # type: ignore[arg-type]
                chain_id=self.settings.chain_id,
            )
        return self._client

    def authenticate(self) -> bool:
        """
        Authenticate with the Polymarket API.

        Returns:
            True if authentication successful
        """
        if not self.settings.private_key:
            logger.warning("No private key configured, running in read-only mode")
            return False

        try:
            # Create or derive API credentials
            creds = self.client.create_or_derive_api_creds()
            self.client.set_api_creds(creds)
            self._authenticated = True
            logger.info("Successfully authenticated with Polymarket")
            return True
        except Exception as e:
            logger.error("Failed to authenticate", error=str(e))
            return False

    def is_healthy(self) -> bool:
        """Check if the API is healthy."""
        try:
            self.client.get_ok()
            return True
        except Exception as e:
            logger.error("Health check failed", error=str(e))
            return False

    def get_server_time(self) -> int:
        """Get the server timestamp."""
        result: Any = self.client.get_server_time()
        return int(result) if result else 0

    def get_markets(self) -> list[dict[str, Any]]:
        """
        Get all simplified markets.

        Returns:
            List of market dictionaries
        """
        try:
            result: Any = self.client.get_simplified_markets()
            return list(result) if result else []
        except Exception as e:
            logger.error("Failed to fetch markets", error=str(e))
            return []

    def get_market(self, condition_id: str) -> dict[str, Any] | None:
        """
        Get a specific market by condition ID.

        Args:
            condition_id: The market condition ID

        Returns:
            Market data or None if not found
        """
        try:
            result: Any = self.client.get_market(condition_id)
            return dict(result) if result else None
        except Exception as e:
            logger.error("Failed to fetch market", condition_id=condition_id, error=str(e))
            return None

    def get_orderbook(self, token_id: str) -> dict[str, Any] | None:
        """
        Get the orderbook for a token.

        Args:
            token_id: The token ID (YES or NO outcome)

        Returns:
            Orderbook data or None
        """
        try:
            result: Any = self.client.get_order_book(token_id)
            # Convert OrderBookSummary to dict if needed
            if hasattr(result, "__dict__"):
                return vars(result)
            return dict(result) if result else None
        except Exception as e:
            logger.error("Failed to fetch orderbook", token_id=token_id, error=str(e))
            return None

    def get_midpoint(self, token_id: str) -> float | None:
        """
        Get the midpoint price for a token.

        Args:
            token_id: The token ID

        Returns:
            Midpoint price (0.0-1.0) or None
        """
        try:
            result: Any = self.client.get_midpoint(token_id)
            return float(result) if result is not None else None
        except Exception as e:
            logger.error("Failed to fetch midpoint", token_id=token_id, error=str(e))
            return None

    def get_price(self, token_id: str, side: str = "BUY") -> float | None:
        """
        Get the best price for a token.

        Args:
            token_id: The token ID
            side: "BUY" or "SELL"

        Returns:
            Best price or None
        """
        try:
            result: Any = self.client.get_price(token_id, side=side)
            return float(result) if result is not None else None
        except Exception as e:
            logger.error("Failed to fetch price", token_id=token_id, side=side, error=str(e))
            return None

    def place_limit_order(
        self,
        token_id: str,
        side: str,
        price: float,
        size: float,
    ) -> OrderResult:
        """
        Place a limit order.

        Args:
            token_id: The token ID to trade
            side: "BUY" or "SELL"
            price: Limit price (0.0-1.0)
            size: Number of shares

        Returns:
            OrderResult with success status and details
        """
        if not self._authenticated:
            return OrderResult(
                success=False,
                order_id=None,
                message="Not authenticated",
            )

        try:
            order_args = OrderArgs(
                token_id=token_id,
                price=price,
                size=size,
                side=BUY if side.upper() == "BUY" else SELL,
            )
            signed_order = self.client.create_order(order_args)
            response: Any = self.client.post_order(signed_order, OrderType.GTC)  # type: ignore[arg-type]

            logger.info(
                "Limit order placed",
                token_id=token_id,
                side=side,
                price=price,
                size=size,
                response=response,
            )

            # Extract order ID from response
            order_id: str | None = None
            if isinstance(response, dict):
                order_id = response.get("orderID")
            elif hasattr(response, "orderID"):
                order_id = response.orderID

            return OrderResult(
                success=True,
                order_id=order_id,
                message="Order placed successfully",
            )
        except Exception as e:
            logger.error(
                "Failed to place limit order",
                token_id=token_id,
                side=side,
                price=price,
                size=size,
                error=str(e),
            )
            return OrderResult(
                success=False,
                order_id=None,
                message=str(e),
            )

    def place_market_order(
        self,
        token_id: str,
        side: str,
        amount_usd: float,
    ) -> OrderResult:
        """
        Place a market order by dollar amount.

        Args:
            token_id: The token ID to trade
            side: "BUY" or "SELL"
            amount_usd: Dollar amount to trade

        Returns:
            OrderResult with success status and details
        """
        if not self._authenticated:
            return OrderResult(
                success=False,
                order_id=None,
                message="Not authenticated",
            )

        try:
            order_args = MarketOrderArgs(
                token_id=token_id,
                amount=amount_usd,
                side=BUY if side.upper() == "BUY" else SELL,
            )
            signed_order = self.client.create_market_order(order_args)
            response: Any = self.client.post_order(signed_order, OrderType.FOK)  # type: ignore[arg-type]

            logger.info(
                "Market order placed",
                token_id=token_id,
                side=side,
                amount_usd=amount_usd,
                response=response,
            )

            # Extract order ID from response
            order_id: str | None = None
            if isinstance(response, dict):
                order_id = response.get("orderID")
            elif hasattr(response, "orderID"):
                order_id = response.orderID

            return OrderResult(
                success=True,
                order_id=order_id,
                message="Order placed successfully",
            )
        except Exception as e:
            logger.error(
                "Failed to place market order",
                token_id=token_id,
                side=side,
                amount_usd=amount_usd,
                error=str(e),
            )
            return OrderResult(
                success=False,
                order_id=None,
                message=str(e),
            )

    def cancel_order(self, order_id: str) -> bool:
        """
        Cancel an order.

        Args:
            order_id: The order ID to cancel

        Returns:
            True if cancelled successfully
        """
        if not self._authenticated:
            return False

        try:
            self.client.cancel(order_id)
            logger.info("Order cancelled", order_id=order_id)
            return True
        except Exception as e:
            logger.error("Failed to cancel order", order_id=order_id, error=str(e))
            return False

    def cancel_all_orders(self) -> bool:
        """
        Cancel all open orders.

        Returns:
            True if all orders cancelled
        """
        if not self._authenticated:
            return False

        try:
            self.client.cancel_all()
            logger.info("All orders cancelled")
            return True
        except Exception as e:
            logger.error("Failed to cancel all orders", error=str(e))
            return False

    def get_open_orders(self) -> list[dict[str, Any]]:
        """
        Get all open orders.

        Returns:
            List of open orders
        """
        if not self._authenticated:
            return []

        try:
            result: Any = self.client.get_orders(OpenOrderParams())
            return list(result) if result else []
        except Exception as e:
            logger.error("Failed to fetch open orders", error=str(e))
            return []

    def get_trades(self) -> list[dict[str, Any]]:
        """
        Get recent trades.

        Returns:
            List of trades
        """
        if not self._authenticated:
            return []

        try:
            result: Any = self.client.get_trades()
            return list(result) if result else []
        except Exception as e:
            logger.error("Failed to fetch trades", error=str(e))
            return []

    def get_balance(self) -> float:
        """
        Get USDC balance available for trading.

        Returns:
            USDC balance in dollars
        """
        if not self._authenticated:
            return 0.0

        try:
            params = BalanceAllowanceParams(asset_type=AssetType.COLLATERAL)  # type: ignore[arg-type]
            result: Any = self.client.get_balance_allowance(params)

            if result:
                # Balance is in raw units (6 decimals for USDC)
                balance_raw = float(result.get("balance", 0))
                return balance_raw / 1_000_000  # Convert to dollars
            return 0.0

        except Exception as e:
            logger.error("Failed to fetch balance", error=str(e))
            return 0.0

    def get_api_credentials(self) -> dict[str, str]:
        """
        Get API credentials for WebSocket authentication.

        Returns:
            Dict with apiKey, secret, passphrase
        """
        if not self._authenticated or not hasattr(self.client, "creds"):
            return {}

        creds = self.client.creds
        if creds:
            return {
                "apiKey": creds.api_key,
                "secret": creds.api_secret,
                "passphrase": creds.api_passphrase,
            }
        return {}
