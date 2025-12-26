# Polymarket News/Sentiment Trading Bot

A Python-based trading bot that monitors news sources, analyzes sentiment using LLMs (Gemini), matches news to relevant Polymarket prediction markets, and executes trades based on information advantage.

## Features

- **News Aggregation**: Pulls from NewsAPI and GNews for broad coverage
- **Semantic Market Matching**: Uses vector embeddings (Chroma + sentence-transformers) to match news to relevant markets
- **LLM-Powered Analysis**: Gemini Flash analyzes news impact and generates trading signals
- **Risk Management**: Position limits, confidence thresholds, and portfolio exposure caps
- **Paper Trading**: Test strategies without risking real money
- **Structured Logging**: Rich console output and log files

## Architecture

```
News Sources (NewsAPI, GNews)
         ↓
    News Aggregator (deduplication, normalization)
         ↓
    Market Matcher (vector search → LLM validation)
         ↓
    Signal Analyzer (LLM generates trading signals)
         ↓
    Risk Manager (validates signals against limits)
         ↓
    Trading Executor (paper or live trades)
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
```

### 3. Configure Settings

Edit `config.yaml` to adjust:

- `paper_trading: true` - Start with paper trading!
- Risk limits (max position size, portfolio exposure)
- Confidence thresholds
- News polling intervals

### 4. Run the Bot

```bash
# Using the installed command
polymarket-bot

# Or directly
python -m src.main
```

## Configuration

### config.yaml

```yaml
# Trading mode - START WITH PAPER TRADING
paper_trading: true

# Risk management
risk:
  max_position_per_market_usd: 100  # Max $100 per market
  max_portfolio_exposure_usd: 500   # Max $500 total
  max_daily_trades: 20

# Signal generation
signals:
  confidence_threshold: 0.7  # Only trade on high confidence
  require_multi_source: true
```

## API Keys

### Polymarket
- Export your private key from MetaMask or your wallet
- Your funder address is your wallet address holding USDC on Polygon

### Gemini (Google)
- Get an API key from [Google AI Studio](https://aistudio.google.com/)

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
│   ├── config.py          # Configuration management
│   ├── main.py             # Entry point
│   ├── markets/            # Market data & matching
│   │   ├── indexer.py      # Fetches Polymarket markets
│   │   ├── embeddings.py   # Vector DB (Chroma)
│   │   └── matcher.py      # News-to-market matching
│   ├── news/               # News ingestion
│   │   ├── newsapi.py      # NewsAPI source
│   │   ├── gnews.py        # GNews source
│   │   └── aggregator.py   # Combines sources
│   ├── signals/            # Signal generation
│   │   └── analyzer.py     # LLM-based analysis
│   ├── trading/            # Trading execution
│   │   └── client.py       # Polymarket client
│   └── utils/              # Shared utilities
├── config.yaml             # Runtime configuration
├── .env                    # API keys (not committed)
└── pyproject.toml          # Dependencies
```

## Safety Notes

1. **Start with paper trading** - Always set `paper_trading: true` initially
2. **Use small position sizes** - Start with $10-50 per market max
3. **Monitor actively** - Don't leave the bot running unattended initially
4. **Validate signals manually** - Review paper trades before enabling live trading
5. **Never share your private key** - Keep `.env` out of version control

## Development

```bash
# Install dev dependencies
uv sync --dev

# Run tests
pytest

# Lint
ruff check src/

# Format
ruff format src/
```

## License

MIT
