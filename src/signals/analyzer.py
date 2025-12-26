"""Signal analyzer using LLM to generate trading signals from news."""

import json
import time
import uuid
from typing import Any

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
    """

    def __init__(
        self,
        llm_settings: LLMSettings,
        signal_settings: SignalSettings,
        risk_settings: RiskSettings,
    ) -> None:
        """
        Initialize the signal analyzer.

        Args:
            llm_settings: LLM configuration
            signal_settings: Signal generation settings
            risk_settings: Risk management settings
        """
        self.llm_settings = llm_settings
        self.signal_settings = signal_settings
        self.risk_settings = risk_settings
        self._client: genai.Client | None = None

        # Configure Gemini
        if llm_settings.google_api_key:
            self._client = genai.Client(api_key=llm_settings.google_api_key)
            logger.info(f"Signal analyzer initialized with {llm_settings.model}")
        else:
            logger.warning("No Google API key configured for signal analyzer")

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
            # Generate signals using LLM
            signals = await self._generate_signals(article, matches)

            # Calculate position sizes
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
