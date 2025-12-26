"""
Signal Model - Formal approach to quantifying trading signals.

Goal: Estimate P(market is mispriced) given our observations.

Framework:
1. Market price P = consensus probability
2. X sentiment = alternative information source
3. Our edge = information the market hasn't priced yet

Key insight: Edge comes from TIMING and QUALITY
- Timing: We have info before market (breaking news, velocity)
- Quality: Our info is reliable (experts, high volume)

Signal Value = Information_Freshness × Information_Quality × Market_Disagreement
"""

from dataclasses import dataclass, field
from typing import Optional
from datetime import datetime, timezone
import math


@dataclass
class MarketState:
    """Current state of a prediction market."""
    condition_id: str
    question: str
    yes_price: float  # 0-1
    no_price: float   # 0-1
    liquidity_usd: float
    hours_to_resolution: Optional[float] = None

    # Price history for momentum (if available)
    price_1h_ago: Optional[float] = None  # YES price 1 hour ago
    price_24h_ago: Optional[float] = None  # YES price 24 hours ago


@dataclass
class XSignal:
    """Raw signal data from X/Twitter via Grok."""
    sentiment: str  # "bullish", "bearish", "neutral"
    velocity: float  # 0-1, discussion acceleration
    influencer_weight: float  # 0-1, expert engagement
    volume: str  # "low", "medium", "high"
    breaking_news: bool
    contrarian: bool  # Extreme sentiment (potential reversal)
    reasoning: str = ""


