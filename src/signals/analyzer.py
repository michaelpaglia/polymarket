"""Signal analyzer using LLM to generate trading signals from news."""

import json
import time
import uuid
from typing import Any, Optional

from google import genai  # New package
from google.genai import types

from src.config import LLMSettings, RiskSettings, SignalSettings
from src.markets.models import MarketMatch
from src.news.models import NewsArticle
from src.signals.models import (
    SignalAnalysisResult,
    SignalDirection,
    TradingSignal,
)
from src.signals.sentiment import XSentimentAnalyzer, SentimentAnalysis
from src.utils.logging import get_logger

logger = get_logger(__name__)


# Prompt for signal generation
SIGNAL_PROMPT = """You are a prediction market analyst. Analyze this news and determine how it affects the probability of each market outcome.

NEWS ARTICLE:
Title: {title}
Published: {published_at}
Description: {description}
Content: {content}

MATCHED MARKETS:
{markets}

For each market, analyze:
1. Does this news make the YES outcome more or less likely?
2. How much does it shift the probability?
3. What is your confidence in this assessment?

Consider:
- Is this new information or already priced in?
- How directly does this news impact the market outcome?
- Are there any caveats or uncertainties?

Respond in JSON format:
{{
    "signals": [
        {{
            "market_id": "condition_id here",
            "direction": "YES" or "NO" or "HOLD",
            "confidence": 0.0-1.0,
            "reasoning": "Brief explanation of why this news impacts the market"
        }}
    ]
}}

Guidelines:
- Use "HOLD" if the news doesn't significantly impact the market
- Only use high confidence (>0.7) if the impact is clear and direct
- Consider if the news is from a reliable source
- Be conservative - it's better to miss a trade than make a bad one"""


