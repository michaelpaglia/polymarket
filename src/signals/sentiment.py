"""Twitter/X sentiment analysis using Grok for trading edge."""

import asyncio
import json
from dataclasses import dataclass
from typing import Optional

import httpx

from src.utils.logging import get_logger

logger = get_logger(__name__)

GROK_API_URL = "https://api.x.ai/v1/chat/completions"


@dataclass
class SentimentAnalysis:
    """Result of X/Twitter sentiment analysis."""

    topic: str
    sentiment: str  # "bullish", "bearish", "neutral"
    sentiment_score: float  # 0.0-1.0 (1.0 = very bullish)
    discussion_volume: str  # "low", "medium", "high"
    key_opinions: list[str]
    confidence: float  # 0.0-1.0
    reasoning: str

    # Edge indicators
    breaking_news_detected: bool = False
    influencer_activity: bool = False
    sentiment_shift: Optional[str] = None  # "strengthening", "weakening", "stable"

    # NOVEL EDGE INDICATORS
    velocity_score: float = 0.0  # 0-1, how fast is discussion accelerating
    influencer_weight: float = 0.0  # 0-1, are big accounts talking or just randoms
    contrarian_signal: bool = False  # True if sentiment is extreme (potential reversal)
    smart_money_direction: Optional[str] = None  # What are "experts" saying vs crowd

    @property
    def is_strong_signal(self) -> bool:
        """Check if sentiment provides a strong trading signal."""
        # Strong if: high confidence + volume + not neutral
        base_strong = (
            self.confidence >= 0.6
            and self.discussion_volume in ("medium", "high")
            and abs(self.sentiment_score - 0.5) >= 0.15
        )

        # Boost if: velocity spike OR major influencer activity
        velocity_boost = self.velocity_score >= 0.7
        influencer_boost = self.influencer_weight >= 0.7

        return base_strong or velocity_boost or influencer_boost

    @property
    def edge_multiplier(self) -> float:
        """
        Calculate edge multiplier based on signal quality.

        Higher = more confident in the edge.
        """
        multiplier = 1.0

        # Velocity spike = information is fresh
        if self.velocity_score >= 0.8:
            multiplier += 0.3
        elif self.velocity_score >= 0.5:
            multiplier += 0.1

        # Influencer activity = higher signal quality
        if self.influencer_weight >= 0.8:
            multiplier += 0.2
        elif self.influencer_weight >= 0.5:
            multiplier += 0.1

        # Breaking news = maximum edge
        if self.breaking_news_detected:
            multiplier += 0.5

        # Sentiment shift = momentum
        if self.sentiment_shift == "strengthening":
            multiplier += 0.1

        # Contrarian discount (extreme sentiment often reverses)
        if self.contrarian_signal:
            multiplier -= 0.2

        return min(2.0, max(0.5, multiplier))

    @property
    def direction(self) -> str:
        """Get trading direction based on sentiment."""
        if self.sentiment_score >= 0.55:
            return "YES"
        elif self.sentiment_score <= 0.45:
            return "NO"
        return "HOLD"


