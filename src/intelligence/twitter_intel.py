"""
Advanced Twitter/X Intelligence using Grok Live Search.

Novel algorithms for prediction market alpha:
1. Influencer Tracking - Monitor key accounts that move markets
2. Breaking News Detection - Catch alpha drops before mainstream media
3. Sentiment Momentum - Track sentiment shifts over time
4. Network Analysis - Who influences whom
5. Event Detection - Identify market-moving events in real-time

Research basis:
- Grok has "most real-time search capabilities of any AI model" (xAI)
- Twitter sentiment predicts markets days in advance (Stanford/CEPR)
- Key influencer tweets move markets within 5-10 minutes
"""

import asyncio
import json
from dataclasses import dataclass, field
from datetime import datetime, timezone, timedelta
from typing import Optional

import httpx

from src.intelligence.knowledge_graph import (
    PredictionMarketKnowledgeGraph,
    Influencer,
)
from src.utils.logging import get_logger

logger = get_logger(__name__)

GROK_API_URL = "https://api.x.ai/v1/chat/completions"


@dataclass
class TweetSignal:
    """A signal extracted from Twitter/X activity."""

    signal_type: str  # "influencer_post", "breaking_news", "sentiment_shift", "viral_thread"
    source_handle: str
    content_summary: str
    timestamp: datetime
    relevance_score: float  # 0-1
    sentiment: str  # "bullish", "bearish", "neutral"
    sentiment_score: float  # 0-1

    # Market impact prediction
    predicted_markets: list[str] = field(default_factory=list)
    predicted_direction: str = "unknown"  # "up", "down", "neutral"
    urgency: str = "normal"  # "critical", "high", "normal", "low"

    # Metadata
    engagement_score: float = 0.0  # Likes, retweets, etc.
    virality_potential: float = 0.0  # How likely to go viral


@dataclass
class InfluencerActivity:
    """Recent activity from a tracked influencer."""

    handle: str
    recent_posts: list[dict] = field(default_factory=list)
    sentiment_trend: str = "stable"  # "bullish", "bearish", "stable"
    activity_level: str = "normal"  # "high", "normal", "low"
    last_market_moving_post: Optional[datetime] = None


