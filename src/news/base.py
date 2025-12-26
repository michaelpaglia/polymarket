"""Abstract base class for news sources."""

from abc import ABC, abstractmethod

from src.news.models import NewsArticle


class NewsSource(ABC):
    """
    Abstract base class for news data sources.

    All news sources should inherit from this class.
    """

    @property
    @abstractmethod
    def name(self) -> str:
        """Get the source name."""
        pass

    @abstractmethod
    async def fetch_headlines(self, max_results: int = 50) -> list[NewsArticle]:
        """
        Fetch top headlines.

        Args:
            max_results: Maximum number of articles to fetch

        Returns:
            List of news articles
        """
        pass

    @abstractmethod
    async def fetch_news(
        self,
        query: str | None = None,
        max_results: int = 50,
    ) -> list[NewsArticle]:
        """
        Fetch news articles.

        Args:
            query: Optional search query
            max_results: Maximum number of articles to fetch

        Returns:
            List of news articles
        """
        pass

    @abstractmethod
    def is_configured(self) -> bool:
        """Check if the source is properly configured."""
        pass
