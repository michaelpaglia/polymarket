"""Market matcher for matching news to relevant Polymarket markets."""

import json
from typing import Any

from google import genai  # New package
from google.genai import types

from src.config import LLMSettings, MarketSettings
from src.markets.embeddings import MarketEmbeddings
from src.markets.models import Market, MarketMatch
from src.news.models import NewsArticle
from src.utils.logging import get_logger

logger = get_logger(__name__)


# Prompt for LLM market relevance validation
RELEVANCE_PROMPT = """You are analyzing whether a news article is relevant to prediction markets.

NEWS ARTICLE:
Title: {title}
Description: {description}
Content: {content}

CANDIDATE MARKETS:
{markets}

For each market, determine:
1. Is this news article directly relevant to predicting the outcome of this market?
2. How confident are you that this news impacts the probability of the market outcome?

Respond in JSON format:
{{
    "matches": [
        {{
            "market_id": "condition_id here",
            "is_relevant": true/false,
            "confidence": 0.0-1.0,
            "reasoning": "Brief explanation"
        }}
    ]
}}

Only include markets where is_relevant is true. Be strict - only mark as relevant if the news directly impacts the market outcome."""


class MarketMatcher:
    """
    Matches news articles to relevant Polymarket markets.

    Uses a two-stage approach:
    1. Vector similarity search (fast, cheap)
    2. LLM validation (accurate, validates relevance)
    """

    def __init__(
        self,
        embeddings: MarketEmbeddings,
        markets: dict[str, Market],
        llm_settings: LLMSettings,
        market_settings: MarketSettings,
    ) -> None:
        """
        Initialize the market matcher.

        Args:
            embeddings: Market embeddings database
            markets: Dictionary of markets by condition_id
            llm_settings: LLM configuration
            market_settings: Market matching configuration
        """
        self.embeddings = embeddings
        self.markets = markets
        self.llm_settings = llm_settings
        self.market_settings = market_settings
        self._client: genai.Client | None = None

        # Configure Gemini
        if llm_settings.google_api_key:
            self._client = genai.Client(api_key=llm_settings.google_api_key)
            logger.info(f"Initialized Gemini model: {llm_settings.model}")
        else:
            logger.warning("No Google API key configured, LLM validation disabled")

    async def match(self, article: NewsArticle) -> list[MarketMatch]:
        """
        Find markets relevant to a news article.

        Args:
            article: News article to match

        Returns:
            List of matched markets with relevance scores
        """
        # Stage 1: Vector similarity search
        candidates = self._vector_search(article)

        if not candidates:
            logger.debug("No candidate markets found for article", title=article.title)
            return []

        # Stage 2: LLM validation (if configured)
        if self._client:
            matches = await self._llm_validate(article, candidates)
        else:
            # Without LLM, use vector similarity scores directly
            matches = self._create_matches_from_candidates(candidates)

        logger.info(
            f"Matched article to {len(matches)} markets",
            title=article.title,
            markets=[m.market.question[:50] for m in matches],
        )

        return matches

    def _vector_search(self, article: NewsArticle) -> list[dict[str, Any]]:
        """
        Perform vector similarity search.

        Args:
            article: News article

        Returns:
            List of candidate markets with similarity scores
        """
        # Use title + description for search
        query = f"{article.title} {article.description}"

        results = self.embeddings.search(
            query=query,
            n_results=self.market_settings.top_k_matches,
            min_liquidity=self.market_settings.min_liquidity_usd,
        )

        return results

    async def _llm_validate(
        self,
        article: NewsArticle,
        candidates: list[dict[str, Any]],
    ) -> list[MarketMatch]:
        """
        Validate market matches using LLM.

        Args:
            article: News article
            candidates: Candidate markets from vector search

        Returns:
            Validated market matches
        """
        if not self._client:
            return []

        # Format markets for prompt
        markets_text = []
        for i, candidate in enumerate(candidates):
            market = self.markets.get(candidate["condition_id"])
            if market:
                markets_text.append(
                    f"{i+1}. [{candidate['condition_id']}] {market.question}"
                )

        if not markets_text:
            return []

        # Create prompt
        prompt = RELEVANCE_PROMPT.format(
            title=article.title,
            description=article.description,
            content=article.content[:500] if article.content else "",
            markets="\n".join(markets_text),
        )

        try:
            # Call Gemini 3
            response = self._client.models.generate_content(
                model=self.llm_settings.model,
                contents=prompt,
                config=types.GenerateContentConfig(
                    temperature=0.1,
                    response_mime_type="application/json",
                ),
            )

            # Parse response
            response_text = response.text or "{}"
            result: dict[str, Any] = json.loads(response_text)
            matches = []

            for match_data in result.get("matches", []):
                condition_id = match_data.get("market_id")
                market = self.markets.get(condition_id)

                if not market:
                    continue

                # Find vector similarity score
                vector_score = 0.0
                for candidate in candidates:
                    if candidate["condition_id"] == condition_id:
                        vector_score = candidate.get("similarity", 0.0)
                        break

                matches.append(
                    MarketMatch(
                        market=market,
                        similarity_score=vector_score,
                        llm_confidence=match_data.get("confidence", 0.0),
                        llm_reasoning=match_data.get("reasoning", ""),
                    )
                )

            # Sort by combined score
            matches.sort(key=lambda m: m.combined_score, reverse=True)

            return matches

        except Exception as e:
            logger.error("LLM validation failed", error=str(e))
            # Fall back to vector-only matches
            return self._create_matches_from_candidates(candidates)

    def _create_matches_from_candidates(
        self,
        candidates: list[dict[str, Any]],
    ) -> list[MarketMatch]:
        """
        Create MarketMatch objects from vector search results.

        Args:
            candidates: Vector search results

        Returns:
            List of MarketMatch objects
        """
        matches = []

        for candidate in candidates:
            market = self.markets.get(candidate["condition_id"])
            if not market:
                continue

            matches.append(
                MarketMatch(
                    market=market,
                    similarity_score=candidate.get("similarity", 0.0),
                    llm_confidence=0.0,  # No LLM validation
                    llm_reasoning="Vector similarity match (no LLM validation)",
                )
            )

        return matches

    def update_markets(self, markets: dict[str, Market]) -> None:
        """
        Update the markets dictionary.

        Args:
            markets: New markets dictionary
        """
        self.markets = markets
        logger.info(f"Updated matcher with {len(markets)} markets")
