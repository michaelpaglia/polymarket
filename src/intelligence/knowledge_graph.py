"""
Knowledge Graph for Prediction Market Intelligence.

This module builds and maintains a knowledge graph that maps:
- Entities (people, companies, topics) → Markets they influence
- Influencers → What topics they move
- Historical patterns → What types of events moved which markets
- Relationships → Entity networks and dependencies

Research basis:
- Knowledge graphs + Deep Learning achieve 89% F1-score for sentiment (MDPI 2021)
- Twitter sentiment predicts market movements days in advance (Stanford, CEPR)
- Grok Live Search provides real-time X data unavailable in other models
"""

import json
from dataclasses import dataclass, field
from datetime import datetime, timezone
from typing import Optional
from pathlib import Path

from src.utils.logging import get_logger

logger = get_logger(__name__)


@dataclass
class Entity:
    """An entity that can influence prediction markets."""

    name: str
    entity_type: str  # "person", "company", "topic", "event"
    aliases: list[str] = field(default_factory=list)
    twitter_handles: list[str] = field(default_factory=list)
    influence_score: float = 0.5  # 0-1, how much this entity moves markets

    # Related markets (condition_ids)
    related_markets: list[str] = field(default_factory=list)

    # Keywords to search for
    search_keywords: list[str] = field(default_factory=list)


@dataclass
class Influencer:
    """A Twitter/X account that moves markets."""

    handle: str
    name: str
    follower_count: int = 0
    influence_score: float = 0.5  # 0-1

    # Topics this influencer impacts
    topics: list[str] = field(default_factory=list)

    # Markets this influencer has historically moved
    market_impact_history: list[dict] = field(default_factory=list)

    # How quickly their tweets move markets (minutes)
    avg_market_reaction_time: float = 30.0


@dataclass
class MarketEntity:
    """A market with its entity relationships."""

    condition_id: str
    question: str

    # Primary entities this market depends on
    primary_entities: list[str] = field(default_factory=list)  # Entity names

    # Secondary/related entities
    secondary_entities: list[str] = field(default_factory=list)

    # Influencers who have moved this market
    key_influencers: list[str] = field(default_factory=list)  # Twitter handles

    # Keywords that typically signal movement
    trigger_keywords: list[str] = field(default_factory=list)

    # Historical volatility patterns
    avg_daily_movement: float = 0.0
    news_sensitivity: float = 0.5  # 0-1, how reactive to news


