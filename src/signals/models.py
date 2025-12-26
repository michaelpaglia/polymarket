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


class TradeDecision(BaseModel):
    """
    Structured trade decision output.

    This is the standardized format for every potential trade,
    clearly showing the edge and decision rationale.
    """

    # === TRADE IDENTIFICATION ===
    decision_id: str = ""
    timestamp: datetime = Field(default_factory=datetime.now)

    # === THE EDGE ===
    news_headline: str = ""
    news_source: str = ""
    news_published_at: Optional[datetime] = None
    time_since_news_minutes: float = 0.0  # How fresh is this info?

    # === MARKET INFO ===
    market_question: str = ""
    market_condition_id: str = ""
    current_yes_price: float = 0.0
    current_no_price: float = 0.0
    market_liquidity_usd: float = 0.0

    # === THE DECISION ===
    action: str = "HOLD"  # BUY_YES, BUY_NO, or HOLD
    position_size_usd: float = 0.0
    target_price: float = 0.0  # Price we're buying at

    # === EDGE ANALYSIS ===
    edge_summary: str = ""  # One-line summary of why this is an edge
    confidence_score: float = 0.0  # 0.0 - 1.0
    reasoning: str = ""  # Full reasoning from LLM

    # === RISK FACTORS ===
    news_freshness: str = ""  # "BREAKING", "RECENT", "STALE"
    source_credibility: str = ""  # "HIGH", "MEDIUM", "LOW"
    price_impact_estimate: float = 0.0  # Expected price move

    # === EXECUTION STATUS ===
    is_paper_trade: bool = True
    executed: bool = False
    execution_timestamp: Optional[datetime] = None
    execution_price: Optional[float] = None
    order_id: Optional[str] = None

    def to_log_dict(self) -> dict:
        """Return a clean dict for logging/display."""
        return {
            "timestamp": self.timestamp.isoformat(),
            "market": self.market_question[:50] + "..." if len(self.market_question) > 50 else self.market_question,
            "action": self.action,
            "size_usd": f"${self.position_size_usd:.2f}",
            "yes_price": f"{self.current_yes_price * 100:.1f}%",
            "no_price": f"{self.current_no_price * 100:.1f}%",
            "confidence": f"{self.confidence_score:.0%}",
            "edge": self.edge_summary,
            "news_age_min": f"{self.time_since_news_minutes:.1f}",
            "is_paper": self.is_paper_trade,
        }

    def __str__(self) -> str:
        """Pretty print the trade decision."""
        status = "[PAPER]" if self.is_paper_trade else "[LIVE]"
        action_symbol = {"BUY_YES": "[YES]", "BUY_NO": "[NO]", "HOLD": "[---]"}.get(self.action, "[?]")

        # Format prices as percentages (more intuitive for prediction markets)
        yes_pct = self.current_yes_price * 100
        no_pct = self.current_no_price * 100

        return f"""
========================================================================
| {status} TRADE DECISION
========================================================================
| NEWS: {self.news_headline[:55]}...
| Age: {self.time_since_news_minutes:.0f} min | Source: {self.news_source}
------------------------------------------------------------------------
| MARKET: {self.market_question[:55]}...
| Prices: YES={yes_pct:.1f}% | NO={no_pct:.1f}%
------------------------------------------------------------------------
| {action_symbol} ACTION: {self.action} @ ${self.position_size_usd:.2f}
| Confidence: {self.confidence_score:.0%}
| Edge: {self.edge_summary}
------------------------------------------------------------------------
| Reasoning: {self.reasoning[:60]}...
========================================================================
"""