@dataclass
class SignalVector:
    """
    Quantified signal for trading decisions.

    All components normalized to [0, 1].

    Components:
    - freshness: How fresh is the information? (time decay, velocity)
    - quality: How reliable is the signal? (influencers, volume)
    - disagreement: How much does X differ from market? (edge opportunity)
    - urgency: How soon does the market resolve? (time value)
    - momentum: Is the market moving in our direction? (trend confirmation)
    """
    # Inputs
    market: MarketState
    x_signal: XSignal
    news_age_minutes: float = 0.0

    # Computed scores (cached)
    _freshness: Optional[float] = field(default=None, repr=False)
    _quality: Optional[float] = field(default=None, repr=False)
    _disagreement: Optional[float] = field(default=None, repr=False)
    _urgency: Optional[float] = field(default=None, repr=False)
    _momentum: Optional[float] = field(default=None, repr=False)

    @property
    def direction(self) -> str:
        """Trading direction: LONG (buy YES), SHORT (buy NO), or HOLD."""
        if self.x_signal.sentiment == "neutral":
            return "HOLD"
        if self.x_signal.sentiment == "bullish":
            # Only go long if market hasn't priced it in
            if self.market.yes_price < 0.75:
                return "LONG"
            return "HOLD"  # Already priced in
        if self.x_signal.sentiment == "bearish":
            # Only go short if market hasn't priced it in
            if self.market.yes_price > 0.25:
                return "SHORT"
            return "HOLD"  # Already priced in
        return "HOLD"

    @property
    def freshness(self) -> float:
        """
        Information freshness score [0, 1].

        Measures: How likely is this info NOT yet priced into the market?

        Components:
        - velocity: Discussion acceleration (new info spreads fast)
        - breaking_news: Detected as breaking (ahead of mainstream)
        - news_age: Older news = more likely already priced
        - base_freshness: Minimum freshness for recent news

        Formula:
        - base_freshness = decay_factor (based on news age alone)
        - velocity_boost = velocity × 0.5 (bonus for accelerating discussion)
        - breaking_boost = 0.3 if breaking news
        - freshness = base_freshness + velocity_boost + breaking_boost (capped at 1.0)
        """
        if self._freshness is not None:
            return self._freshness

        # Time decay: news gets priced in over time
        # Half-life of ~60 minutes (1 hour)
        half_life_minutes = 60.0
        decay_factor = math.exp(-0.693 * self.news_age_minutes / half_life_minutes)

        # Base freshness from age alone (recent news is somewhat fresh)
        # This ensures we don't get 0 just because velocity is 0
        base_freshness = decay_factor * 0.4  # Max 40% from age alone

        # Velocity boost (accelerating discussion = fresher info)
        velocity_boost = self.x_signal.velocity * 0.4  # Max 40% from velocity

        # Breaking news boost
        breaking_boost = 0.2 if self.x_signal.breaking_news else 0.0

        # Combine (capped at 1.0)
        self._freshness = min(1.0, base_freshness + velocity_boost + breaking_boost)
        return self._freshness

    @property
    def quality(self) -> float:
        """
        Information quality score [0, 1].

        Measures: How reliable is this signal?

        Components:
        - influencer_weight: Experts > random users
        - volume: More discussion = larger sample size
        - not contrarian: Extreme sentiment often reverses

        Formula: quality = (influencer + volume) / 2 × contrarian_penalty
        """
        if self._quality is not None:
            return self._quality

        # Influencer weight (0-1)
        influencer = self.x_signal.influencer_weight

        # Volume score
        volume_map = {"low": 0.2, "medium": 0.5, "high": 1.0}
        volume = volume_map.get(self.x_signal.volume, 0.2)

        # Contrarian penalty: extreme sentiment often reverses
        contrarian_penalty = 0.5 if self.x_signal.contrarian else 1.0

        # Combine
        self._quality = ((influencer + volume) / 2) * contrarian_penalty
        return self._quality

    @property
    def disagreement(self) -> float:
        """
        Market disagreement score [0, 1].

        Measures: How much does X sentiment differ from market price?

        If X is bullish and market is at 20%, high disagreement.
        If X is bullish and market is at 80%, low disagreement (already priced).

        Formula: |X_implied_prob - market_price|

        We assume X sentiment implies:
        - bullish → ~70% YES probability
        - bearish → ~30% YES probability
        """
        if self._disagreement is not None:
            return self._disagreement

        market_price = self.market.yes_price

        # X-implied probability (conservative estimates)
        if self.x_signal.sentiment == "bullish":
            x_implied = 0.65  # X thinks YES is more likely
        elif self.x_signal.sentiment == "bearish":
            x_implied = 0.35  # X thinks NO is more likely
        else:
            x_implied = 0.50  # Neutral

        # Disagreement = absolute difference
        self._disagreement = abs(x_implied - market_price)
        return self._disagreement

    @property
    def urgency(self) -> float:
        """
        Time urgency score [0, 1].

        Measures: How soon does the market resolve?

        Markets resolving soon have higher signal value because:
        - Less time for market to correct
        - News has more immediate impact
        - Higher alpha opportunity (time decay in our favor)

        Formula (exponential decay in reverse):
        - Resolution in <24h: 1.0
        - Resolution in 3 days: 0.8
        - Resolution in 7 days: 0.6
        - Resolution in 30 days: 0.3
        - Resolution in 90+ days: 0.1
        """
        if self._urgency is not None:
            return self._urgency

        hours = self.market.hours_to_resolution
        if hours is None:
            self._urgency = 0.3  # Unknown resolution = low urgency
            return self._urgency

        days = hours / 24.0

        if days <= 1:
            self._urgency = 1.0
        elif days <= 3:
            self._urgency = 0.85
        elif days <= 7:
            self._urgency = 0.7
        elif days <= 14:
            self._urgency = 0.55
        elif days <= 30:
            self._urgency = 0.4
        elif days <= 90:
            self._urgency = 0.25
        else:
            self._urgency = 0.1

        return self._urgency

    @property
    def momentum(self) -> float:
        """
        Momentum alignment score [0, 1].

        Measures: Is the market already moving in our direction?

        Components:
        - If bullish signal and price rising: momentum confirms (positive)
        - If bullish signal and price falling: momentum conflicts (negative)
        - If no price history: neutral (0.5)

        Higher momentum = market is confirming our thesis.
        But: Very high momentum may mean we're late.

        Returns:
        - 1.0: Strong confirmation (market moving our way)
        - 0.5: Neutral (no price history or stable)
        - 0.0: Strong conflict (market moving against us)
        """
        if self._momentum is not None:
            return self._momentum

        # Need price history
        current_price = self.market.yes_price
        price_1h = self.market.price_1h_ago
        price_24h = self.market.price_24h_ago

        if price_1h is None and price_24h is None:
            self._momentum = 0.5  # No history = neutral
            return self._momentum

        # Calculate price change
        if price_1h is not None:
            price_change = current_price - price_1h
        elif price_24h is not None:
            price_change = current_price - price_24h
        else:
            price_change = 0.0

        # Determine if momentum aligns with our signal
        if self.x_signal.sentiment == "bullish":
            # Bullish: want price to be rising
            alignment = price_change
        elif self.x_signal.sentiment == "bearish":
            # Bearish: want price to be falling
            alignment = -price_change
        else:
            alignment = 0.0

        # Convert to 0-1 scale
        # +0.1 change → 1.0, -0.1 change → 0.0, 0 → 0.5
        self._momentum = min(1.0, max(0.0, 0.5 + alignment * 5.0))
        return self._momentum

    @property
    def signal_score(self) -> float:
        """
        Combined signal score [0, 1].

        Higher = stronger signal to trade.

        Formula: Weighted combination prioritizing disagreement (the actual edge).

        Components (weighted):
        - disagreement: 50% weight (this IS the edge - X vs market)
        - quality: 25% weight (signal reliability)
        - freshness: 25% weight (timing advantage)

        Temporal multipliers:
        - urgency: 0.7-1.3x (soon-resolving markets get boost)
        - momentum: 0.85-1.15x (confirmation from price movement)

        Interpretation:
        - 0.0-0.10: Weak signal, don't trade
        - 0.10-0.20: Moderate signal, 25% position
        - 0.20-0.35: Good signal, 50% position
        - 0.35-0.50: Strong signal, 75% position
        - 0.50+: Very strong signal, full position
        """
        # Weighted combination (disagreement is the edge, others are modifiers)
        # This is more forgiving than pure multiplication
        base_score = (
            self.disagreement * 0.50 +  # The edge opportunity
            self.quality * 0.25 +        # Signal reliability
            self.freshness * 0.25        # Timing advantage
        )

        # Urgency multiplier: soon-resolving markets get boost
        # urgency 1.0 → 1.3x, urgency 0.1 → 0.7x
        urgency_mult = 0.6 + self.urgency * 0.7

        # Momentum multiplier: confirmation boosts, conflict reduces
        # momentum 1.0 → 1.15x, momentum 0.5 → 1.0x, momentum 0.0 → 0.85x
        momentum_mult = 0.85 + self.momentum * 0.3

        # Combined score
        scaled = min(1.0, base_score * urgency_mult * momentum_mult)

        return scaled

    @property
    def position_size_pct(self) -> float:
        """
        Recommended position size as % of max position.

        Based on signal score (quasi-Kelly):
        - Stronger signal = larger position
        - But capped to manage risk
        """
        score = self.signal_score

        if score < 0.10:
            return 0.0  # Don't trade
        elif score < 0.20:
            return 0.25  # 25% position
        elif score < 0.35:
            return 0.50  # 50% position
        elif score < 0.50:
            return 0.75  # 75% position
        else:
            return 1.0  # Full position

    @property
    def should_trade(self) -> bool:
        """Whether signal meets minimum threshold."""
        return (
            self.direction != "HOLD" and
            self.signal_score >= 0.10 and
            self.quality >= 0.15  # Need some quality
        )

    def to_dict(self) -> dict:
        """Export signal data for logging/analysis."""
        return {
            "market_id": self.market.condition_id,
            "market_question": self.market.question[:50],
            "market_price": self.market.yes_price,
            "direction": self.direction,
            "x_sentiment": self.x_signal.sentiment,
            # Information quality components
            "freshness": round(self.freshness, 3),
            "quality": round(self.quality, 3),
            "disagreement": round(self.disagreement, 3),
            # Temporal components
            "urgency": round(self.urgency, 3),
            "momentum": round(self.momentum, 3),
            "hours_to_resolution": self.market.hours_to_resolution,
            # Final score
            "signal_score": round(self.signal_score, 3),
            "position_pct": self.position_size_pct,
            "should_trade": self.should_trade,
            # Raw inputs
            "breaking": self.x_signal.breaking_news,
            "velocity": self.x_signal.velocity,
            "influencer": self.x_signal.influencer_weight,
            "volume": self.x_signal.volume,
        }

    def __str__(self) -> str:
        hours = self.market.hours_to_resolution
        time_str = f"{hours:.0f}h" if hours else "?"
        return (
            f"{self.direction} | "
            f"Score={self.signal_score:.1%} "
            f"[F={self.freshness:.0%} Q={self.quality:.0%} D={self.disagreement:.0%}] "
            f"[U={self.urgency:.0%} M={self.momentum:.0%} T={time_str}] | "
            f"Size={self.position_size_pct:.0%}"
        )


