"""Market data models."""

from datetime import datetime, timezone
from typing import Optional

from pydantic import BaseModel, Field


class MarketOutcome(BaseModel):
    """Represents a single outcome (YES/NO) in a market."""

    token_id: str
    outcome: str  # "Yes" or "No"
    price: float = 0.0  # Current price (0.0-1.0)


class Market(BaseModel):
    """
    Represents a Polymarket prediction market.

    Contains all relevant information for trading decisions.
    """

    # Identifiers
    condition_id: str
    question_id: str = ""
    slug: str = ""

    # Market info
    question: str
    description: str = ""
    category: str = ""
    tags: list[str] = Field(default_factory=list)

    # Outcomes
    outcomes: list[MarketOutcome] = Field(default_factory=list)

    # Trading info
    liquidity: float = 0.0  # Total liquidity in USD
    volume_24h: float = 0.0  # 24h volume in USD
    volume_total: float = 0.0  # Total volume in USD

    # Timing
    end_date: Optional[datetime] = None
    created_at: Optional[datetime] = None

    # Status
    active: bool = True
    closed: bool = False

    @property
    def yes_token_id(self) -> str | None:
        """Get the YES outcome token ID."""
        for outcome in self.outcomes:
            if outcome.outcome.lower() == "yes":
                return outcome.token_id
        return None

    @property
    def no_token_id(self) -> str | None:
        """Get the NO outcome token ID."""
        for outcome in self.outcomes:
            if outcome.outcome.lower() == "no":
                return outcome.token_id
        return None

    @property
    def yes_price(self) -> float:
        """Get the YES price."""
        for outcome in self.outcomes:
            if outcome.outcome.lower() == "yes":
                return outcome.price
        return 0.0

    @property
    def no_price(self) -> float:
        """Get the NO price."""
        for outcome in self.outcomes:
            if outcome.outcome.lower() == "no":
                return outcome.price
        return 0.0

    @property
    def days_to_resolution(self) -> int | None:
        """Calculate days until market resolution."""
        if self.end_date is None:
            return None
        now = datetime.now(timezone.utc)
        delta = self.end_date - now
        return max(0, delta.days)

    @property
    def embedding_text(self) -> str:
        """Generate text for embedding/similarity search."""
        parts = [self.question]
        if self.description:
            parts.append(self.description)
        if self.category:
            parts.append(f"Category: {self.category}")
        if self.tags:
            parts.append(f"Tags: {', '.join(self.tags)}")
        return " ".join(parts)


class MarketMatch(BaseModel):
    """
    Represents a matched market for a news article.

    Contains the market and relevance scoring.
    """

    market: Market
    similarity_score: float = 0.0  # Vector similarity (0.0-1.0)
    llm_confidence: float = 0.0  # LLM-assessed relevance (0.0-1.0)
    llm_reasoning: str = ""

    @property
    def time_urgency_score(self) -> float:
        """
        Score boost for soon-ending markets.

        Markets ending sooner get higher priority since:
        1. Information has more impact
        2. Less time for market to correct
        3. Higher alpha opportunity
        """
        days = self.market.days_to_resolution
        if days is None:
            return 0.5  # Unknown expiry = neutral

        if days <= 1:
            return 1.0  # Ending today/tomorrow - maximum urgency
        elif days <= 3:
            return 0.9  # Ending in 3 days
        elif days <= 7:
            return 0.8  # Ending this week
        elif days <= 14:
            return 0.7  # Ending in 2 weeks
        elif days <= 30:
            return 0.6  # Ending this month
        else:
            return 0.4  # Long-dated - lower priority

    @property
    def combined_score(self) -> float:
        """
        Combined relevance score with time urgency.

        Prioritizes:
        1. LLM confidence (accuracy)
        2. Time urgency (soon-ending markets)
        3. Vector similarity (relevance)
        """
        base_score = (self.similarity_score * 0.2) + (self.llm_confidence * 0.5)
        urgency_boost = self.time_urgency_score * 0.3
        return base_score + urgency_boost
