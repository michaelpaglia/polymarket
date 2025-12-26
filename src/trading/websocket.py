"""WebSocket client for real-time Polymarket data."""

import asyncio
import json
import threading
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Any, Callable, Optional

import websocket

from src.utils.logging import get_logger

logger = get_logger(__name__)

WS_URL = "wss://ws-subscriptions-clob.polymarket.com"


@dataclass
class PriceUpdate:
    """Real-time price update from WebSocket."""

    token_id: str
    best_bid: float
    best_ask: float
    midpoint: float
    timestamp: datetime = field(default_factory=lambda: datetime.now(timezone.utc))


@dataclass
class OrderUpdate:
    """Real-time order status update."""

    order_id: str
    status: str  # MATCHED, MINED, CONFIRMED, FAILED
    filled_amount: float = 0.0
    timestamp: datetime = field(default_factory=lambda: datetime.now(timezone.utc))


class PolymarketWebSocket:
    """
    WebSocket client for real-time Polymarket data.

    Provides:
    - Real-time orderbook/price updates (market channel)
    - Order status notifications (user channel)
    """

    def __init__(
        self,
        api_key: str = "",
        api_secret: str = "",
        api_passphrase: str = "",
        on_price_update: Optional[Callable[[PriceUpdate], None]] = None,
        on_order_update: Optional[Callable[[OrderUpdate], None]] = None,
    ) -> None:
        """
        Initialize WebSocket client.

        Args:
            api_key: API key for authenticated channels
            api_secret: API secret
            api_passphrase: API passphrase
            on_price_update: Callback for price updates
            on_order_update: Callback for order updates
        """
        self.api_key = api_key
        self.api_secret = api_secret
        self.api_passphrase = api_passphrase
        self.on_price_update = on_price_update
        self.on_order_update = on_order_update

        self._market_ws: Optional[websocket.WebSocketApp] = None
        self._user_ws: Optional[websocket.WebSocketApp] = None
        self._running = False
        self._subscribed_tokens: set[str] = set()
        self._subscribed_markets: set[str] = set()

        # Price cache
        self.prices: dict[str, PriceUpdate] = {}

    def connect_market_channel(self, token_ids: list[str]) -> None:
        """
        Connect to market channel for price updates.

        Args:
            token_ids: List of token IDs to subscribe to
        """
        if self._market_ws is not None:
            logger.warning("Market channel already connected")
            return

        self._subscribed_tokens = set(token_ids)
        url = f"{WS_URL}/ws/market"

        self._market_ws = websocket.WebSocketApp(
            url,
            on_open=self._on_market_open,
            on_message=self._on_market_message,
            on_error=self._on_error,
            on_close=self._on_close,
        )

        # Run in background thread
        thread = threading.Thread(target=self._run_market_ws, daemon=True)
        thread.start()

        logger.info(f"Connecting to market channel with {len(token_ids)} tokens")

    def connect_user_channel(self, market_ids: list[str]) -> None:
        """
        Connect to user channel for order updates.

        Requires authentication.

        Args:
            market_ids: List of market condition IDs to monitor
        """
        if not self.api_key:
            logger.warning("Cannot connect to user channel without API credentials")
            return

        if self._user_ws is not None:
            logger.warning("User channel already connected")
            return

        self._subscribed_markets = set(market_ids)
        url = f"{WS_URL}/ws/user"

        self._user_ws = websocket.WebSocketApp(
            url,
            on_open=self._on_user_open,
            on_message=self._on_user_message,
            on_error=self._on_error,
            on_close=self._on_close,
        )

        thread = threading.Thread(target=self._run_user_ws, daemon=True)
        thread.start()

        logger.info(f"Connecting to user channel with {len(market_ids)} markets")

    def subscribe_tokens(self, token_ids: list[str]) -> None:
        """Subscribe to additional tokens."""
        if self._market_ws is None:
            self.connect_market_channel(token_ids)
            return

        new_tokens = [t for t in token_ids if t not in self._subscribed_tokens]
        if new_tokens:
            self._subscribed_tokens.update(new_tokens)
            msg = json.dumps({
                "assets_ids": new_tokens,
                "operation": "subscribe",
            })
            self._market_ws.send(msg)
            logger.info(f"Subscribed to {len(new_tokens)} new tokens")

    def unsubscribe_tokens(self, token_ids: list[str]) -> None:
        """Unsubscribe from tokens."""
        if self._market_ws is None:
            return

        self._subscribed_tokens -= set(token_ids)
        msg = json.dumps({
            "assets_ids": token_ids,
            "operation": "unsubscribe",
        })
        self._market_ws.send(msg)

    def get_price(self, token_id: str) -> Optional[PriceUpdate]:
        """Get cached price for token."""
        return self.prices.get(token_id)

    def disconnect(self) -> None:
        """Disconnect all WebSocket connections."""
        self._running = False

        if self._market_ws:
            self._market_ws.close()
            self._market_ws = None

        if self._user_ws:
            self._user_ws.close()
            self._user_ws = None

        logger.info("WebSocket connections closed")

    def _run_market_ws(self) -> None:
        """Run market WebSocket in background."""
        self._running = True
        while self._running and self._market_ws:
            try:
                self._market_ws.run_forever(ping_interval=30, ping_timeout=10)
            except Exception as e:
                logger.error(f"Market WebSocket error: {e}")
                if self._running:
                    asyncio.get_event_loop().call_later(5, self._reconnect_market)
                break

    def _run_user_ws(self) -> None:
        """Run user WebSocket in background."""
        self._running = True
        while self._running and self._user_ws:
            try:
                self._user_ws.run_forever(ping_interval=30, ping_timeout=10)
            except Exception as e:
                logger.error(f"User WebSocket error: {e}")
                break

    def _reconnect_market(self) -> None:
        """Reconnect to market channel."""
        if self._running:
            tokens = list(self._subscribed_tokens)
            self._market_ws = None
            self.connect_market_channel(tokens)

    def _on_market_open(self, ws: Any) -> None:
        """Handle market channel connection."""
        logger.info("Market WebSocket connected")

        # Subscribe to tokens
        if self._subscribed_tokens:
            msg = json.dumps({
                "assets_ids": list(self._subscribed_tokens),
                "type": "market",
            })
            ws.send(msg)

        # Start ping thread
        def ping() -> None:
            while self._running and self._market_ws:
                try:
                    ws.send("PING")
                except Exception:
                    break
                threading.Event().wait(10)

        threading.Thread(target=ping, daemon=True).start()

    def _on_user_open(self, ws: Any) -> None:
        """Handle user channel connection."""
        logger.info("User WebSocket connected")

        # Authenticate and subscribe
        msg = json.dumps({
            "markets": list(self._subscribed_markets),
            "type": "user",
            "auth": {
                "apiKey": self.api_key,
                "secret": self.api_secret,
                "passphrase": self.api_passphrase,
            },
        })
        ws.send(msg)

    def _on_market_message(self, ws: Any, message: str) -> None:
        """Handle market channel message."""
        if message == "PONG":
            return

        try:
            data = json.loads(message)

            # Parse orderbook update
            if "market" in data or "asset_id" in data:
                token_id = data.get("asset_id") or data.get("market")

                # Extract best bid/ask
                bids = data.get("bids", [])
                asks = data.get("asks", [])

                best_bid = float(bids[0]["price"]) if bids else 0.0
                best_ask = float(asks[0]["price"]) if asks else 1.0
                midpoint = (best_bid + best_ask) / 2

                update = PriceUpdate(
                    token_id=token_id,
                    best_bid=best_bid,
                    best_ask=best_ask,
                    midpoint=midpoint,
                )

                self.prices[token_id] = update

                if self.on_price_update:
                    self.on_price_update(update)

        except Exception as e:
            logger.debug(f"Failed to parse market message: {e}")

    def _on_user_message(self, ws: Any, message: str) -> None:
        """Handle user channel message."""
        try:
            data = json.loads(message)

            # Parse order update
            if "order" in data or "orderID" in data:
                order_id = data.get("orderID") or data.get("order", {}).get("id")
                status = data.get("status", "UNKNOWN")
                filled = float(data.get("filledAmount", 0))

                update = OrderUpdate(
                    order_id=order_id,
                    status=status,
                    filled_amount=filled,
                )

                logger.info(f"Order update: {order_id} -> {status}")

                if self.on_order_update:
                    self.on_order_update(update)

        except Exception as e:
            logger.debug(f"Failed to parse user message: {e}")

    def _on_error(self, ws: Any, error: Any) -> None:
        """Handle WebSocket error."""
        logger.error(f"WebSocket error: {error}")

    def _on_close(self, ws: Any, close_status: Any, close_msg: Any) -> None:
        """Handle WebSocket close."""
        logger.info(f"WebSocket closed: {close_status} - {close_msg}")
