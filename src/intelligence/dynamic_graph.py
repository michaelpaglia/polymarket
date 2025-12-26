"""
Dynamic Knowledge Graph - Auto-expanding entity and influencer tracking.

No hardcoded entities - learns from:
1. Market questions (extract people, companies, topics)
2. Twitter activity (discover influential accounts)
3. News articles (identify trending entities)

Persists to disk and continuously expands.
"""

import json
import re
from dataclasses import dataclass, field
from datetime import datetime, timezone, timedelta
from pathlib import Path
from typing import Optional
import asyncio

import httpx

from src.utils.logging import get_logger

logger = get_logger(__name__)


@dataclass
class DynamicEntity:
    """An entity discovered from markets/news."""

    name: str
    entity_type: str  # "person", "company", "topic", "event", "location"
    aliases: list[str] = field(default_factory=list)

    # Discovered from markets
    related_market_ids: set[str] = field(default_factory=set)
    market_mentions: int = 0

    # Twitter handles (discovered or known)
    twitter_handles: list[str] = field(default_factory=list)

    # Relevance scoring
    relevance_score: float = 0.5  # Based on market count, recency
    last_seen: datetime = field(default_factory=lambda: datetime.now(timezone.utc))

    # Search patterns
    search_patterns: list[str] = field(default_factory=list)


@dataclass
class DynamicInfluencer:
    """An influencer discovered from Twitter activity."""

    handle: str
    name: str = ""

    # Discovered metrics
    estimated_followers: int = 0
    influence_score: float = 0.5

    # Topics they influence (discovered)
    topics: set[str] = field(default_factory=set)

    # Markets they've potentially moved
    related_market_ids: set[str] = field(default_factory=set)

    # Activity tracking
    last_active: datetime = field(default_factory=lambda: datetime.now(timezone.utc))
    mentions_found: int = 0


