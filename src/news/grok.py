"""Grok/X (Twitter) news source using xAI API."""

import json
from datetime import datetime, timezone
from typing import Optional

import httpx

from src.news.base import NewsSource
from src.news.models import NewsArticle
from src.utils.logging import get_logger

logger = get_logger(__name__)

# Grok API endpoint (OpenAI-compatible)
GROK_API_URL = "https://api.x.ai/v1/chat/completions"


class GrokNewsSource(NewsSource):
    """
    News source that uses Grok to get trending news from X (Twitter).

    Grok has real-time access to X posts and can summarize trending topics.
    """

    # Rate limiting: minimum seconds between API calls
    RATE_LIMIT_DELAY = 60.0  # 60 seconds between calls for 24/7 operation

    def __init__(self, api_key: str) -> None:
        """
        Initialize the Grok news source.

        Args:
            api_key: xAI API key
        """
        self._api_key = api_key
        self._client: Optional[httpx.AsyncClient] = None
        self._last_call_time: float = 0.0
        logger.info("Initialized Grok/X news source")

    async def _rate_limit(self) -> None:
        """Enforce rate limiting between API calls."""
        import asyncio
        import time
        now = time.time()
        elapsed = now - self._last_call_time
        if elapsed < self.RATE_LIMIT_DELAY:
            wait_time = self.RATE_LIMIT_DELAY - elapsed
            logger.debug(f"Rate limiting: waiting {wait_time:.1f}s before Grok API call")
            await asyncio.sleep(wait_time)
        self._last_call_time = time.time()

    @property
    def name(self) -> str:
        """Get the source name."""
        return "grok"

    def is_configured(self) -> bool:
        """Check if the source is properly configured."""
        return bool(self._api_key)

    async def _get_client(self) -> httpx.AsyncClient:
        """Get or create HTTP client."""
        if self._client is None:
            self._client = httpx.AsyncClient(timeout=60.0)
        return self._client

    async def fetch_headlines(self, max_results: int = 50) -> list[NewsArticle]:
        """
        Fetch trending news from X using Grok.

        Args:
            max_results: Maximum number of articles (up to 10)

        Returns:
            List of NewsArticle objects from X/Twitter
        """
        return await self.fetch_news(query=None, max_results=max_results)

    async def fetch_news(
        self,
        query: str | None = None,
        max_results: int = 50,
    ) -> list[NewsArticle]:
        """
        Fetch news from X using Grok.

        Args:
            query: Optional topic focus
            max_results: Maximum number of articles

        Returns:
            List of news articles
        """
        if not self.is_configured():
            logger.warning("Grok not configured, skipping fetch")
            return []

        try:
            # Rate limit before API call
            await self._rate_limit()

            client = await self._get_client()

            # Build the prompt based on whether there's a query
            topic_focus = f"Focus specifically on: {query}\n\n" if query else ""

            prompt = f"""You have real-time access to X (Twitter). Search for the most important
breaking news and trending topics from the last hour that could affect prediction markets.

{topic_focus}Focus on:
- Political announcements (elections, policy changes, appointments)
- Economic news (Fed decisions, market movements, company earnings)
- Sports results and breaking sports news
- Celebrity/entertainment news
- Crypto/blockchain news
- Major world events

For each news item, provide:
1. A clear headline summarizing the news
2. A brief description (2-3 sentences)
3. The source (which account or outlet broke it)
4. Approximate time (how many minutes ago)

Return as JSON array with format:
[
  {{
    "headline": "Breaking: ...",
    "description": "...",
    "source": "@username or outlet name",
    "minutes_ago": 15
  }}
]

Return up to {min(max_results, 10)} of the most market-relevant news items. Only include real, verifiable news."""

            response = await client.post(
                GROK_API_URL,
                headers={
                    "Content-Type": "application/json",
                    "Authorization": f"Bearer {self._api_key}",
                },
                json={
                    "model": "grok-3-fast-latest",
                    "messages": [
                        {
                            "role": "system",
                            "content": "You are a real-time news analyst with access to X (Twitter). "
                            "Your job is to find breaking news that could impact prediction markets. "
                            "Always return valid JSON.",
                        },
                        {"role": "user", "content": prompt},
                    ],
                    "temperature": 0.1,
                },
            )

            if response.status_code != 200:
                logger.error(
                    "Grok API error",
                    status=response.status_code,
                    response=response.text[:200],
                )
                return []

            data = response.json()
            content = data.get("choices", [{}])[0].get("message", {}).get("content", "")

            # Parse JSON from response
            articles = self._parse_response(content)
            logger.info(f"Fetched {len(articles)} items from Grok/X")
            return articles

        except Exception as e:
            logger.error("Failed to fetch from Grok", error=str(e))
            return []

    def _parse_response(self, content: str) -> list[NewsArticle]:
        """
        Parse Grok's response into NewsArticle objects.

        Args:
            content: Raw response content from Grok

        Returns:
            List of NewsArticle objects
        """
        articles = []

        try:
            # Try to extract JSON from the response
            # Sometimes Grok wraps it in markdown code blocks
            if "```json" in content:
                content = content.split("```json")[1].split("```")[0]
            elif "```" in content:
                content = content.split("```")[1].split("```")[0]

            news_items = json.loads(content)

            for item in news_items:
                # Calculate published time based on minutes_ago
                minutes_ago = item.get("minutes_ago", 30)
                published_at = datetime.now(timezone.utc)

                headline = item.get("headline", "")
                # Generate unique ID from headline
                article_id = f"grok_{abs(hash(headline)) % (10**10)}"

                articles.append(
                    NewsArticle(
                        id=article_id,
                        title=headline,
                        description=item.get("description", ""),
                        content=item.get("description", ""),
                        source_name=f"X/{item.get('source', 'unknown')}",
                        url=f"https://x.com/search?q={headline.replace(' ', '%20')[:50]}",
                        published_at=published_at,
                    )
                )

        except json.JSONDecodeError as e:
            logger.warning("Failed to parse Grok response as JSON", error=str(e))
        except Exception as e:
            logger.warning("Error parsing Grok response", error=str(e))

        return articles