class PredictionMarketKnowledgeGraph:
    """
    Knowledge graph for prediction market intelligence.

    Enables:
    1. Fast entity-to-market mapping (Who influences what?)
    2. Influencer tracking (What accounts move markets?)
    3. Keyword-to-market mapping (What news affects what?)
    4. Historical pattern recognition (What happened before?)
    """

    def __init__(self, persist_path: str = "data/knowledge_graph.json") -> None:
        self.persist_path = Path(persist_path)
        self.persist_path.parent.mkdir(parents=True, exist_ok=True)

        # Core graph data
        self.entities: dict[str, Entity] = {}
        self.influencers: dict[str, Influencer] = {}
        self.markets: dict[str, MarketEntity] = {}

        # Indexes for fast lookup
        self._keyword_to_markets: dict[str, set[str]] = {}
        self._entity_to_markets: dict[str, set[str]] = {}
        self._handle_to_influencer: dict[str, str] = {}

        # Load existing data
        self._load()

        # Initialize with seed data if empty
        if not self.entities:
            self._initialize_seed_data()

    def _initialize_seed_data(self) -> None:
        """Initialize with key entities for prediction markets."""

        # Political figures
        political_entities = [
            Entity(
                name="Donald Trump",
                entity_type="person",
                aliases=["Trump", "DJT", "45", "47"],
                twitter_handles=["@realDonaldTrump"],
                influence_score=0.95,
                search_keywords=["Trump", "MAGA", "GOP", "Republican"],
            ),
            Entity(
                name="Joe Biden",
                entity_type="person",
                aliases=["Biden", "POTUS", "46"],
                twitter_handles=["@POTUS", "@JoeBiden"],
                influence_score=0.9,
                search_keywords=["Biden", "Democrat", "White House"],
            ),
            Entity(
                name="RFK Jr",
                entity_type="person",
                aliases=["Robert Kennedy Jr", "RFK", "Kennedy"],
                twitter_handles=["@RobertKennedyJr"],
                influence_score=0.7,
                search_keywords=["RFK", "Kennedy", "vaccine", "HHS"],
            ),
            Entity(
                name="Elon Musk",
                entity_type="person",
                aliases=["Musk", "Elon"],
                twitter_handles=["@elonmusk"],
                influence_score=0.95,
                search_keywords=["Elon", "Musk", "Tesla", "SpaceX", "X", "DOGE"],
            ),
        ]

        # Key influencers
        key_influencers = [
            Influencer(
                handle="@elonmusk",
                name="Elon Musk",
                follower_count=200_000_000,
                influence_score=0.95,
                topics=["crypto", "tech", "politics", "Tesla", "SpaceX"],
                avg_market_reaction_time=5.0,  # Markets move within 5 minutes
            ),
            Influencer(
                handle="@realDonaldTrump",
                name="Donald Trump",
                follower_count=90_000_000,
                influence_score=0.95,
                topics=["politics", "elections", "policy"],
                avg_market_reaction_time=10.0,
            ),
            Influencer(
                handle="@WSJ",
                name="Wall Street Journal",
                follower_count=20_000_000,
                influence_score=0.8,
                topics=["finance", "economics", "politics", "business"],
                avg_market_reaction_time=15.0,
            ),
            Influencer(
                handle="@AP",
                name="Associated Press",
                follower_count=15_000_000,
                influence_score=0.85,
                topics=["breaking news", "politics", "world"],
                avg_market_reaction_time=10.0,
            ),
            Influencer(
                handle="@Politico",
                name="Politico",
                follower_count=5_000_000,
                influence_score=0.75,
                topics=["politics", "policy", "elections"],
                avg_market_reaction_time=20.0,
            ),
        ]

        # Topic entities
        topic_entities = [
            Entity(
                name="Federal Reserve",
                entity_type="topic",
                aliases=["Fed", "FOMC", "Jerome Powell"],
                twitter_handles=["@federalreserve"],
                influence_score=0.9,
                search_keywords=["Fed", "FOMC", "interest rate", "Powell", "monetary policy"],
            ),
            Entity(
                name="Bitcoin",
                entity_type="topic",
                aliases=["BTC", "crypto"],
                twitter_handles=[],
                influence_score=0.8,
                search_keywords=["Bitcoin", "BTC", "crypto", "cryptocurrency"],
            ),
            Entity(
                name="AI/Tech",
                entity_type="topic",
                aliases=["artificial intelligence", "OpenAI", "Google AI"],
                twitter_handles=["@OpenAI", "@GoogleAI", "@AnthropicAI"],
                influence_score=0.7,
                search_keywords=["AI", "GPT", "artificial intelligence", "AGI"],
            ),
        ]

        # Add all entities
        for entity in political_entities + topic_entities:
            self.add_entity(entity)

        for influencer in key_influencers:
            self.add_influencer(influencer)

        self._save()
        logger.info(f"Initialized knowledge graph with {len(self.entities)} entities, {len(self.influencers)} influencers")

    def add_entity(self, entity: Entity) -> None:
        """Add or update an entity."""
        self.entities[entity.name] = entity

        # Index keywords
        for keyword in entity.search_keywords:
            keyword_lower = keyword.lower()
            if keyword_lower not in self._keyword_to_markets:
                self._keyword_to_markets[keyword_lower] = set()

        # Index entity to markets
        self._entity_to_markets[entity.name] = set(entity.related_markets)

    def add_influencer(self, influencer: Influencer) -> None:
        """Add or update an influencer."""
        self.influencers[influencer.handle] = influencer
        self._handle_to_influencer[influencer.handle.lower()] = influencer.handle

    def add_market(self, market: MarketEntity) -> None:
        """Add or update a market with its entity relationships."""
        self.markets[market.condition_id] = market

        # Update entity-to-market mappings
        for entity_name in market.primary_entities + market.secondary_entities:
            if entity_name in self._entity_to_markets:
                self._entity_to_markets[entity_name].add(market.condition_id)
            else:
                self._entity_to_markets[entity_name] = {market.condition_id}

        # Update keyword-to-market mappings
        for keyword in market.trigger_keywords:
            keyword_lower = keyword.lower()
            if keyword_lower in self._keyword_to_markets:
                self._keyword_to_markets[keyword_lower].add(market.condition_id)
            else:
                self._keyword_to_markets[keyword_lower] = {market.condition_id}

    def find_markets_for_entity(self, entity_name: str) -> list[str]:
        """Find all markets related to an entity."""
        return list(self._entity_to_markets.get(entity_name, set()))

    def find_markets_for_keywords(self, text: str) -> list[tuple[str, float]]:
        """
        Find markets relevant to text based on keyword matching.
        Returns list of (condition_id, relevance_score) tuples.
        """
        text_lower = text.lower()
        market_scores: dict[str, float] = {}

        for keyword, market_ids in self._keyword_to_markets.items():
            if keyword in text_lower:
                for market_id in market_ids:
                    if market_id not in market_scores:
                        market_scores[market_id] = 0.0
                    market_scores[market_id] += 1.0

        # Normalize and sort
        if market_scores:
            max_score = max(market_scores.values())
            results = [(mid, score / max_score) for mid, score in market_scores.items()]
            results.sort(key=lambda x: x[1], reverse=True)
            return results

        return []

    def get_influencer(self, handle: str) -> Optional[Influencer]:
        """Get influencer by handle."""
        normalized = handle.lower()
        if normalized in self._handle_to_influencer:
            return self.influencers.get(self._handle_to_influencer[normalized])
        return None

    def get_priority_handles_for_topic(self, topic: str) -> list[str]:
        """Get Twitter handles to monitor for a topic."""
        handles = []
        topic_lower = topic.lower()

        for influencer in self.influencers.values():
            for inf_topic in influencer.topics:
                if topic_lower in inf_topic.lower() or inf_topic.lower() in topic_lower:
                    handles.append(influencer.handle)
                    break

        # Sort by influence score
        handles.sort(key=lambda h: self.influencers[h].influence_score, reverse=True)
        return handles

    def extract_entities_from_text(self, text: str) -> list[Entity]:
        """Extract known entities mentioned in text."""
        found = []
        text_lower = text.lower()

        for entity in self.entities.values():
            # Check name and aliases
            names_to_check = [entity.name.lower()] + [a.lower() for a in entity.aliases]
            for name in names_to_check:
                if name in text_lower:
                    found.append(entity)
                    break

        return found

    def get_search_queries_for_market(self, condition_id: str) -> list[str]:
        """Generate search queries to monitor for a market."""
        queries = []

        if condition_id in self.markets:
            market = self.markets[condition_id]

            # Add trigger keywords
            queries.extend(market.trigger_keywords[:5])

            # Add primary entity keywords
            for entity_name in market.primary_entities:
                if entity_name in self.entities:
                    entity = self.entities[entity_name]
                    queries.extend(entity.search_keywords[:3])

        return list(set(queries))[:10]  # Dedupe and limit

    def _save(self) -> None:
        """Persist graph to disk."""
        data = {
            "entities": {k: v.__dict__ for k, v in self.entities.items()},
            "influencers": {k: v.__dict__ for k, v in self.influencers.items()},
            "markets": {k: v.__dict__ for k, v in self.markets.items()},
        }
        with open(self.persist_path, "w") as f:
            json.dump(data, f, indent=2, default=str)

    def _load(self) -> None:
        """Load graph from disk."""
        if not self.persist_path.exists():
            return

        try:
            with open(self.persist_path) as f:
                data = json.load(f)

            for name, entity_data in data.get("entities", {}).items():
                self.entities[name] = Entity(**entity_data)

            for handle, inf_data in data.get("influencers", {}).items():
                self.influencers[handle] = Influencer(**inf_data)

            for cid, market_data in data.get("markets", {}).items():
                self.markets[cid] = MarketEntity(**market_data)

            # Rebuild indexes
            self._rebuild_indexes()

            logger.info(f"Loaded knowledge graph: {len(self.entities)} entities, {len(self.influencers)} influencers, {len(self.markets)} markets")

        except Exception as e:
            logger.error(f"Failed to load knowledge graph: {e}")

    def _rebuild_indexes(self) -> None:
        """Rebuild lookup indexes from loaded data."""
        self._keyword_to_markets = {}
        self._entity_to_markets = {}
        self._handle_to_influencer = {}

        for entity in self.entities.values():
            for keyword in entity.search_keywords:
                if keyword.lower() not in self._keyword_to_markets:
                    self._keyword_to_markets[keyword.lower()] = set()
            self._entity_to_markets[entity.name] = set(entity.related_markets)

        for influencer in self.influencers.values():
            self._handle_to_influencer[influencer.handle.lower()] = influencer.handle

        for market in self.markets.values():
            for entity_name in market.primary_entities + market.secondary_entities:
                if entity_name not in self._entity_to_markets:
                    self._entity_to_markets[entity_name] = set()
                self._entity_to_markets[entity_name].add(market.condition_id)

            for keyword in market.trigger_keywords:
                if keyword.lower() not in self._keyword_to_markets:
                    self._keyword_to_markets[keyword.lower()] = set()
                self._keyword_to_markets[keyword.lower()].add(market.condition_id)


def auto_map_markets_to_entities(kg: PredictionMarketKnowledgeGraph, markets: list) -> None:
    """
    Automatically map markets to entities using NLP extraction.

    This analyzes market questions to identify:
    - People mentioned (politicians, celebrities, etc.)
    - Topics mentioned (crypto, vaccines, elections)
    - Companies mentioned
    """
    for market in markets:
        # Extract entities from question
        entities = kg.extract_entities_from_text(market.question)

        if entities:
            market_entity = MarketEntity(
                condition_id=market.condition_id,
                question=market.question,
                primary_entities=[e.name for e in entities[:2]],
                secondary_entities=[e.name for e in entities[2:]],
                trigger_keywords=_extract_keywords(market.question),
            )
            kg.add_market(market_entity)


def _extract_keywords(text: str) -> list[str]:
    """Extract important keywords from text."""
    # Simple keyword extraction - could be enhanced with NLP
    stop_words = {"will", "the", "be", "in", "to", "a", "of", "for", "on", "by", "is", "at", "or", "an"}
    words = text.lower().replace("?", "").replace("'", "").split()
    keywords = [w for w in words if len(w) > 3 and w not in stop_words]
    return keywords[:10]
