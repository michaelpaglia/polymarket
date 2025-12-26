"""News ingestion components."""

from src.news.aggregator import NewsAggregator
from src.news.base import NewsSource
from src.news.gnews import GNewsSource
from src.news.models import NewsArticle
from src.news.newsapi import NewsAPISource

__all__ = [
    "NewsAggregator",
    "NewsArticle",
    "NewsSource",
    "NewsAPISource",
    "GNewsSource",
]
