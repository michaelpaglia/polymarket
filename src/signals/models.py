"""Signal data models."""

from datetime import datetime
from enum import Enum
from typing import Optional

from pydantic import BaseModel, Field


class SignalDirection(str, Enum):
    """Trading signal direction."""

    YES = "YES"
    NO = "NO"
    HOLD = "HOLD"  # No action


class TradingSignal(BaseModel):
    """
    Represents a trading signal generated from news analysis.

    Contains all information needed to make a trading decision.
    """

    # Identifiers
    signal_id: str = Field(default="")
    article_id: str = ""  # Source article ID
    market_id: str = ""  # Polymarket condition ID

    # Signal
    direction: SignalDirection = SignalDirection.HOLD
    confidence: float = 0.0  # 0.0 - 1.0
    reasoning: str = ""

    # Market info (for convenience)
    market_question: str = ""
    current_yes_price: float = 0.0
    current_no_price: float = 0.0

    # Suggested action
    suggested_size_usd: float = 0.0
    target_token_id: str = ""  # Token to buy

    # Timing
    generated_at: datetime = Field(default_factory=datetime.now)
    article_published_at: Optional[datetime] = None

    # Validation status
    is_valid: bool = True
    validation_errors: list[str] = Field(default_factory=list)

    @property
    def is_actionable(self) -> bool:
        """Check if this signal should result in a trade."""
        return (
            self.is_valid
            and self.direction != SignalDirection.HOLD
            and self.confidence > 0.5
            and self.suggested_size_usd > 0
        )

    @property
    def age_seconds(self) -> float:
        """Signal age in seconds."""
        delta = datetime.now() - self.generated_at
        return delta.total_seconds()


class SignalAnalysisResult(BaseModel):
    """Result of analyzing a news article against matched markets."""

    article_id: str
    article_title: str
    signals: list[TradingSignal] = Field(default_factory=list)
    analysis_time_ms: float = 0.0
    error: Optional[str] = None

    @property
    def has_actionable_signals(self) -> bool:
        """Check if any signals are actionable."""
        return any(s.is_actionable for s in self.signals)

    @property
    def actionable_signals(self) -> list[TradingSignal]:
        """Get only actionable signals."""
        return [s for s in self.signals if s.is_actionable]
