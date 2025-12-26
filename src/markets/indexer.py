"""Market indexer for fetching and indexing Polymarket markets."""

from datetime import datetime
from typing import Optional

import httpx

from src.config import MarketSettings
from src.markets.models import Market, MarketOutcome
from src.utils.logging import get_logger

logger = get_logger(__name__)

# Polymarket Gamma API endpoints
GAMMA_API_BASE = "https://gamma-api.polymarket.com"
CLOB_API_BASE = "https://clob.polymarket.com"


class MarketIndexer:
    """
    Fetches and indexes all active Polymarket markets.

    Uses the Gamma API for market metadata and CLOB API for trading data.
    """

    def __init__(self, settings: MarketSettings) -> None:
        """
        Initialize the market indexer.

        Args:
            settings: Market configuration settings
        """
        self.settings = settings
        self._markets: dict[str, Market] = {}
        self._last_refresh: Optional[datetime] = None

    @property
    def markets(self) -> list[Market]:
        """Get all indexed markets."""
        return list(self._markets.values())

    async def fetch_markets(self) -> list[Market]:
        """
        Fetch all active markets from Polymarket.

        Returns:
            List of Market objects
        """
        markets = []

        try:
            async with httpx.AsyncClient(timeout=30.0) as client:
                # Fetch markets from Gamma API
                response = await client.get(
                    f"{GAMMA_API_BASE}/markets",
                    params={
                        "closed": "false",
                        "limit": 500,  # Fetch up to 500 markets
                    },
                )
                response.raise_for_status()
                data = response.json()

                for market_data in data:
                    market = self._parse_market(market_data)
                    if market and self._should_include(market):
                        markets.append(market)

                logger.info(f"Fetched {len(markets)} active markets from Polymarket")

        except httpx.HTTPError as e:
            logger.error("Failed to fetch markets from Gamma API", error=str(e))
        except Exception as e:
            logger.error("Unexpected error fetching markets", error=str(e))

        return markets

    def _parse_market(self, data: dict) -> Optional[Market]:
        """
        Parse raw market data into a Market object.

        Args:
            data: Raw market data from API

        Returns:
            Market object or None if parsing fails
        """
        try:
            # Parse outcomes
            outcomes = []
            tokens = data.get("tokens", [])
            for token in tokens:
                outcomes.append(
                    MarketOutcome(
                        token_id=token.get("token_id", ""),
                        outcome=token.get("outcome", ""),
                        price=float(token.get("price", 0.0)),
                    )
                )

            # Parse end date
            end_date = None
            end_date_str = data.get("end_date_iso")
            if end_date_str:
                try:
                    end_date = datetime.fromisoformat(end_date_str.replace("Z", "+00:00"))
                except ValueError:
                    pass

            # Parse tags
            tags = data.get("tags", [])
            if isinstance(tags, str):
                tags = [tags]

            return Market(
                condition_id=data.get("condition_id", ""),
                question_id=data.get("question_id", ""),
                slug=data.get("slug", ""),
                question=data.get("question", ""),
                description=data.get("description", ""),
                category=data.get("category", ""),
                tags=tags,
                outcomes=outcomes,
                liquidity=float(data.get("liquidity", 0)),
                volume_24h=float(data.get("volume_24hr", 0)),
                volume_total=float(data.get("volume", 0)),
                end_date=end_date,
                active=data.get("active", True),
                closed=data.get("closed", False),
            )
        except Exception as e:
            logger.warning("Failed to parse market", error=str(e), data=data)
            return None

    def _should_include(self, market: Market) -> bool:
        """
        Check if market should be included based on filters.

        Args:
            market: Market to check

        Returns:
            True if market should be included
        """
        # Must be active and not closed
        if not market.active or market.closed:
            return False

        # Must have sufficient liquidity
        if market.liquidity < self.settings.min_liquidity_usd:
            return False

        # Must resolve within max days
        if market.days_to_resolution is not None:
            if market.days_to_resolution > self.settings.max_days_to_resolution:
                return False

        # Must have valid outcomes
        if not market.outcomes:
            return False

        return True

    async def refresh(self) -> None:
        """Refresh the market index."""
        markets = await self.fetch_markets()

        # Update index
        self._markets = {m.condition_id: m for m in markets}
        self._last_refresh = datetime.now()

        logger.info(
            "Market index refreshed",
            market_count=len(self._markets),
            last_refresh=self._last_refresh.isoformat(),
        )

    def get_market(self, condition_id: str) -> Optional[Market]:
        """
        Get a market by condition ID.

        Args:
            condition_id: Market condition ID

        Returns:
            Market or None
        """
        return self._markets.get(condition_id)

    def get_markets_by_category(self, category: str) -> list[Market]:
        """
        Get markets by category.

        Args:
            category: Category name

        Returns:
            List of markets in that category
        """
        return [m for m in self.markets if m.category.lower() == category.lower()]

    def needs_refresh(self) -> bool:
        """Check if the index needs refreshing."""
        if self._last_refresh is None:
            return True

        elapsed = (datetime.now() - self._last_refresh).total_seconds()
        refresh_seconds = self.settings.refresh_interval_minutes * 60
        return elapsed >= refresh_seconds
