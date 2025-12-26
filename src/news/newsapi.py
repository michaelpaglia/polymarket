"""NewsAPI news source implementation."""

import hashlib
from datetime import datetime
from typing import Optional

import httpx

from src.news.base import NewsSource
from src.news.models import NewsArticle
from src.utils.logging import get_logger

logger = get_logger(__name__)

NEWSAPI_BASE = "https://newsapi.org/v2"


class NewsAPISource(NewsSource):
    """
    News source using NewsAPI.org.

    Free tier: 100 requests/day
    """

    def __init__(self, api_key: str) -> None:
        """
        Initialize the NewsAPI source.

        Args:
            api_key: NewsAPI API key
        """
        self._api_key = api_key

    @property
    def name(self) -> str:
        return "newsapi"

    def is_configured(self) -> bool:
        return bool(self._api_key)

    async def fetch_headlines(self, max_results: int = 50) -> list[NewsArticle]:
        """
        Fetch top headlines from NewsAPI.

        Args:
            max_results: Maximum number of articles

        Returns:
            List of news articles
        """
        if not self.is_configured():
            logger.warning("NewsAPI not configured, skipping fetch")
            return []

        try:
            async with httpx.AsyncClient(timeout=30.0) as client:
                response = await client.get(
                    f"{NEWSAPI_BASE}/top-headlines",
                    params={
                        "apiKey": self._api_key,
                        "country": "us",
                        "pageSize": min(max_results, 100),
                    },
                )
                response.raise_for_status()
                data = response.json()

                if data.get("status") != "ok":
                    logger.error("NewsAPI error", message=data.get("message"))
                    return []

                articles = []
                for article_data in data.get("articles", []):
                    article = self._parse_article(article_data)
                    if article:
                        articles.append(article)

                logger.info(f"Fetched {len(articles)} headlines from NewsAPI")
                return articles

        except httpx.HTTPError as e:
            logger.error("Failed to fetch from NewsAPI", error=str(e))
            return []
        except Exception as e:
            logger.error("Unexpected error fetching from NewsAPI", error=str(e))
            return []

    async def fetch_news(
        self,
        query: str | None = None,
        max_results: int = 50,
    ) -> list[NewsArticle]:
        """
        Fetch news articles from NewsAPI.

        Args:
            query: Optional search query
            max_results: Maximum number of articles

        Returns:
            List of news articles
        """
        if not self.is_configured():
            logger.warning("NewsAPI not configured, skipping fetch")
            return []

        # If no query, just get headlines
        if not query:
            return await self.fetch_headlines(max_results)

        try:
            async with httpx.AsyncClient(timeout=30.0) as client:
                response = await client.get(
                    f"{NEWSAPI_BASE}/everything",
                    params={
                        "apiKey": self._api_key,
                        "q": query,
                        "language": "en",
                        "sortBy": "publishedAt",
                        "pageSize": min(max_results, 100),
                    },
                )
                response.raise_for_status()
                data = response.json()

                if data.get("status") != "ok":
                    logger.error("NewsAPI error", message=data.get("message"))
                    return []

                articles = []
                for article_data in data.get("articles", []):
                    article = self._parse_article(article_data)
                    if article:
                        articles.append(article)

                logger.info(f"Fetched {len(articles)} articles from NewsAPI for query: {query}")
                return articles

        except httpx.HTTPError as e:
            logger.error("Failed to fetch from NewsAPI", error=str(e))
            return []
        except Exception as e:
            logger.error("Unexpected error fetching from NewsAPI", error=str(e))
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
                title=data.get("title") or "",
                description=data.get("description") or "",
                content=data.get("content") or "",
                source_name=source.get("name") or "",
                source_id=source.get("id") or "",
                author=data.get("author") or "",
                published_at=published_at,
                image_url=data.get("urlToImage") or "",
            )
        except Exception as e:
            logger.warning("Failed to parse NewsAPI article", error=str(e))
            return None
