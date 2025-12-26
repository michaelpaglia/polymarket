"""News data models."""

from datetime import datetime
from typing import Optional

from pydantic import BaseModel, Field


class NewsArticle(BaseModel):
    """
    Represents a news article from any source.

    Normalized format for consistent processing.
    """

    # Identifiers
    id: str = Field(default="")  # Unique ID (hash of URL or source-provided)
    url: str = ""

    # Content
    title: str
    description: str = ""
    content: str = ""  # Full article content if available

    # Source info
    source_name: str = ""
    source_id: str = ""  # Source identifier (e.g., "bbc-news", "cnn")
    author: str = ""

    # Metadata
    published_at: Optional[datetime] = None
    fetched_at: datetime = Field(default_factory=datetime.now)

    # Categorization
    category: str = ""
    keywords: list[str] = Field(default_factory=list)

    # Image
    image_url: str = ""

    @property
    def text_for_analysis(self) -> str:
        """Get combined text for LLM analysis."""
        parts = [self.title]
        if self.description:
            parts.append(self.description)
        if self.content:
            parts.append(self.content[:1000])  # Limit content length
        return " ".join(parts)

    @property
    def age_minutes(self) -> float:
        """Get article age in minutes."""
        if self.published_at is None:
            return 0
        delta = datetime.now() - self.published_at.replace(tzinfo=None)
        return delta.total_seconds() / 60

    def __hash__(self) -> int:
        """Hash based on URL for deduplication."""
        return hash(self.url)

    def __eq__(self, other: object) -> bool:
        """Equality based on URL."""
        if not isinstance(other, NewsArticle):
            return False
        return self.url == other.url
