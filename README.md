# Polymarket News/Sentiment Trading Bot

A Python-based trading bot that monitors news sources and Twitter/X, analyzes sentiment using LLMs (Gemini + Grok), matches news to relevant Polymarket prediction markets, and executes trades based on information advantage.

## Features

### Core Trading
- **News Aggregation**: Pulls from NewsAPI, GNews, and X/Twitter via Grok
- **Semantic Market Matching**: Uses vector embeddings (ChromaDB + sentence-transformers) to match news to relevant markets
- **LLM-Powered Analysis**: Gemini Flash analyzes news impact and generates trading signals
- **Risk Management**: Position limits, confidence thresholds, and portfolio exposure caps
- **Paper Trading**: Test strategies without risking real money
- **Position Tracking**: Real-time P&L monitoring with automatic exit management

### Twitter/X Intelligence (Edge Detection)
- **Real-Time X Search**: Grok's live search finds breaking news before mainstream media
- **Sentiment Edge Detection**: Compares X sentiment vs market prices to find mispricings
- **Influencer Tracking**: Monitors high-influence accounts that move markets
- **Viral Content Detection**: Identifies rapidly spreading content early
- **Breaking News Alerts**: Catches "BREAKING" and "JUST IN" posts within minutes

### Knowledge Graph
- **Dynamic Entity Extraction**: Auto-discovers entities from all market questions
- **Entity-to-Market Mapping**: Links people, companies, topics to relevant markets
- **Influencer Database**: Tracks who moves which markets
- **Topic Monitoring**: Generates search queries for market-relevant topics

### Infrastructure
- **WebSocket Support**: Real-time price updates from Polymarket CLOB
- **Daemon Mode**: Auto-restart with exponential backoff for 24/7 operation
- **Position Persistence**: Saves positions to disk, survives restarts
- **Structured Logging**: Rich console output and rotating log files

## Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                      DATA SOURCES                           │
├─────────────────────────────────────────────────────────────┤
│  NewsAPI    GNews    Grok/X (Twitter)    Polymarket API     │
└──────┬────────┬────────────┬─────────────────┬──────────────┘
       │        │            │                 │
       ▼        ▼            ▼                 ▼
┌─────────────────────┐  ┌─────────────────────────────────────┐
│   News Aggregator   │  │         Market Indexer              │
│  (dedup, normalize) │  │    (fetch, filter, index)           │
└─────────┬───────────┘  └──────────────┬──────────────────────┘
          │                             │
          ▼                             ▼
┌─────────────────────┐  ┌─────────────────────────────────────┐
│   Market Matcher    │◄─│   Dynamic Knowledge Graph           │
│ (vector + LLM)      │  │   (entities, influencers, topics)   │
└─────────┬───────────┘  └──────────────┬──────────────────────┘
          │                             │
          ▼                             ▼
┌─────────────────────────────────────────────────────────────┐
│                    Signal Analyzer                          │
│         (Gemini analysis + X sentiment edge)                │
└──────────────────────────┬──────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                    Risk Manager                             │
│    (position limits, exposure caps, confidence filter)      │
└──────────────────────────┬──────────────────────────────────┘
                           │
                           ▼
┌─────────────────────────────────────────────────────────────┐
│                 Trading Executor                            │
│         (paper/live trades, position tracking)              │
└─────────────────────────────────────────────────────────────┘
```

## Quick Start

### 1. Clone and Install

```bash
git clone https://github.com/michaelpaglia/polymarket.git
cd polymarket

# Using uv (recommended)
uv sync

# Or using pip
pip install -e .
```

### 2. Configure Environment

**Option A: Interactive Setup (Recommended)**
```bash
python -m src.setup
```

**Option B: Manual Setup**
```bash
cp .env.example .env
```

Edit `.env` with your API keys:

```bash
# Required for trading
POLYMARKET_PRIVATE_KEY=your_private_key
POLYMARKET_FUNDER_ADDRESS=your_wallet_address

# Required for LLM analysis
GOOGLE_API_KEY=your_gemini_api_key

# Required for news (at least one)
NEWSAPI_KEY=your_newsapi_key
GNEWS_API_KEY=your_gnews_key

# Highly recommended for edge detection
GROK_API_KEY=your_xai_api_key
```

### 3. Configure Settings

Edit `config.yaml` to adjust:

- `paper_trading: true` - Start with paper trading!
- Risk limits (max position size, portfolio exposure)
- Confidence thresholds
- News polling intervals

### 4. Run the Bot

```bash
# Paper trading (default)
python -m src.main

# Or using the installed command
polymarket-bot

# Daemon mode (auto-restart, 24/7 operation)
python scripts/run_daemon.py
```

## Configuration

### config.yaml

```yaml
# Trading mode - START WITH PAPER TRADING
paper_trading: true

# News sources
news:
  poll_interval_seconds: 60
  sources:
    - newsapi
    - gnews
    - grok  # X/Twitter via Grok API