class SignalAnalyzer:
    """
    Analyzes news articles and generates trading signals.

    Uses Gemini to understand news impact on prediction markets.
    Combines with X/Twitter sentiment for edge detection.
    """

    def __init__(
        self,
        llm_settings: LLMSettings,
        signal_settings: SignalSettings,
        risk_settings: RiskSettings,
        grok_api_key: str = "",
    ) -> None:
        """
        Initialize the signal analyzer.

        Args:
            llm_settings: LLM configuration
            signal_settings: Signal generation settings
            risk_settings: Risk management settings
            grok_api_key: Optional Grok API key for X sentiment
        """
        self.llm_settings = llm_settings
        self.signal_settings = signal_settings
        self.risk_settings = risk_settings
        self._client: genai.Client | None = None
        self._sentiment_analyzer: Optional[XSentimentAnalyzer] = None

        # Configure Gemini
        if llm_settings.google_api_key:
            self._client = genai.Client(api_key=llm_settings.google_api_key)
            logger.info(f"Signal analyzer initialized with {llm_settings.model}")

        # Configure X sentiment analyzer
        if grok_api_key:
            self._sentiment_analyzer = XSentimentAnalyzer(grok_api_key)
            logger.info("X sentiment analyzer enabled for edge detection")

    async def analyze(
        self,
        article: NewsArticle,
        matches: list[MarketMatch],
    ) -> SignalAnalysisResult:
        """
        Analyze a news article and generate trading signals.

        Args:
            article: News article to analyze
            matches: Matched markets from market matcher

        Returns:
            Signal analysis result with trading signals
        """
        start_time = time.time()

        if not self._client:
            return SignalAnalysisResult(
                article_id=article.id,
                article_title=article.title,
                error="LLM not configured",
            )

        if not matches:
            return SignalAnalysisResult(
                article_id=article.id,
                article_title=article.title,
                signals=[],
            )

        try:
            # Generate signals using LLM (news-based)
            signals = await self._generate_signals(article, matches)

            # Enhance signals with X sentiment edge detection (parallel)
            if self._sentiment_analyzer:
                signals = await self._enhance_with_sentiment(signals, matches, article)

            # Calculate position sizes (scaled by edge size)
            for signal in signals:
                signal.suggested_size_usd = self._calculate_position_size(signal)

            # Filter by confidence threshold
            signals = [
                s for s in signals
                if s.confidence >= self.signal_settings.confidence_threshold
            ]

            analysis_time = (time.time() - start_time) * 1000

            return SignalAnalysisResult(
                article_id=article.id,
                article_title=article.title,
                signals=signals,
                analysis_time_ms=analysis_time,
            )

        except Exception as e:
            logger.error("Signal analysis failed", error=str(e), article=article.title)
            return SignalAnalysisResult(
                article_id=article.id,
                article_title=article.title,
                error=str(e),
            )

    async def _generate_signals(
        self,
        article: NewsArticle,
        matches: list[MarketMatch],
    ) -> list[TradingSignal]:
        """
        Generate trading signals using LLM.

        Args:
            article: News article
            matches: Matched markets

        Returns:
            List of trading signals
        """
        # Format markets for prompt
        markets_text = []
        market_lookup: dict[str, MarketMatch] = {}

        for match in matches:
            market = match.market
            markets_text.append(
                f"- [{market.condition_id}] {market.question}\n"
                f"  Current prices: YES={market.yes_price:.2f}, NO={market.no_price:.2f}\n"
                f"  Match confidence: {match.llm_confidence:.2f}"
            )
            market_lookup[market.condition_id] = match

        # Create prompt
        prompt = SIGNAL_PROMPT.format(
            title=article.title,
            published_at=article.published_at.isoformat() if article.published_at else "Unknown",
            description=article.description,
            content=article.content[:1000] if article.content else "",
            markets="\n".join(markets_text),
        )

        # Call Gemini 3
        assert self._client is not None, "Client should be initialized"
        response = self._client.models.generate_content(
            model=self.llm_settings.model,
            contents=prompt,
            config=types.GenerateContentConfig(
                temperature=0.2,
                response_mime_type="application/json",
            ),
        )

        # Parse response
        response_text = response.text or "{}"
        result: dict[str, Any] = json.loads(response_text)
        signals = []

        for signal_data in result.get("signals", []):
            market_id = signal_data.get("market_id")
            match = market_lookup.get(market_id)

            if not match:
                continue

            direction_str = signal_data.get("direction", "HOLD").upper()
            try:
                direction = SignalDirection(direction_str)
            except ValueError:
                direction = SignalDirection.HOLD

            # Determine target token
            target_token_id = ""
            if direction == SignalDirection.YES:
                target_token_id = match.market.yes_token_id or ""
            elif direction == SignalDirection.NO:
                target_token_id = match.market.no_token_id or ""

            signal = TradingSignal(
                signal_id=str(uuid.uuid4()),
                article_id=article.id,
                market_id=market_id,
                direction=direction,
                confidence=signal_data.get("confidence", 0.0),
                reasoning=signal_data.get("reasoning", ""),
                market_question=match.market.question,
                current_yes_price=match.market.yes_price,
                current_no_price=match.market.no_price,
                target_token_id=target_token_id,
                article_published_at=article.published_at,
            )

            signals.append(signal)

        return signals

    def _calculate_position_size(self, signal: TradingSignal) -> float:
        """
        Calculate position size based on confidence and risk settings.

        Args:
            signal: Trading signal

        Returns:
            Suggested position size in USD
        """
        if signal.direction == SignalDirection.HOLD:
            return 0.0

        # Base size from confidence
        # Higher confidence = larger position (linear scaling)
        base_size = self.risk_settings.max_position_per_market_usd * signal.confidence

        # Cap at maximum
        position_size = min(base_size, self.risk_settings.max_position_per_market_usd)

        # Minimum viable trade
        if position_size < 1.0:
            position_size = 0.0

        return round(position_size, 2)

    async def _enhance_with_sentiment(
        self,
        signals: list[TradingSignal],
        matches: list[MarketMatch],
        article: NewsArticle,
    ) -> list[TradingSignal]:
        """
        Enhance signals with X/Twitter sentiment edge detection.

        THE EDGE: Find discrepancy between X sentiment and market price.
        If Twitter thinks probability is 70% but market is at 50%, that's a 20% edge.

        Args:
            signals: Initial signals from news analysis
            matches: Matched markets
            article: The news article

        Returns:
            Enhanced signals with sentiment edge incorporated
        """
        if not self._sentiment_analyzer:
            return signals

        # Build lookup for signals by market_id
        signal_lookup = {s.market_id: s for s in signals}

        # Prepare sentiment queries (market question + news context)
        sentiment_queries = []
        for match in matches:
            market = match.market
            sentiment_queries.append((
                market.question,
                f"News: {article.title}"
            ))

        # Get sentiment for all markets in parallel
        try:
            sentiments = await self._sentiment_analyzer.analyze_multiple(sentiment_queries)
        except Exception as e:
            logger.warning("Sentiment analysis failed, using news signals only", error=str(e))
            return signals

        # Enhance signals with sentiment edge
        enhanced_signals = []
        for i, match in enumerate(matches):
            market = match.market
            sentiment = sentiments[i] if i < len(sentiments) else None

            # Get existing signal or create new one
            signal = signal_lookup.get(market.condition_id)

            if sentiment and sentiment.confidence >= 0.5:
                # REALISTIC EDGE CALCULATION
                # Don't compare X "probability" to market price directly - that's not rigorous
                # Instead: use X as a DIRECTIONAL signal with small edge assumptions

                market_price = market.yes_price
                x_direction = sentiment.sentiment  # "bullish", "bearish", "neutral"

                # Estimate realistic edge based on signal quality
                # Base edge: 3-5% for typical signal, up to 10% for high-quality
                base_edge = 0.03  # 3% base edge

                # Quality multipliers (conservative)
                if sentiment.breaking_news_detected:
                    base_edge = 0.08  # 8% edge for breaking news
                elif sentiment.velocity_score >= 0.7:
                    base_edge = 0.06  # 6% edge for velocity spike
                elif sentiment.influencer_weight >= 0.7:
                    base_edge = 0.05  # 5% edge for influencer activity
                elif sentiment.discussion_volume == "high":
                    base_edge = 0.04  # 4% edge for high volume

                # Determine direction
                if x_direction == "bullish" and market_price < 0.5:
                    # X bullish, market low - potential upside
                    direction = SignalDirection.YES
                    realistic_edge = base_edge
                elif x_direction == "bearish" and market_price > 0.5:
                    # X bearish, market high - potential downside
                    direction = SignalDirection.NO
                    realistic_edge = base_edge
                elif x_direction == "bullish" and market_price > 0.8:
                    # X bullish but market already priced in - skip
                    direction = SignalDirection.HOLD
                    realistic_edge = 0
                elif x_direction == "bearish" and market_price < 0.2:
                    # X bearish but market already priced in - skip
                    direction = SignalDirection.HOLD
                    realistic_edge = 0
                else:
                    direction = SignalDirection.HOLD
                    realistic_edge = 0

                # Log with realistic numbers
                logger.info(
                    f"X signal: {x_direction} (confidence={sentiment.confidence:.0%}) | "
                    f"Market={market_price:.1%} | Edge={realistic_edge:.0%}",
                    market=market.question[:50],
                    breaking=sentiment.breaking_news_detected,
                    velocity=sentiment.velocity_score,
                )

                if signal and direction != SignalDirection.HOLD:
                    # Enhance existing signal if X confirms direction
                    if signal.direction == direction:
                        signal.confidence = min(0.95, signal.confidence + realistic_edge)
                        signal.reasoning += f" [X {x_direction}: +{realistic_edge:.0%} edge]"
                    enhanced_signals.append(signal)

                elif direction != SignalDirection.HOLD and realistic_edge >= 0.03:
                    # Create signal from X sentiment (only if we have edge)
                    target_token_id = market.yes_token_id if direction == SignalDirection.YES else market.no_token_id

                    new_signal = TradingSignal(
                        signal_id=str(uuid.uuid4()),
                        article_id=article.id,
                        market_id=market.condition_id,
                        direction=direction,
                        confidence=0.6 + realistic_edge,  # Conservative confidence
                        reasoning=f"[X {x_direction.upper()}] {realistic_edge:.0%} edge. "
                                  f"{'BREAKING ' if sentiment.breaking_news_detected else ''}"
                                  f"{'VELOCITY ' if sentiment.velocity_score >= 0.7 else ''}"
                                  f"{sentiment.reasoning[:80]}",
                        market_question=market.question,
                        current_yes_price=market.yes_price,
                        current_no_price=market.no_price,
                        target_token_id=target_token_id or "",
                        article_published_at=article.published_at,
                    )
                    enhanced_signals.append(new_signal)
                    logger.info(
                        f"X sentiment trade: {direction.value} | Edge={realistic_edge:.0%}",
                        market=market.question[:40],
                    )

            elif signal:
                # No sentiment data, keep original signal
                enhanced_signals.append(signal)

        return enhanced_signals