class XSentimentAnalyzer:
    """
    Analyzes X/Twitter sentiment for prediction market edges.

    Exploits/edges this provides:
    1. Pre-news sentiment - Twitter often knows before mainstream media
    2. Crowd probability estimation - What does Twitter think the odds are?
    3. Event verification - Confirm news is real via Twitter reaction
    4. Influencer tracking - Key accounts that move markets
    5. Sentiment momentum - Shifting sentiment is predictive
    """

    # Rate limiting: minimum seconds between API calls
    RATE_LIMIT_DELAY = 60.0  # 60 seconds between calls for 24/7 operation

    def __init__(self, api_key: str, proxy_url: str = "") -> None:
        """Initialize the sentiment analyzer."""
        self.api_key = api_key
        self.proxy_url = proxy_url
        self._client: Optional[httpx.AsyncClient] = None
        self._last_call_time: float = 0.0
        if proxy_url:
            logger.info(f"Initialized X sentiment analyzer with EU proxy")
        else:
            logger.info("Initialized X sentiment analyzer")

    async def _rate_limit(self) -> None:
        """Enforce rate limiting between API calls."""
        import time
        now = time.time()
        elapsed = now - self._last_call_time
        if elapsed < self.RATE_LIMIT_DELAY:
            wait_time = self.RATE_LIMIT_DELAY - elapsed
            logger.debug(f"Rate limiting: waiting {wait_time:.1f}s before X API call")
            await asyncio.sleep(wait_time)
        self._last_call_time = time.time()

    async def _get_client(self) -> httpx.AsyncClient:
        """Get or create HTTP client with optional proxy."""
        if self._client is None:
            if self.proxy_url:
                self._client = httpx.AsyncClient(
                    timeout=60.0,
                    proxy=self.proxy_url,
                )
            else:
                self._client = httpx.AsyncClient(timeout=60.0)
        return self._client

    async def analyze_market_sentiment(
        self,
        market_question: str,
        context: str = "",
    ) -> Optional[SentimentAnalysis]:
        """
        Analyze X sentiment for a specific market question.

        Args:
            market_question: The prediction market question
            context: Optional additional context (e.g., recent news)

        Returns:
            SentimentAnalysis with X sentiment data
        """
        if not self.api_key:
            return None

        try:
            # Rate limit before API call
            await self._rate_limit()

            client = await self._get_client()

            context_text = f"\nRecent news context: {context}" if context else ""

            prompt = f"""Search X (Twitter) right now for discussions related to this prediction market:

MARKET QUESTION: {market_question}
{context_text}

Analyze the real-time X sentiment. I need EDGE DETECTION - things that would help predict market moves:

1. **Sentiment**: Are people bullish (YES) or bearish (NO)?
2. **Volume**: How much discussion right now?
3. **VELOCITY**: Is discussion ACCELERATING? (sudden spike in last 1-2 hours = something happened)
4. **INFLUENCER WEIGHT**: Are big accounts talking (politicians, journalists, celebrities) or just random users?
5. **Breaking news**: Any news that JUST broke on X (not yet in mainstream media)?
6. **Sentiment shift**: Is sentiment strengthening or weakening vs earlier today?
7. **Contrarian check**: Is sentiment EXTREMELY one-sided (>90%)? That often means reversal coming.
8. **Smart money**: What are "expert" accounts saying vs the general crowd?

Return JSON:
{{
    "topic": "extracted topic",
    "sentiment": "bullish" or "bearish" or "neutral",
    "sentiment_score": 0.0-1.0 (what probability does X crowd imply?),
    "discussion_volume": "low" or "medium" or "high",
    "key_opinions": ["opinion 1", "opinion 2"],
    "confidence": 0.0-1.0,
    "reasoning": "explanation",
    "breaking_news_detected": true/false,
    "influencer_activity": true/false,
    "sentiment_shift": "strengthening" or "weakening" or "stable",
    "velocity_score": 0.0-1.0 (0=stable, 1=exploding right now),
    "influencer_weight": 0.0-1.0 (0=only randoms, 1=major accounts engaged),
    "contrarian_signal": true/false (true if >90% one-sided),
    "smart_money_direction": "bullish" or "bearish" or "neutral" or null
}}"""

            response = await client.post(
                GROK_API_URL,
                headers={
                    "Content-Type": "application/json",
                    "Authorization": f"Bearer {self.api_key}",
                },
                json={
                    "model": "grok-4-1-fast-non-reasoning",
                    "messages": [
                        {
                            "role": "system",
                            "content": "You have real-time access to X (Twitter). "
                            "You are a sentiment analyst for prediction markets. "
                            "Analyze what people on X are saying and thinking about topics. "
                            "Always return valid JSON.",
                        },
                        {"role": "user", "content": prompt},
                    ],
                    "temperature": 0.1,
                },
            )

            if response.status_code != 200:
                logger.error("Grok sentiment API error", status=response.status_code)
                return None

            data = response.json()
            content = data.get("choices", [{}])[0].get("message", {}).get("content", "")

            return self._parse_response(content)

        except Exception as e:
            logger.error("Sentiment analysis failed", error=str(e))
            return None

    async def analyze_multiple(
        self,
        markets: list[tuple[str, str]],  # List of (market_question, context)
    ) -> list[Optional[SentimentAnalysis]]:
        """
        Analyze sentiment for multiple markets sequentially with rate limiting.

        Args:
            markets: List of (market_question, context) tuples

        Returns:
            List of SentimentAnalysis results
        """
        # Run sequentially to respect rate limits (not in parallel!)
        results = []
        for question, context in markets:
            result = await self.analyze_market_sentiment(question, context)
            results.append(result)
        return results

    async def get_crowd_probability(
        self,
        market_question: str,
    ) -> Optional[float]:
        """
        Ask Grok what X users think the probability is.

        This is a unique edge - crowd wisdom from Twitter.

        Args:
            market_question: The prediction market question

        Returns:
            Estimated probability from X sentiment (0.0-1.0)
        """
        if not self.api_key:
            return None

        try:
            # Rate limit before API call
            await self._rate_limit()

            client = await self._get_client()

            prompt = f"""Search X (Twitter) for discussions about this prediction:

"{market_question}"

Based on what people on X are saying:
1. What do most people seem to think will happen?
2. If you had to estimate - what probability would X users assign to YES?

Return JSON:
{{
    "crowd_probability": 0.0-1.0,
    "reasoning": "brief explanation of why X users think this"
}}"""

            response = await client.post(
                GROK_API_URL,
                headers={
                    "Content-Type": "application/json",
                    "Authorization": f"Bearer {self.api_key}",
                },
                json={
                    "model": "grok-4-1-fast-non-reasoning",
                    "messages": [
                        {
                            "role": "system",
                            "content": "You analyze X/Twitter to estimate crowd probabilities. "
                            "Return only valid JSON.",
                        },
                        {"role": "user", "content": prompt},
                    ],
                    "temperature": 0.1,
                },
            )

            if response.status_code != 200:
                return None

            data = response.json()
            content = data.get("choices", [{}])[0].get("message", {}).get("content", "")

            # Extract JSON
            if "```json" in content:
                content = content.split("```json")[1].split("```")[0]
            elif "```" in content:
                content = content.split("```")[1].split("```")[0]

            result = json.loads(content)
            return float(result.get("crowd_probability", 0.5))

        except Exception as e:
            logger.error("Crowd probability failed", error=str(e))
            return None

    async def detect_breaking_news(
        self,
        topics: list[str],
    ) -> list[dict]:
        """
        Scan X for breaking news on specific topics.

        Edge: Twitter breaks news 5-30 minutes before mainstream media.

        Args:
            topics: List of topics to monitor

        Returns:
            List of breaking news items detected
        """
        if not self.api_key:
            return []

        try:
            # Rate limit before API call
            await self._rate_limit()

            client = await self._get_client()

            topics_str = ", ".join(topics)

            prompt = f"""Search X (Twitter) for breaking news in the last 30 minutes about:
{topics_str}

Focus on:
- News that JUST broke (within last 30 minutes)
- Significant developments that could move prediction markets
- Verified or highly credible sources

Return JSON:
{{
    "breaking_news": [
        {{
            "headline": "what happened",
            "topic": "which topic",
            "source": "who reported it",
            "minutes_ago": estimated minutes,
            "credibility": "high/medium/low",
            "market_impact": "brief note on potential market impact"
        }}
    ]
}}

Return empty array if no breaking news found."""

            response = await client.post(
                GROK_API_URL,
                headers={
                    "Content-Type": "application/json",
                    "Authorization": f"Bearer {self.api_key}",
                },
                json={
                    "model": "grok-4-1-fast-non-reasoning",
                    "messages": [
                        {
                            "role": "system",
                            "content": "You detect breaking news on X/Twitter. "
                            "Only report genuinely new developments. Return valid JSON.",
                        },
                        {"role": "user", "content": prompt},
                    ],
                    "temperature": 0.1,
                },
            )

            if response.status_code != 200:
                return []

            data = response.json()
            content = data.get("choices", [{}])[0].get("message", {}).get("content", "")

            if "```json" in content:
                content = content.split("```json")[1].split("```")[0]
            elif "```" in content:
                content = content.split("```")[1].split("```")[0]

            result = json.loads(content)
            return result.get("breaking_news", [])

        except Exception as e:
            logger.error("Breaking news detection failed", error=str(e))
            return []

    def _parse_response(self, content: str) -> Optional[SentimentAnalysis]:
        """Parse Grok response into SentimentAnalysis."""
        try:
            if "```json" in content:
                content = content.split("```json")[1].split("```")[0]
            elif "```" in content:
                content = content.split("```")[1].split("```")[0]

            data = json.loads(content)

            # Grok tends to be overconfident - scale down confidence
            raw_confidence = float(data.get("confidence", 0.5))
            # Apply sigmoid-like dampening: high values get reduced more
            # 0.75 -> ~0.55, 0.9 -> ~0.65, 0.5 -> ~0.45
            calibrated_confidence = raw_confidence * 0.6 + 0.1  # Max ~0.7

            # Also dampen sentiment score to be more realistic
            raw_sentiment = float(data.get("sentiment_score", 0.5))
            # Pull towards 0.5 (reduce extremes)
            calibrated_sentiment = 0.5 + (raw_sentiment - 0.5) * 0.7

            return SentimentAnalysis(
                topic=data.get("topic", ""),
                sentiment=data.get("sentiment", "neutral"),
                sentiment_score=calibrated_sentiment,
                discussion_volume=data.get("discussion_volume", "low"),
                key_opinions=data.get("key_opinions", []),
                confidence=calibrated_confidence,
                reasoning=data.get("reasoning", ""),
                breaking_news_detected=data.get("breaking_news_detected", False),
                influencer_activity=data.get("influencer_activity", False),
                sentiment_shift=data.get("sentiment_shift"),
                # Novel edge indicators
                velocity_score=float(data.get("velocity_score", 0.0)),
                influencer_weight=float(data.get("influencer_weight", 0.0)),
                contrarian_signal=data.get("contrarian_signal", False),
                smart_money_direction=data.get("smart_money_direction"),
            )
        except Exception as e:
            logger.warning("Failed to parse sentiment response", error=str(e))
            return None
