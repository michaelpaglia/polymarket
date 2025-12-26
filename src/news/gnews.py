"""GNews news source implementation."""

import hashlib
from datetime import datetime
from typing import Optional

import httpx

from src.news.base import NewsSource
from src.news.models import NewsArticle
from src.utils.logging import get_logger

logger = get_logger(__name__)

GNEWS_BASE = "https://gnews.io/api/v4"


class GNewsSource(NewsSource):
    """
    News source using GNews.io.

    Free tier: 100 requests/day, 10 articles per request
    """

    def __init__(self, api_key: str) -> None:
        """
        Initialize the GNews source.

        Args:
            api_key: GNews API key
        """
        self._api_key = api_key

    @property
    def name(self) -> str:
        return "gnews"

    def is_configured(self) -> bool:
        return bool(self._api_key)

    async def fetch_headlines(self, max_results: int = 50) -> list[NewsArticle]:
        """
        Fetch top headlines from GNews.

        Args:
            max_results: Maximum number of articles

        Returns:
            List of news articles
        """
        if not self.is_configured():
            logger.warning("GNews not configured, skipping fetch")
            return []

        try:
            async with httpx.AsyncClient(timeout=30.0) as client:
                response = await client.get(
                    f"{GNEWS_BASE}/top-headlines",
                    params={
                        "token": self._api_key,
                        "lang": "en",
                        "country": "us",
                        "max": min(max_results, 10),  # GNews free tier limit
                    },
                )
                response.raise_for_status()
                data = response.json()

                articles = []
                for article_data in data.get("articles", []):
                    article = self._parse_article(article_data)
                    if article:
                        articles.append(article)

                logger.info(f"Fetched {len(articles)} headlines from GNews")
                return articles

        except httpx.HTTPError as e:
            logger.error("Failed to fetch from GNews", error=str(e))
            return []
        except Exception as e:
            logger.error("Unexpected error fetching from GNews", error=str(e))
            return []

    async def fetch_news(
        self,
        query: str | None = None,
        max_results: int = 50,
    ) -> list[NewsArticle]:
        """
        Fetch news articles from GNews.

        Args:
            query: Optional search query
            max_results: Maximum number of articles

        Returns:
            List of news articles
        """
        if not self.is_configured():
            logger.warning("GNews not configured, skipping fetch")
            return []

        # If no query, just get headlines
        if not query:
            return await self.fetch_headlines(max_results)

        try:
            async with httpx.AsyncClient(timeout=30.0) as client:
                response = await client.get(
                    f"{GNEWS_BASE}/search",
                    params={
                        "token": self._api_key,
                        "q": query,
                        "lang": "en",
                        "max": min(max_results, 10),  # GNews free tier limit
                    },
                )
                response.raise_for_status()
                data = response.json()

                articles = []
                for article_data in data.get("articles", []):
                    article = self._parse_article(article_data)
                    if article:
                        articles.append(article)

                logger.info(f"Fetched {len(articles)} articles from GNews for query: {query}")
                return articles

        except httpx.HTTPError as e:
            logger.error("Failed to fetch from GNews", error=str(e))
            return []
        except Exception as e:
            logger.error("Unexpected error fetching from GNews", error=str(e))
            return []

    def _parse_article(self, data: dict) -> Optional[NewsArticle]:
        """Parse raw article data into NewsArticle."""
        try:
            url = data.get("url", "")
            if not url:
                return None

            # Generate ID from URL
            article_id = hashlib.md5(url.encode()).hexdigest()

            # Parse published date
            published_at = None
            published_str = data.get("publishedAt")
            if published_str:
                try:
                    published_at = datetime.fromisoformat(published_str.replace("Z", "+00:00"))
                except ValueError:
                    pass

            # Get source info
            source = data.get("source", {})

            return NewsArticle(
                id=article_id,
                url=url,
                title=data.get("title", ""),
                description=data.get("description", ""),
                content=data.get("content", ""),
                source_name=source.get("name", ""),
                source_id=source.get("url", ""),
                published_at=published_at,
                image_url=data.get("image", ""),
            )
        except Exception as e:
            logger.warning("Failed to parse GNews article", error=str(e))
            return None
