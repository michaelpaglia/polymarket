"""Vector embeddings and database for market semantic search."""

import json
import pickle
from pathlib import Path
from typing import Any, Optional

import faiss
import numpy as np
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

    Uses FAISS for vector search and sentence-transformers for embeddings.
    """

    def __init__(
        self,
        persist_directory: str = "data/faiss",
        model_name: str = DEFAULT_MODEL,
    ) -> None:
        """
        Initialize the embeddings manager.

        Args:
            persist_directory: Directory for FAISS persistence
            model_name: Sentence transformer model to use
        """
        self.persist_directory = Path(persist_directory)
        self.model_name = model_name

        # Create persist directory
        self.persist_directory.mkdir(parents=True, exist_ok=True)

        # Initialize embedding model
        logger.info(f"Loading embedding model: {model_name}")
        self._model = SentenceTransformer(model_name)
        self._dimension = self._model.get_sentence_embedding_dimension()

        # Initialize FAISS index (cosine similarity via inner product on normalized vectors)
        self._index: Optional[faiss.IndexFlatIP] = None
        self._documents: list[str] = []
        self._metadatas: list[dict[str, Any]] = []
        self._ids: list[str] = []

        # Try to load existing index
        self._load_index()

        logger.info(
            "MarketEmbeddings initialized",
            persist_directory=str(self.persist_directory),
            model=model_name,
            collection_count=len(self._ids),
        )

    def _load_index(self) -> None:
        """Load existing index from disk if available."""
        index_path = self.persist_directory / "index.faiss"
        meta_path = self.persist_directory / "metadata.pkl"

        if index_path.exists() and meta_path.exists():
            try:
                self._index = faiss.read_index(str(index_path))
                with open(meta_path, "rb") as f:
                    data = pickle.load(f)
                    self._ids = data["ids"]
                    self._documents = data["documents"]
                    self._metadatas = data["metadatas"]
                logger.info(f"Loaded {len(self._ids)} markets from disk")
            except Exception as e:
                logger.warning(f"Failed to load index: {e}")
                self._index = None

    def _save_index(self) -> None:
        """Save index to disk."""
        if self._index is None:
            return

        index_path = self.persist_directory / "index.faiss"
        meta_path = self.persist_directory / "metadata.pkl"

        faiss.write_index(self._index, str(index_path))
        with open(meta_path, "wb") as f:
            pickle.dump({
                "ids": self._ids,
                "documents": self._documents,
                "metadatas": self._metadatas,
            }, f)

    def embed_text(self, text: str) -> list[float]:
        """
        Generate embedding for text.

        Args:
            text: Text to embed

        Returns:
            Embedding vector
        """
        embedding = self._model.encode(text, convert_to_tensor=False, normalize_embeddings=True)
        return embedding.tolist()

    def embed_texts(self, texts: list[str]) -> np.ndarray:
        """
        Generate embeddings for multiple texts.

        Args:
            texts: List of texts to embed

        Returns:
            Numpy array of embedding vectors (normalized for cosine similarity)
        """
        embeddings = self._model.encode(texts, convert_to_tensor=False, normalize_embeddings=True)
        return np.array(embeddings, dtype=np.float32)

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

        # Clear existing data
        self._ids = []
        self._documents = []
        self._metadatas = []

        for market in markets:
            embed_text = market.embedding_text

            self._ids.append(market.condition_id)
            self._documents.append(embed_text)
            self._metadatas.append({
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
        logger.info(f"Generating embeddings for {len(self._documents)} markets...")
        embeddings = self.embed_texts(self._documents)

        # Create FAISS index (Inner Product for cosine similarity on normalized vectors)
        self._index = faiss.IndexFlatIP(self._dimension)
        self._index.add(embeddings)

        # Save to disk
        self._save_index()

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
        if self._index is None or len(self._ids) == 0:
            return []

        # Generate query embedding (normalized)
        query_embedding = np.array([self.embed_text(query)], dtype=np.float32)

        # Search more results than needed if filtering
        search_k = n_results * 3 if min_liquidity else n_results

        # Search
        scores, indices = self._index.search(query_embedding, min(search_k, len(self._ids)))

        # Format results with filtering
        formatted = []
        for i, idx in enumerate(indices[0]):
            if idx < 0:  # FAISS returns -1 for empty slots
                continue

            metadata = self._metadatas[idx]

            # Apply liquidity filter
            if min_liquidity is not None and metadata["liquidity"] < min_liquidity:
                continue

            formatted.append({
                "condition_id": self._ids[idx],
                "document": self._documents[idx],
                "metadata": metadata,
                "similarity": float(scores[0][i]),  # Already cosine similarity (0-1)
            })

            if len(formatted) >= n_results:
                break

        return formatted

    def get_collection_count(self) -> int:
        """Get the number of indexed markets."""
        return len(self._ids)

    def clear(self) -> None:
        """Clear all indexed markets."""
        self._index = None
        self._ids = []
        self._documents = []
        self._metadatas = []

        # Remove persisted files
        index_path = self.persist_directory / "index.faiss"
        meta_path = self.persist_directory / "metadata.pkl"
        if index_path.exists():
            index_path.unlink()
        if meta_path.exists():
            meta_path.unlink()

        logger.info("Market embeddings cleared")