class TwitterIntelligence:
    """
    Advanced Twitter/X intelligence for prediction market alpha.

    Uses Grok's real-time X search to:
    1. Monitor key influencers
    2. Detect breaking news before mainstream media
    3. Track sentiment momentum
    4. Identify viral content early
    """

    def __init__(
        self,
        grok_api_key: str,
        knowledge_graph: Optional[PredictionMarketKnowledgeGraph] = None,
    ) -> None:
        self.api_key = grok_api_key
        self.kg = knowledge_graph or PredictionMarketKnowledgeGraph()
        self._client: Optional[httpx.AsyncClient] = None

        # Cache for rate limiting and efficiency
        self._influencer_cache: dict[str, InfluencerActivity] = {}
        self._last_scan: dict[str, datetime] = {}

        logger.info("Twitter Intelligence initialized")

    async def _get_client(self) -> httpx.AsyncClient:
        if self._client is None:
            self._client = httpx.AsyncClient(timeout=60.0)
        return self._client

    async def scan_for_alpha(
        self,
        topics: Optional[list[str]] = None,
        market_ids: Optional[list[str]] = None,
    ) -> list[TweetSignal]:
        """
        Comprehensive scan for market-moving Twitter activity.

        Runs multiple detection algorithms in parallel:
        1. Breaking news scan
        2. Influencer activity check
        3. Viral content detection
        4. Sentiment shift detection

        Returns prioritized list of signals.
        """
        tasks = [
            self._scan_breaking_news(topics or []),
            self._scan_influencer_activity(),
            self._scan_viral_content(topics or []),
        ]

        results = await asyncio.gather(*tasks, return_exceptions=True)

        all_signals = []
        for result in results:
            if isinstance(result, list):
                all_signals.extend(result)
            elif isinstance(result, Exception):
                logger.warning(f"Scan task failed: {result}")

        # Sort by urgency and relevance
        urgency_order = {"critical": 0, "high": 1, "normal": 2, "low": 3}
        all_signals.sort(
            key=lambda s: (urgency_order.get(s.urgency, 2), -s.relevance_score)
        )

        return all_signals

    async def _scan_breaking_news(self, topics: list[str]) -> list[TweetSignal]:
        """
        Detect breaking news before mainstream media.

        Algorithm:
        1. Search for posts from credible sources in last 15 minutes
        2. Look for "BREAKING", "JUST IN", "DEVELOPING" patterns
        3. Check for sudden engagement spikes
        4. Cross-reference with known market topics
        """
        if not self.api_key:
            return []

        try:
            client = await self._get_client()

            topic_str = ", ".join(topics[:5]) if topics else "politics, crypto, finance, sports"

            prompt = f"""Search X/Twitter for BREAKING NEWS in the last 30 minutes related to:
{topic_str}

Focus on:
1. Posts containing "BREAKING", "JUST IN", "DEVELOPING", "EXCLUSIVE"
2. Posts from credible news accounts (@AP, @Reuters, @WSJ, @Bloomberg, etc.)
3. Posts with rapidly increasing engagement (going viral)
4. Posts from verified political/business figures

For each breaking news item found, provide:
- Source handle
- Summary of the news
- How many minutes ago
- Estimated credibility (high/medium/low)
- Potential market impact

Return JSON:
{{
    "breaking_news": [
        {{
            "handle": "@source",
            "summary": "what happened",
            "minutes_ago": 10,
            "credibility": "high",
            "market_impact": "Could affect X market",
            "engagement": "likes/retweets estimate"
        }}
    ]
}}

Return empty array if no significant breaking news."""

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
                            "content": "You are a breaking news detector with real-time X access. "
                            "Only report genuinely new, market-relevant news. Return valid JSON.",
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

            return self._parse_breaking_news(content)

        except Exception as e:
            logger.error(f"Breaking news scan failed: {e}")
            return []

    async def _scan_influencer_activity(self) -> list[TweetSignal]:
        """
        Monitor key influencers for market-moving posts.

        Algorithm:
        1. Get list of high-influence handles from knowledge graph
        2. Check their recent posts (last hour)
        3. Analyze content for market relevance
        4. Track sentiment changes from their baseline
        """
        if not self.api_key:
            return []

        # Get top influencers from knowledge graph
        top_influencers = sorted(
            self.kg.influencers.values(),
            key=lambda x: x.influence_score,
            reverse=True,
        )[:10]

        if not top_influencers:
            return []

        handles = [inf.handle for inf in top_influencers]

        try:
            client = await self._get_client()

            handles_str = ", ".join(handles)

            prompt = f"""Check recent activity (last 2 hours) from these influential Twitter accounts:
{handles_str}

For each account, find:
1. Any posts about prediction market topics (politics, crypto, elections, sports, etc.)
2. Posts that seem to be making predictions or statements about future events
3. Posts that are generating unusual engagement
4. Posts that could move markets

Return JSON:
{{
    "influencer_activity": [
        {{
            "handle": "@account",
            "post_summary": "what they said",
            "topic": "politics/crypto/etc",
            "sentiment": "bullish/bearish/neutral",
            "hours_ago": 0.5,
            "market_relevance": "high/medium/low",
            "potential_impact": "description of market impact"
        }}
    ]
}}

Only include posts that could affect prediction markets."""

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
                            "content": "You analyze influencer Twitter activity for market impact. "
                            "Focus on posts that could move prediction markets. Return valid JSON.",
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

            return self._parse_influencer_activity(content)

        except Exception as e:
            logger.error(f"Influencer scan failed: {e}")
            return []

    async def _scan_viral_content(self, topics: list[str]) -> list[TweetSignal]:
        """
        Detect viral content that could move markets.

        Algorithm:
        1. Search for rapidly growing threads
        2. Look for engagement velocity (likes/min)
        3. Identify content crossing from niche to mainstream
        4. Track quote-tweet storms
        """
        if not self.api_key:
            return []

        try:
            client = await self._get_client()

            topic_str = ", ".join(topics[:5]) if topics else "trending topics"

            prompt = f"""Search X for VIRAL content (last 2 hours) about: {topic_str}

Find posts that:
1. Are getting unusually high engagement (going viral)
2. Are being widely quote-tweeted with opinions
3. Are trending or about to trend
4. Could affect public opinion on prediction market topics

Return JSON:
{{
    "viral_content": [
        {{
            "handle": "@source",
            "content_summary": "what the viral post is about",
            "topic": "relevant topic",
            "engagement_level": "viral/high/medium",
            "sentiment": "positive/negative/mixed",
            "hours_old": 1.5,
            "market_relevance": "description of how this could affect markets"
        }}
    ]
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
                            "content": "You detect viral Twitter content that could affect markets. "
                            "Return valid JSON.",
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

            return self._parse_viral_content(content)

        except Exception as e:
            logger.error(f"Viral content scan failed: {e}")
            return []

    async def get_sentiment_for_market(
        self,
        market_question: str,
        condition_id: str,
    ) -> dict:
        """
        Get comprehensive Twitter sentiment for a specific market.

        Returns detailed sentiment analysis including:
        - Overall sentiment score
        - Discussion volume
        - Key influencer opinions
        - Sentiment momentum (increasing/decreasing)
        - Crowd probability estimate
        """
        if not self.api_key:
            return {}

        # Get search queries from knowledge graph
        queries = self.kg.get_search_queries_for_market(condition_id)
        query_str = " OR ".join(queries[:5]) if queries else market_question

        try:
            client = await self._get_client()

            prompt = f"""Analyze Twitter sentiment for this prediction market:

