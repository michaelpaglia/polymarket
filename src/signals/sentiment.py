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

    @property
    def is_strong_signal(self) -> bool:
        """Check if sentiment provides a strong trading signal."""
        return (
            self.confidence >= 0.7
            and self.discussion_volume in ("medium", "high")
            and abs(self.sentiment_score - 0.5) >= 0.2  # Not neutral
        )

    @property
    def direction(self) -> str:
        """Get trading direction based on sentiment."""
        if self.sentiment_score >= 0.6:
            return "YES"
        elif self.sentiment_score <= 0.4:
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

    def __init__(self, api_key: str) -> None:
        """Initialize the sentiment analyzer."""
        self.api_key = api_key
        self._client: Optional[httpx.AsyncClient] = None
        logger.info("Initialized X sentiment analyzer")

    async def _get_client(self) -> httpx.AsyncClient:
        """Get or create HTTP client."""
        if self._client is None:
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
            client = await self._get_client()

            context_text = f"\nRecent news context: {context}" if context else ""

            prompt = f"""Search X (Twitter) right now for discussions related to this prediction market:

MARKET QUESTION: {market_question}
{context_text}

Analyze the real-time X sentiment and provide:

1. Overall sentiment - Are people on X bullish (think YES will happen) or bearish (think NO)?
2. Discussion volume - How much is this being discussed right now?
3. Key opinions - What are influential accounts or popular tweets saying?
4. Breaking news - Is there any breaking news on X about this topic?
5. Influencer activity - Have any major accounts (politicians, celebrities, experts) posted about this?
6. Sentiment shift - Is sentiment strengthening, weakening, or stable compared to earlier?

Return JSON:
{{
    "topic": "extracted topic",
    "sentiment": "bullish" or "bearish" or "neutral",
    "sentiment_score": 0.0-1.0,
    "discussion_volume": "low" or "medium" or "high",
    "key_opinions": ["opinion 1", "opinion 2"],
    "confidence": 0.0-1.0,
    "reasoning": "explanation",
    "breaking_news_detected": true/false,
    "influencer_activity": true/false,
    "sentiment_shift": "strengthening" or "weakening" or "stable"
}}"""

            response = await client.post(
                GROK_API_URL,
                headers={
                    "Content-Type": "application/json",
                    "Authorization": f"Bearer {self.api_key}",
                },
                json={
                    "model": "grok-3-fast-latest",
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
        Analyze sentiment for multiple markets in parallel.

        Args:
            markets: List of (market_question, context) tuples

        Returns:
            List of SentimentAnalysis results
        """
        tasks = [
            self.analyze_market_sentiment(question, context)
            for question, context in markets
        ]
        return await asyncio.gather(*tasks)

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
                    "model": "grok-3-fast-latest",
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
                    "model": "grok-3-fast-latest",
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

            return SentimentAnalysis(
                topic=data.get("topic", ""),
                sentiment=data.get("sentiment", "neutral"),
                sentiment_score=float(data.get("sentiment_score", 0.5)),
                discussion_volume=data.get("discussion_volume", "low"),
                key_opinions=data.get("key_opinions", []),
                confidence=float(data.get("confidence", 0.5)),
                reasoning=data.get("reasoning", ""),
                breaking_news_detected=data.get("breaking_news_detected", False),
                influencer_activity=data.get("influencer_activity", False),
                sentiment_shift=data.get("sentiment_shift"),
            )
        except Exception as e:
            logger.warning("Failed to parse sentiment response", error=str(e))
            return None
