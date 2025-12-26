"""News aggregator that combines multiple news sources."""

import asyncio
from datetime import datetime
from difflib import SequenceMatcher
from typing import Optional

from src.config import NewsSettings
from src.news.base import NewsSource
from src.news.gnews import GNewsSource
from src.news.grok import GrokNewsSource
from src.news.models import NewsArticle
from src.news.newsapi import NewsAPISource
from src.utils.logging import get_logger

logger = get_logger(__name__)


class NewsAggregator:
    """
    Aggregates news from multiple sources.

    Features:
    - Combines results from multiple news APIs
    - Deduplicates similar articles
    - Tracks seen articles to avoid reprocessing
    """

    def __init__(self, settings: NewsSettings) -> None:
        """
        Initialize the news aggregator.

        Args:
            settings: News configuration settings
        """
        self.settings = settings
        self._sources: list[NewsSource] = []
        self._seen_urls: set[str] = set()
        self._last_fetch: Optional[datetime] = None

        # Initialize sources
        self._init_sources()

    def _init_sources(self) -> None:
        """Initialize configured news sources."""
        if "newsapi" in self.settings.sources and self.settings.newsapi_key:
            self._sources.append(NewsAPISource(self.settings.newsapi_key))
            logger.info("Initialized NewsAPI source")

        if "gnews" in self.settings.sources and self.settings.gnews_api_key:
            self._sources.append(GNewsSource(self.settings.gnews_api_key))
            logger.info("Initialized GNews source")

        if "grok" in self.settings.sources and self.settings.grok_api_key:
            self._sources.append(GrokNewsSource(self.settings.grok_api_key))
            logger.info("Initialized Grok/X source")

        if not self._sources:
            logger.warning("No news sources configured!")

    @property
    def sources(self) -> list[NewsSource]:
        """Get configured sources."""
        return self._sources

    def add_source(self, source: NewsSource) -> None:
        """Add a news source."""
        if source.is_configured():
            self._sources.append(source)
            logger.info(f"Added news source: {source.name}")

    async def fetch_all(self, max_per_source: int = 50) -> list[NewsArticle]:
        """
        Fetch news from all sources.

        Args:
            max_per_source: Maximum articles per source

        Returns:
            Combined, deduplicated list of articles
        """
        if not self._sources:
            logger.warning("No news sources available")
            return []

        # Fetch from all sources concurrently
        tasks = [
            source.fetch_headlines(max_per_source)
            for source in self._sources
        ]

        results = await asyncio.gather(*tasks, return_exceptions=True)

        # Combine results
        all_articles: list[NewsArticle] = []
        for i, result in enumerate(results):
            if isinstance(result, BaseException):
                logger.error(
                    f"Error fetching from {self._sources[i].name}",
                    error=str(result),
                )
                continue
            # result is now known to be list[NewsArticle]
            all_articles.extend(result)

        # Deduplicate
        unique_articles = self._deduplicate(all_articles)

        # Filter out seen articles
        new_articles = [
            a for a in unique_articles
            if a.url not in self._seen_urls
        ]

        # Mark as seen
        for article in new_articles:
            self._seen_urls.add(article.url)

        # Limit seen URLs to prevent memory issues
        if len(self._seen_urls) > 10000:
            self._seen_urls = set(list(self._seen_urls)[-5000:])

        self._last_fetch = datetime.now()

        logger.info(
            f"Fetched {len(all_articles)} total, {len(unique_articles)} unique, {len(new_articles)} new articles"
        )

        return new_articles

    def _deduplicate(self, articles: list[NewsArticle]) -> list[NewsArticle]:
        """
        Remove duplicate or very similar articles.

        Args:
            articles: List of articles to deduplicate

        Returns:
            Deduplicated list
        """
        if not articles:
            return []

        unique = []
        seen_titles: list[str] = []

        for article in articles:
            # Check for duplicate URLs
            if any(a.url == article.url for a in unique):
                continue

            # Check for similar titles
            is_similar = False
            for seen_title in seen_titles:
                similarity = SequenceMatcher(
                    None,
                    article.title.lower(),
                    seen_title.lower(),
                ).ratio()
                if similarity > 0.8:  # 80% similar
                    is_similar = True
                    break

            if not is_similar:
                unique.append(article)
                seen_titles.append(article.title)

        return unique

    def needs_fetch(self) -> bool:
        """Check if it's time to fetch new articles."""
        if self._last_fetch is None:
            return True

        elapsed = (datetime.now() - self._last_fetch).total_seconds()
        return elapsed >= self.settings.poll_interval_seconds

    def clear_seen(self) -> None:
        """Clear the seen articles cache."""
        self._seen_urls.clear()
        logger.info("Cleared seen articles cache")

    @property
    def last_fetch(self) -> Optional[datetime]:
        """Get the last fetch timestamp."""
        return self._last_fetch