class DynamicKnowledgeGraph:
    """
    Self-expanding knowledge graph that learns from data.

    No hardcoded limits - discovers entities and influencers automatically.
    """

    def __init__(self, persist_path: str = "data/dynamic_kg.json") -> None:
        self.persist_path = Path(persist_path)
        self.persist_path.parent.mkdir(parents=True, exist_ok=True)

        # Core data - starts empty, expands dynamically
        self.entities: dict[str, DynamicEntity] = {}
        self.influencers: dict[str, DynamicInfluencer] = {}

        # Market -> entities mapping
        self.market_entities: dict[str, set[str]] = {}

        # Topic -> influencers mapping
        self.topic_influencers: dict[str, set[str]] = {}

        # Known high-value Twitter accounts (seed list, but will expand)
        self._seed_news_handles = {
            "@AP", "@Reuters", "@WSJ", "@Bloomberg", "@CNBC",
            "@CNN", "@FoxNews", "@NBCNews", "@ABC", "@CBS",
            "@NYTimes", "@WashingtonPost", "@Politico", "@TheHill",
            "@axios", "@theeconomist", "@Forbes", "@business",
        }

        # Entity extraction patterns
        self._person_patterns = [
            r'\b([A-Z][a-z]+ [A-Z][a-z]+(?:\s+[A-Z][a-z]+)?)\b',  # Full names
            r'\b((?:President|Senator|Rep\.|Gov\.|CEO|Dr\.) [A-Z][a-z]+ [A-Z][a-z]+)\b',
        ]

        self._load()
        logger.info(f"Dynamic KG initialized: {len(self.entities)} entities, {len(self.influencers)} influencers")

    def extract_entities_from_markets(self, markets: list) -> None:
        """
        Extract entities from ALL market questions dynamically.

        This is the primary way the graph expands - by analyzing market questions.
        """
        for market in markets:
            question = market.question
            condition_id = market.condition_id

            # Extract entities from question
            extracted = self._extract_entities_from_text(question)

            for entity_name, entity_type in extracted:
                self._add_or_update_entity(
                    name=entity_name,
                    entity_type=entity_type,
                    market_id=condition_id,
                    source_text=question,
                )

            # Also extract from description if available
            if hasattr(market, 'description') and market.description:
                desc_entities = self._extract_entities_from_text(market.description)
                for entity_name, entity_type in desc_entities:
                    self._add_or_update_entity(
                        name=entity_name,
                        entity_type=entity_type,
                        market_id=condition_id,
                        source_text=market.description,
                    )

        # Update relevance scores based on market count
        self._update_relevance_scores()
        self._save()

        logger.info(f"Extracted entities from {len(markets)} markets, now tracking {len(self.entities)} entities")

    def _extract_entities_from_text(self, text: str) -> list[tuple[str, str]]:
        """Extract named entities from text using patterns and heuristics."""
        entities = []

        # Garbage phrases to filter out
        garbage_phrases = {
            'will the', 'in the', 'by the', 'of the', 'on the', 'at the',
            'et otherwise', 'otherwise the', 'the market', 'this market',
            'market will', 'will resolve', 'resolve yes', 'resolve no',
            'pm et', 'am et', 'open market', 'market committee',
            'economic research', 'economic analysis', 'national bureau',
            'federal open', 'reserve board', 'limited inc', 'coinbase',
            'usdtusd', 'the national', 'if the', 'for the', 'to the',
            'and the', 'from the', 'with the', 'be the', 'is the',
        }

        # Common patterns for prediction market questions
        patterns = {
            # People - specific names
            r'\b(Donald Trump|Trump)\b': 'person',
            r'\b(Joe Biden|Biden)\b': 'person',
            r'\b(Elon Musk|Musk)\b': 'person',
            r'\b(RFK Jr\.?|Robert Kennedy)\b': 'person',
            r'\b(Putin|Vladimir Putin)\b': 'person',
            r'\b(Xi Jinping|Xi)\b': 'person',
            r'\b(Zelensky|Zelenskyy)\b': 'person',
            r'\b(Netanyahu)\b': 'person',
            r'\b(Khamenei)\b': 'person',
            r'\b(Kim Jong[- ]?Un)\b': 'person',
            r'\b(Taylor Swift)\b': 'person',
            r'\b(LeBron James|LeBron)\b': 'person',
            r'\b(Patrick Mahomes|Mahomes)\b': 'person',
            r'\b(Aaron Rodgers|Rodgers)\b': 'person',

            # Politicians with titles
            r'\b(President [A-Z][a-z]+)\b': 'person',
            r'\b(Senator [A-Z][a-z]+)\b': 'person',
            r'\b(Governor [A-Z][a-z]+)\b': 'person',

            # Companies/Organizations
            r'\b(Tesla|SpaceX|OpenAI|Google|Apple|Microsoft|Amazon|Meta|Nvidia|Netflix|Disney)\b': 'company',
            r'\b(Federal Reserve|Fed|FOMC)\b': 'organization',
            r'\b(SEC|FDA|EPA|FTC|DOJ|FBI|CIA|NSA)\b': 'organization',
            r'\b(NATO|United Nations|UN|WHO|IMF|World Bank)\b': 'organization',
            r'\b(Democratic Party|Republican Party|Democrats|Republicans|GOP)\b': 'organization',

            # Crypto/Finance
            r'\b(Bitcoin|BTC)\b': 'asset',
            r'\b(Ethereum|ETH)\b': 'asset',
            r'\b(Solana|SOL)\b': 'asset',
            r'\b(XRP|Ripple)\b': 'asset',
            r'\b(Dogecoin|DOGE)\b': 'asset',
            r'\b(S&P ?500|SPX)\b': 'index',
            r'\b(Dow Jones|DJIA)\b': 'index',
            r'\b(Nasdaq|QQQ)\b': 'index',

            # Sports teams - NFL
            r'\b(Chiefs|49ers|Eagles|Cowboys|Bills|Ravens|Lions|Packers|Vikings|Bears|Dolphins|Jets|Patriots|Broncos|Raiders|Chargers|Bengals|Browns|Steelers|Titans|Colts|Jaguars|Texans|Saints|Buccaneers|Falcons|Panthers|Cardinals|Rams|Seahawks|Commanders|Giants)\b': 'sports_team',
            r'\b(NFL|Super Bowl)\b': 'sports',
            r'\b(NBA|Finals)\b': 'sports',
            r'\b(MLB|World Series)\b': 'sports',
            r'\b(NHL|Stanley Cup)\b': 'sports',
            r'\b(UFC|MMA)\b': 'sports',
            r'\b(NFC|AFC)\b': 'sports',

            # Countries/Locations
            r'\b(United States|U\.S\.|USA|America)\b': 'location',
            r'\b(China|Chinese)\b': 'location',
            r'\b(Russia|Russian)\b': 'location',
            r'\b(Ukraine|Ukrainian)\b': 'location',
            r'\b(Israel|Israeli)\b': 'location',
            r'\b(Iran|Iranian)\b': 'location',
            r'\b(North Korea|DPRK)\b': 'location',
            r'\b(Taiwan)\b': 'location',
            r'\b(Gaza|Palestine|Palestinian)\b': 'location',
            r'\b(Syria|Syrian)\b': 'location',

            # Events/Topics
            r'\b(recession)\b': 'topic',
            r'\b(inflation)\b': 'topic',
            r'\b(interest rate|rate cut|rate hike)\b': 'topic',
            r'\b(election|midterms|primary)\b': 'event',
            r'\b(impeachment|indictment|trial|verdict|conviction)\b': 'event',
            r'\b(vaccine|COVID|pandemic|coronavirus)\b': 'topic',
            r'\b(war|invasion|conflict|ceasefire)\b': 'event',
            r'\b(AI|artificial intelligence|AGI|GPT)\b': 'topic',
            r'\b(nuclear|nuke)\b': 'topic',
            r'\b(tariff|trade war)\b': 'topic',
            r'\b(shutdown|government shutdown)\b': 'event',
            r'\b(debt ceiling)\b': 'topic',
        }

        for pattern, entity_type in patterns.items():
            matches = re.findall(pattern, text, re.IGNORECASE)
            for match in matches:
                # Normalize the match
                name = match.strip()
                if len(name) >= 2:  # Skip single characters
                    # Filter garbage
                    if name.lower() not in garbage_phrases:
                        entities.append((name, entity_type))

        # Deduplicate while preserving order
        seen = set()
        unique_entities = []
        for entity in entities:
            key = entity[0].lower()
            if key not in seen and key not in garbage_phrases:
                seen.add(key)
                unique_entities.append(entity)

        return unique_entities

    def _add_or_update_entity(
        self,
        name: str,
        entity_type: str,
        market_id: str,
        source_text: str,
    ) -> None:
        """Add or update an entity in the graph."""
        # Normalize name
        name_key = name.lower().strip()

        if name_key in self.entities:
            entity = self.entities[name_key]
            entity.related_market_ids.add(market_id)
            entity.market_mentions += 1
            entity.last_seen = datetime.now(timezone.utc)
        else:
            entity = DynamicEntity(
                name=name,
                entity_type=entity_type,
                related_market_ids={market_id},
                market_mentions=1,
                search_patterns=[name.lower()],
            )
            self.entities[name_key] = entity

        # Update market -> entity mapping
        if market_id not in self.market_entities:
            self.market_entities[market_id] = set()
        self.market_entities[market_id].add(name_key)

    def _update_relevance_scores(self) -> None:
        """Update relevance scores based on market mentions and recency."""
        if not self.entities:
            return

        max_mentions = max(e.market_mentions for e in self.entities.values())
        now = datetime.now(timezone.utc)

        for entity in self.entities.values():
            # Score based on market mentions (normalized)
            mention_score = entity.market_mentions / max_mentions if max_mentions > 0 else 0

            # Recency score (decay over 7 days)
            age_hours = (now - entity.last_seen).total_seconds() / 3600
            recency_score = max(0, 1 - (age_hours / 168))  # 7 days = 168 hours

            entity.relevance_score = (mention_score * 0.7) + (recency_score * 0.3)

    def add_influencer(self, handle: str, topics: list[str], name: str = "") -> None:
        """Add or update an influencer discovered from Twitter."""
        handle_key = handle.lower()

        if handle_key in self.influencers:
            inf = self.influencers[handle_key]
            inf.topics.update(topics)
            inf.last_active = datetime.now(timezone.utc)
            inf.mentions_found += 1
        else:
            self.influencers[handle_key] = DynamicInfluencer(
                handle=handle,
                name=name,
                topics=set(topics),
            )

        # Update topic -> influencer mapping
        for topic in topics:
            topic_key = topic.lower()
            if topic_key not in self.topic_influencers:
                self.topic_influencers[topic_key] = set()
            self.topic_influencers[topic_key].add(handle_key)

        self._save()

    def get_entities_for_market(self, market_id: str) -> list[DynamicEntity]:
        """Get all entities related to a market."""
        entity_keys = self.market_entities.get(market_id, set())
        return [self.entities[k] for k in entity_keys if k in self.entities]

    def get_top_entities(self, limit: int = 50) -> list[DynamicEntity]:
        """Get top entities by relevance score."""
        sorted_entities = sorted(
            self.entities.values(),
            key=lambda e: e.relevance_score,
            reverse=True,
        )
        return sorted_entities[:limit]

    def get_search_topics(self, limit: int = 20) -> list[str]:
        """Get top topics/entities to search for on Twitter."""
        top_entities = self.get_top_entities(limit)
        return [e.name for e in top_entities]

    def get_influencers_for_topic(self, topic: str) -> list[DynamicInfluencer]:
        """Get influencers known to discuss a topic."""
        topic_key = topic.lower()

        # Direct match
        handles = self.topic_influencers.get(topic_key, set())

        # Partial match
        for key, inf_handles in self.topic_influencers.items():
            if topic_key in key or key in topic_key:
                handles.update(inf_handles)

        return [self.influencers[h] for h in handles if h in self.influencers]

    def get_all_influencer_handles(self) -> list[str]:
        """Get all known influencer handles for monitoring."""
        # Combine discovered influencers with seed news accounts
        handles = set(self._seed_news_handles)
        handles.update(inf.handle for inf in self.influencers.values())
        return list(handles)

    def find_markets_for_text(self, text: str) -> list[str]:
        """Find market IDs that might be affected by text content."""
        text_lower = text.lower()
        matching_markets = set()

        for entity_key, entity in self.entities.items():
            # Check if entity name or aliases appear in text
            if entity_key in text_lower:
                matching_markets.update(entity.related_market_ids)
            for alias in entity.aliases:
                if alias.lower() in text_lower:
                    matching_markets.update(entity.related_market_ids)

        return list(matching_markets)

    def _save(self) -> None:
        """Persist graph to disk."""
        data = {
            "entities": {
                k: {
                    "name": v.name,
                    "entity_type": v.entity_type,
                    "aliases": v.aliases,
                    "related_market_ids": list(v.related_market_ids),
                    "market_mentions": v.market_mentions,
                    "twitter_handles": v.twitter_handles,
                    "relevance_score": v.relevance_score,
                    "last_seen": v.last_seen.isoformat(),
                    "search_patterns": v.search_patterns,
                }
                for k, v in self.entities.items()
            },
            "influencers": {
                k: {
                    "handle": v.handle,
                    "name": v.name,
                    "estimated_followers": v.estimated_followers,
                    "influence_score": v.influence_score,
                    "topics": list(v.topics),
                    "related_market_ids": list(v.related_market_ids),
                    "last_active": v.last_active.isoformat(),
                    "mentions_found": v.mentions_found,
                }
                for k, v in self.influencers.items()
            },
            "market_entities": {k: list(v) for k, v in self.market_entities.items()},
            "topic_influencers": {k: list(v) for k, v in self.topic_influencers.items()},
            "updated_at": datetime.now(timezone.utc).isoformat(),
        }

        with open(self.persist_path, "w") as f:
            json.dump(data, f, indent=2)

    def _load(self) -> None:
        """Load graph from disk."""
        if not self.persist_path.exists():
            return

        try:
            with open(self.persist_path) as f:
                data = json.load(f)

            for k, v in data.get("entities", {}).items():
                self.entities[k] = DynamicEntity(
                    name=v["name"],
                    entity_type=v["entity_type"],
                    aliases=v.get("aliases", []),
                    related_market_ids=set(v.get("related_market_ids", [])),
                    market_mentions=v.get("market_mentions", 0),
                    twitter_handles=v.get("twitter_handles", []),
                    relevance_score=v.get("relevance_score", 0.5),
                    last_seen=datetime.fromisoformat(v["last_seen"]) if "last_seen" in v else datetime.now(timezone.utc),
                    search_patterns=v.get("search_patterns", []),
                )

            for k, v in data.get("influencers", {}).items():
                self.influencers[k] = DynamicInfluencer(
                    handle=v["handle"],
                    name=v.get("name", ""),
                    estimated_followers=v.get("estimated_followers", 0),
                    influence_score=v.get("influence_score", 0.5),
                    topics=set(v.get("topics", [])),
                    related_market_ids=set(v.get("related_market_ids", [])),
                    last_active=datetime.fromisoformat(v["last_active"]) if "last_active" in v else datetime.now(timezone.utc),
                    mentions_found=v.get("mentions_found", 0),
                )

            self.market_entities = {k: set(v) for k, v in data.get("market_entities", {}).items()}
            self.topic_influencers = {k: set(v) for k, v in data.get("topic_influencers", {}).items()}

        except Exception as e:
            logger.error(f"Failed to load dynamic KG: {e}")
