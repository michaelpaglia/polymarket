"""Vector embeddings and database for market semantic search."""

from pathlib import Path
from typing import Any, Optional, cast

import chromadb
from chromadb.config import Settings as ChromaSettings
from sentence_transformers import SentenceTransformer

from src.markets.models import Market
from src.utils.logging import get_logger

logger = get_logger(__name__)

# Default embedding model - good balance of quality and speed
DEFAULT_MODEL = "all-MiniLM-L6-v2"  # Fast, 384 dimensions
# Alternative: "BAAI/bge-large-en-v1.5" for higher quality


class MarketEmbeddings:
    """
    Manages vector embeddings for Polymarket markets.

    Uses ChromaDB for storage and sentence-transformers for embeddings.
    """

    def __init__(
        self,
        persist_directory: str = "data/chroma",
        model_name: str = DEFAULT_MODEL,
    ) -> None:
        """
        Initialize the embeddings manager.

        Args:
            persist_directory: Directory for ChromaDB persistence
            model_name: Sentence transformer model to use
        """
        self.persist_directory = Path(persist_directory)
        self.model_name = model_name

        # Create persist directory
        self.persist_directory.mkdir(parents=True, exist_ok=True)

        # Initialize embedding model
        logger.info(f"Loading embedding model: {model_name}")
        self._model = SentenceTransformer(model_name)

        # Initialize ChromaDB
        self._client = chromadb.PersistentClient(
            path=str(self.persist_directory),
            settings=ChromaSettings(
                anonymized_telemetry=False,
            ),
        )

        # Get or create collection
        self._collection = self._client.get_or_create_collection(
            name="polymarket_markets",
            metadata={"hnsw:space": "cosine"},  # Use cosine similarity
        )

        logger.info(
            "MarketEmbeddings initialized",
            persist_directory=str(self.persist_directory),
            model=model_name,
            collection_count=self._collection.count(),
        )

    def embed_text(self, text: str) -> list[float]:
        """
        Generate embedding for text.

        Args:
            text: Text to embed

        Returns:
            Embedding vector
        """
        return self._model.encode(text, convert_to_tensor=False).tolist()

    def embed_texts(self, texts: list[str]) -> list[list[float]]:
        """
        Generate embeddings for multiple texts.

        Args:
            texts: List of texts to embed

        Returns:
            List of embedding vectors
        """
        embeddings = self._model.encode(texts, convert_to_tensor=False)
        return [e.tolist() for e in embeddings]

    def index_markets(self, markets: list[Market]) -> int:
        """
        Index markets into the vector database.

        Args:
            markets: List of markets to index

        Returns:
            Number of markets indexed
        """
        if not markets:
            return 0

        # Clear existing collection
        self._client.delete_collection("polymarket_markets")
        self._collection = self._client.create_collection(
            name="polymarket_markets",
            metadata={"hnsw:space": "cosine"},
        )

        # Prepare data for indexing
        ids = []
        documents = []
        metadatas = []
        embeddings = []

        for market in markets:
            # Generate embedding text
            embed_text = market.embedding_text

            ids.append(market.condition_id)
            documents.append(embed_text)
            metadatas.append({
                "question": market.question,
                "category": market.category,
                "liquidity": market.liquidity,
                "yes_token_id": market.yes_token_id or "",
                "no_token_id": market.no_token_id or "",
                "yes_price": market.yes_price,
                "no_price": market.no_price,
                "days_to_resolution": market.days_to_resolution or -1,
            })

        # Generate embeddings in batch
        logger.info(f"Generating embeddings for {len(documents)} markets...")
        embeddings = self.embed_texts(documents)

        # Add to collection - cast embeddings for chromadb compatibility
        self._collection.add(
            ids=ids,
            documents=documents,
            metadatas=metadatas,  # type: ignore[arg-type]
            embeddings=cast(Any, embeddings),
        )

        logger.info(f"Indexed {len(markets)} markets into vector database")
        return len(markets)

    def search(
        self,
        query: str,
        n_results: int = 5,
        min_liquidity: Optional[float] = None,
    ) -> list[dict]:
        """
        Search for markets similar to a query.

        Args:
            query: Query text (news headline, etc.)
            n_results: Number of results to return
            min_liquidity: Minimum liquidity filter

        Returns:
            List of search results with scores
        """
        # Generate query embedding
        query_embedding = self.embed_text(query)

        # Build where clause for filtering
        where_clause: dict[str, Any] | None = None
        if min_liquidity is not None:
            where_clause = {"liquidity": {"$gte": min_liquidity}}

        # Search
        results = self._collection.query(
            query_embeddings=[query_embedding],
            n_results=n_results,
            where=where_clause,  # type: ignore[arg-type]
            include=["documents", "metadatas", "distances"],
        )

        # Format results
        formatted = []
        if results and results["ids"]:
            for i, condition_id in enumerate(results["ids"][0]):
                # Convert distance to similarity (cosine distance to similarity)
                distance = results["distances"][0][i] if results["distances"] else 0
                similarity = 1 - distance  # Cosine similarity

                formatted.append({
                    "condition_id": condition_id,
                    "document": results["documents"][0][i] if results["documents"] else "",
                    "metadata": results["metadatas"][0][i] if results["metadatas"] else {},
                    "similarity": similarity,
                })

        return formatted

    def get_collection_count(self) -> int:
        """Get the number of indexed markets."""
        return self._collection.count()

    def clear(self) -> None:
        """Clear all indexed markets."""
        self._client.delete_collection("polymarket_markets")
        self._collection = self._client.create_collection(
            name="polymarket_markets",
            metadata={"hnsw:space": "cosine"},
        )
        logger.info("Market embeddings cleared")