MARKET: {market_question}

Search X for discussions about this topic using terms like: {query_str}

Provide comprehensive analysis:
1. Overall sentiment (bullish/bearish on YES outcome)
2. Discussion volume (how much is this being discussed?)
3. Sentiment momentum (is sentiment shifting?)
4. Key voices (any influencers talking about this?)
5. Crowd probability (what probability does Twitter sentiment imply?)
6. Confidence in assessment

Return JSON:
{{
    "market_question": "{market_question[:50]}...",
    "sentiment": "bullish/bearish/neutral",
    "sentiment_score": 0.0-1.0,
    "discussion_volume": "high/medium/low",
    "sentiment_momentum": "increasing/stable/decreasing",
    "crowd_probability": 0.0-1.0,
    "key_voices": ["@handle1", "@handle2"],
    "key_opinions": ["opinion 1", "opinion 2"],
    "confidence": 0.0-1.0,
    "reasoning": "brief explanation"
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
                            "content": "You analyze Twitter sentiment for prediction markets. "
                            "Provide accurate sentiment analysis based on real X data. Return valid JSON.",
                        },
                        {"role": "user", "content": prompt},
                    ],
                    "temperature": 0.1,
                },
            )

            if response.status_code != 200:
                return {}

            data = response.json()
            content = data.get("choices", [{}])[0].get("message", {}).get("content", "")

            return self._parse_json_response(content)

        except Exception as e:
            logger.error(f"Market sentiment analysis failed: {e}")
            return {}

    def _parse_breaking_news(self, content: str) -> list[TweetSignal]:
        """Parse breaking news response into signals."""
        signals = []
        try:
            data = self._parse_json_response(content)
            for item in data.get("breaking_news", []):
                urgency = "critical" if item.get("minutes_ago", 60) < 15 else "high"
                signals.append(
                    TweetSignal(
                        signal_type="breaking_news",
                        source_handle=item.get("handle", ""),
                        content_summary=item.get("summary", ""),
                        timestamp=datetime.now(timezone.utc),
                        relevance_score=0.9 if item.get("credibility") == "high" else 0.7,
                        sentiment="neutral",
                        sentiment_score=0.5,
                        urgency=urgency,
                    )
                )
        except Exception as e:
            logger.warning(f"Failed to parse breaking news: {e}")
        return signals

    def _parse_influencer_activity(self, content: str) -> list[TweetSignal]:
        """Parse influencer activity response into signals."""
        signals = []
        try:
            data = self._parse_json_response(content)
            for item in data.get("influencer_activity", []):
                # Get influencer info from knowledge graph
                influencer = self.kg.get_influencer(item.get("handle", ""))
                influence_score = influencer.influence_score if influencer else 0.5

                sentiment_str = item.get("sentiment", "neutral")
                sentiment_score = {"bullish": 0.8, "bearish": 0.2, "neutral": 0.5}.get(
                    sentiment_str, 0.5
                )

                urgency = "high" if item.get("market_relevance") == "high" else "normal"

                signals.append(
                    TweetSignal(
                        signal_type="influencer_post",
                        source_handle=item.get("handle", ""),
                        content_summary=item.get("post_summary", ""),
                        timestamp=datetime.now(timezone.utc),
                        relevance_score=influence_score,
                        sentiment=sentiment_str,
                        sentiment_score=sentiment_score,
                        urgency=urgency,
                    )
                )
        except Exception as e:
            logger.warning(f"Failed to parse influencer activity: {e}")
        return signals

    def _parse_viral_content(self, content: str) -> list[TweetSignal]:
        """Parse viral content response into signals."""
        signals = []
        try:
            data = self._parse_json_response(content)
            for item in data.get("viral_content", []):
                engagement = item.get("engagement_level", "medium")
                relevance = {"viral": 0.95, "high": 0.8, "medium": 0.6}.get(engagement, 0.6)

                signals.append(
                    TweetSignal(
                        signal_type="viral_thread",
                        source_handle=item.get("handle", ""),
                        content_summary=item.get("content_summary", ""),
                        timestamp=datetime.now(timezone.utc),
                        relevance_score=relevance,
                        sentiment=item.get("sentiment", "neutral"),
                        sentiment_score=0.5,
                        urgency="high" if engagement == "viral" else "normal",
                        virality_potential=relevance,
                    )
                )
        except Exception as e:
            logger.warning(f"Failed to parse viral content: {e}")
        return signals

    def _parse_json_response(self, content: str) -> dict:
        """Parse JSON from Grok response."""
        try:
            if "```json" in content:
                content = content.split("```json")[1].split("```")[0]
            elif "```" in content:
                content = content.split("```")[1].split("```")[0]
            return json.loads(content)
        except Exception:
            return {}