def build_signal(
    market: MarketState,
    x_signal: XSignal,
    news_age_minutes: float = 0.0,
) -> SignalVector:
    """Build a signal vector from market state and X data."""
    return SignalVector(
        market=market,
        x_signal=x_signal,
        news_age_minutes=news_age_minutes,
    )


# Example usage
if __name__ == "__main__":
    # Test case: Breaking news, bullish sentiment, low market price
    market = MarketState(
        condition_id="test123",
        question="Will X happen in 2025?",
        yes_price=0.25,
        no_price=0.75,
        liquidity_usd=10000,
    )

    x = XSignal(
        sentiment="bullish",
        velocity=0.8,  # High velocity
        influencer_weight=0.6,
        volume="high",
        breaking_news=True,
        contrarian=False,
    )

    signal = build_signal(market, x, news_age_minutes=5)

    print("Signal Analysis:")
    print(f"  {signal}")
    print(f"  Freshness: {signal.freshness:.1%} (velocity={x.velocity}, breaking={x.breaking_news})")
    print(f"  Quality: {signal.quality:.1%} (influencer={x.influencer_weight}, volume={x.volume})")
    print(f"  Disagreement: {signal.disagreement:.1%} (X=bullish, market={market.yes_price:.0%})")
    print(f"  Should trade: {signal.should_trade}")