# Market matching
markets:
  refresh_interval_minutes: 15
  min_liquidity_usd: 500
  max_days_to_resolution: 90

# Risk management
risk:
  max_position_per_market_usd: 100
  max_portfolio_exposure_usd: 500
  max_daily_trades: 20

# Signal generation
signals:
  confidence_threshold: 0.7
  require_multi_source: true
```

## API Keys

### Polymarket
- Export your private key from MetaMask or your wallet
- Your funder address is your wallet address holding USDC on Polygon
- Need USDC on Polygon network for live trading

### Gemini (Google)
- Get an API key from [Google AI Studio](https://aistudio.google.com/)
- Used for news analysis and signal generation

### Grok (xAI) - Recommended
- Get an API key from [x.ai](https://x.ai/)
- Provides real-time X/Twitter search for edge detection
- This is the key differentiator for finding alpha

### NewsAPI
- Get a free API key from [newsapi.org](https://newsapi.org/)
- Free tier: 100 requests/day

### GNews
- Get a free API key from [gnews.io](https://gnews.io/)
- Free tier: 100 requests/day, 10 articles/request

## Project Structure

```
polymarket/
├── src/
│   ├── main.py              # Bot entry point & orchestrator
│   ├── config.py            # Configuration management
│   ├── setup.py             # Interactive setup wizard
│   │
│   ├── markets/             # Market data & matching
│   │   ├── indexer.py       # Fetches markets from Gamma API
│   │   ├── embeddings.py    # Vector DB (ChromaDB)
│   │   ├── matcher.py       # News-to-market matching
│   │   └── models.py        # Market data models
│   │
│   ├── news/                # News ingestion
│   │   ├── newsapi.py       # NewsAPI source
│   │   ├── gnews.py         # GNews source
│   │   ├── grok.py          # X/Twitter via Grok
│   │   ├── aggregator.py    # Combines & dedupes sources
│   │   └── models.py        # Article models
│   │
│   ├── signals/             # Signal generation
│   │   ├── analyzer.py      # LLM-based analysis
│   │   ├── sentiment.py     # X sentiment edge detection
│   │   └── models.py        # Signal & TradeDecision models
│   │
│   ├── trading/             # Trading execution
│   │   ├── client.py        # Polymarket CLOB client
│   │   ├── executor.py      # Trade execution
│   │   ├── positions.py     # Position tracking & P&L
│   │   └── websocket.py     # Real-time price updates
│   │
│   ├── intelligence/        # Alpha generation
│   │   ├── knowledge_graph.py   # Entity-market mapping
│   │   ├── dynamic_graph.py     # Auto-expanding graph
│   │   └── twitter_intel.py     # Twitter alpha scanning
│   │
│   ├── risk/                # Risk management
│   │   └── manager.py       # Position & exposure limits
│   │
│   └── utils/               # Shared utilities
│       └── logging.py       # Structured logging
│
├── scripts/
│   └── run_daemon.py        # 24/7 daemon with auto-restart
│
├── data/                    # Persisted data (gitignored)
│   ├── positions.json       # Open positions
│   ├── knowledge_graph.json # Entity graph
│   └── chroma/              # Vector embeddings
│
├── config.yaml              # Runtime configuration
├── .env                     # API keys (not committed)
└── pyproject.toml           # Dependencies
```

## How the Edge Works

### 1. News Speed Advantage
Traditional news sources (NewsAPI, GNews) provide headlines. The bot matches these to markets faster than manual traders.

### 2. X/Twitter Sentiment Edge
This is the real alpha:
- Grok searches X in real-time for discussions about market topics
- Compares X sentiment (what Twitter thinks the probability is) vs market price
- If X thinks 70% probability but market is at 50%, that's a 20% edge
- Trades when sentiment diverges significantly from market price

### 3. Breaking News Detection
- Scans for "BREAKING", "JUST IN", "DEVELOPING" posts
- Monitors credible news accounts (@AP, @Reuters, @WSJ, etc.)
- Catches news 5-30 minutes before mainstream media coverage

### 4. Influencer Tracking
- Monitors high-influence accounts (Elon Musk, politicians, etc.)
- These accounts can move markets within minutes
- Bot detects their posts and evaluates market impact

## Safety Notes

1. **Start with paper trading** - Always set `paper_trading: true` initially
2. **Use small position sizes** - Start with $10-50 per market max
3. **Monitor actively** - Don't leave the bot running unattended initially
4. **Validate signals manually** - Review paper trades before enabling live trading
5. **Never share your private key** - Keep `.env` out of version control
6. **Understand the risks** - Prediction markets are volatile; you can lose money

## Development

```bash
# Install dev dependencies
uv sync --dev

# Run tests
pytest

# Type checking
pyright src/

# Lint
ruff check src/

# Format
ruff format src/
```

## License

MIT
