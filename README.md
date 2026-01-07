# Polymarket Trading Bot

[![Rust](https://img.shields.io/badge/Rust-1.75+-orange?logo=rust)](https://www.rust-lang.org/)
[![Python](https://img.shields.io/badge/Python-3.11+-blue?logo=python&logoColor=white)](https://python.org/)
[![License: MIT](https://img.shields.io/badge/License-MIT-green.svg)](LICENSE.md)
[![Polygon](https://img.shields.io/badge/Network-Polygon-8247E5?logo=polygon&logoColor=white)](https://polygon.technology/)

A dual-strategy automated trading system for [Polymarket](https://polymarket.com/) prediction markets, combining real-time sentiment analysis with low-latency arbitrage execution.

| Module | Language | Strategy | Edge Source |
|--------|----------|----------|-------------|
| **Sentiment Bot** | Python | News/Twitter analysis | Information advantage (5-30 min) |
| **HFT Arbitrage** | Rust | YES/NO mispricing | Speed + market inefficiency |

## Quick Start

### 1. Install Dependencies

```bash
git clone https://github.com/michaelpaglia/polymarket-bot.git
cd polymarket-bot

# Python dependencies
pip install -e .

# Rust toolchain (for HFT module)
# Windows
winget install Rustlang.Rustup

# macOS/Linux
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

### 2. Configure (Interactive Setup)

```bash
python -m src.setup
```

This prompts for all required API keys and writes `.env`:
- Polymarket credentials (private key, wallet address)
- Google Gemini API key
- News API keys (NewsAPI, GNews)
- Grok API key (for X/Twitter edge)

### 3. Run Both Modules

```bash
# Build Rust HFT module (first time only)
cd rust-hft && cargo build --release && cd ..

# Run everything (both modules concurrently)
python scripts/run_all.py
```

#### Command Line Options

```bash
# Paper trading (default)
python scripts/run_all.py

# Live trading
python scripts/run_all.py --live

# With EU proxy (for geo-restricted regions)
python scripts/run_all.py --proxy "host:port:user:pass" --live

# HFT only or sentiment only
python scripts/run_all.py --hft-only
python scripts/run_all.py --sentiment-only

# Custom settings
python scripts/run_all.py --min-liquidity 5000 --hft-balance-pct 0.60
```

Or run individually:
```bash
# Sentiment bot only
python -m src.main

# HFT module only
cd rust-hft && cargo run --release
```

## Architecture

```
┌─────────────────────────────────────────────────────────────────────────────┐
│                           POLYMARKET TRADING SYSTEM                         │
├─────────────────────────────────┬───────────────────────────────────────────┤
│      SENTIMENT BOT (Python)     │           HFT ARBITRAGE (Rust)            │
├─────────────────────────────────┼───────────────────────────────────────────┤
│                                 │                                           │
│  News Sources ──► Market Match  │  WebSocket ──► Orderbook ──► Arbitrage   │
│       │              │          │      │            │             │         │
│       ▼              ▼          │      ▼            ▼             ▼         │
│  LLM Analysis ──► Signal Gen    │  Price Feed ──► Spread Calc ──► Execute  │
│       │              │          │      │            │             │         │
│       ▼              ▼          │      ▼            ▼             ▼         │
│  Risk Check ───► Execute Trade  │  Risk Limits ─► Circuit Break ─► P&L     │
│                                 │                                           │
├─────────────────────────────────┼───────────────────────────────────────────┤
│  Edge: Information advantage    │  Edge: Speed + YES/NO mispricing          │
│  Latency: Seconds               │  Latency: Microseconds                    │
│  Holding: Hours to days         │  Holding: Seconds to minutes              │
└─────────────────────────────────┴───────────────────────────────────────────┘
                                    │
                                    ▼
                        ┌───────────────────────┐
                        │   Polymarket CLOB     │
                        │   (Polygon Network)   │
                        └───────────────────────┘
```

## Capital Management

The system automatically manages capital allocation between modules:

```
┌─────────────────────────────────────────────────────────┐
│                    TOTAL BALANCE                        │
├────────────────────────┬────────────────────────────────┤
│   50% Sentiment Bot    │        50% HFT Module          │
│   (Python)             │        (Rust)                  │
├────────────────────────┼────────────────────────────────┤
│  • Slower trades       │  • Sub-second trades           │
│  • Higher conviction   │  • Lower edge per trade        │
│  • News-driven         │  • Volume-driven               │
└────────────────────────┴────────────────────────────────┘
```

- **Dynamic rebalancing**: Balance syncs every 60 seconds
- **Auto-scaling**: Both modules scale with P&L
- **Configurable split**: `--hft-balance-pct 0.60` for 60/40

## Two Trading Strategies

### Strategy 1: Sentiment Bot (Python)

Finds edge through **information advantage**:

| Component | Purpose |
|-----------|---------|
| News Aggregation | Pull from NewsAPI, GNews, X/Twitter |
| Market Matching | Vector search + LLM to find relevant markets |
| Signal Analysis | Gemini analyzes news impact on probabilities |
| Sentiment Edge | Compare X sentiment vs market price |
| Risk Management | Position limits, exposure caps |

**Edge sources:**
- Breaking news before mainstream media (5-30 min advantage)
- X/Twitter sentiment divergence from market price
- Influencer posts that move markets

### Strategy 2: HFT Arbitrage (Rust)

Finds edge through **speed and market inefficiency**:

| Component | Purpose |
|-----------|---------|
| WebSocket Client | Real-time orderbook updates |
| Arbitrage Detection | Find YES + NO < 1.0 (guaranteed profit) |
| Order Execution | EIP-712 signed orders, sub-ms latency |
| Circuit Breaker | Daily loss limits, auto-pause |

**Edge sources:**
- YES + NO prices sum to < 1.0 (e.g., YES=0.45, NO=0.52 = 3% profit)
- Temporary mispricings from large orders
- Cross-market arbitrage opportunities

## Features

### Sentiment Bot
- **News Aggregation**: NewsAPI, GNews, X/Twitter via Grok
- **Semantic Matching**: ChromaDB + sentence-transformers
- **LLM Analysis**: Gemini Flash for signal generation
- **Knowledge Graph**: Auto-discovers entities from markets
- **Position Tracking**: Real-time P&L with exit management
- **Paper Trading**: Test without risking money

### HFT Module
- **Low Latency**: Rust with zero-copy parsing (simd-json)
- **WebSocket Streaming**: Real-time orderbook updates
- **Auto-Subscribe**: Fetches and subscribes to 500+ markets automatically
- **Risk Controls**: Circuit breaker, position limits, exposure caps
- **REST API**: Control via Python client or curl
- **Prometheus Metrics**: Full observability
- **Auto-Recovery**: Reconnection and error handling

### Infrastructure
- **EU Proxy Support**: Residential proxies for geo-restricted regions
- **Proxy Rotation**: Automatic fallback through proxy list
- **Balance Sync**: Dynamic 50/50 capital allocation
- **Position Closing**: `sell_shares()` for exact share liquidation

## Configuration

### Environment Variables (.env)

```bash
# Polymarket (Required)
POLYMARKET_PRIVATE_KEY=0x...
POLYMARKET_FUNDER_ADDRESS=0x...

# For API authentication (get from Polymarket)
POLYMARKET_API_KEY=your_api_key
POLYMARKET_API_SECRET=your_api_secret
POLYMARKET_API_PASSPHRASE=your_passphrase

# LLM (Required for sentiment bot)
GOOGLE_API_KEY=your_gemini_key

# News Sources
NEWSAPI_KEY=your_newsapi_key
GNEWS_API_KEY=your_gnews_key
GROK_API_KEY=your_xai_key  # Highly recommended

# HFT Settings (Optional)
HFT_CAPITAL_USD=5000
HFT_BALANCE_PERCENTAGE=50

# Proxy (Optional - for EU/geo-restricted regions)
PROXY_HOST=premium.residential-proxy.com
PROXY_PORT=22226
PROXY_USER=your_user
PROXY_PASS=your_pass
```

### config.yaml (Sentiment Bot)

```yaml
paper_trading: true  # Start with paper trading!

risk:
  max_position_per_market_usd: 100
  max_portfolio_exposure_usd: 500
  max_daily_trades: 20

signals:
  confidence_threshold: 0.7
```

### rust-hft/config/default.toml (HFT Module)

```toml
[arbitrage]
min_spread_bps = 30          # Minimum 0.30% edge to trade
max_position_size_usd = 500
cooldown_ms = 100            # 100ms between same-market trades

[risk]
max_total_exposure_usd = 5000
max_daily_loss_usd = 500     # Circuit breaker triggers
max_positions = 10
```

## Project Structure

```
polymarket/
├── src/                      # Python sentiment bot
│   ├── main.py               # Entry point
│   ├── setup.py              # Interactive CLI setup
│   ├── config.py             # Configuration
│   ├── markets/              # Market indexing & matching
│   ├── news/                 # News aggregation
│   ├── signals/              # Signal generation
│   │   ├── analyzer.py       # LLM analysis
│   │   ├── sentiment.py      # X sentiment edge
│   │   └── signal_model.py   # Quantitative scoring
│   ├── trading/              # Trade execution
│   │   ├── client.py         # Polymarket client
│   │   ├── positions.py      # Position tracking
│   │   └── websocket.py      # Price streaming
│   ├── intelligence/         # Knowledge graph
│   ├── risk/                 # Risk management
│   └── hft/                  # HFT Python client
│       ├── client.py         # REST client for Rust server
│       ├── market_fetcher.py # Auto-subscribe 500+ markets
│       └── test_client.py    # Integration tests
│
├── rust-hft/                 # Rust HFT module
│   ├── Cargo.toml            # Workspace config
│   ├── src/main.rs           # Entry point
│   ├── config/               # TOML configuration
│   └── crates/
│       ├── hft-core/         # Types, orderbook, arbitrage
│       ├── hft-websocket/    # Polymarket WebSocket
│       ├── hft-executor/     # Order execution, signing
│       ├── hft-risk/         # Circuit breaker, limits
│       ├── hft-api/          # REST API (Axum)
│       └── hft-metrics/      # Prometheus, logging
│
├── scripts/
│   ├── run_all.py            # Launch both modules
│   └── run_daemon.py         # 24/7 operation
│
├── config.yaml               # Sentiment bot config
└── .env                      # API keys (gitignored)
```

## HFT Module API

The Rust HFT server exposes a REST API on `http://127.0.0.1:8080`:

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check |
| `/api/v1/status` | GET | Current state, P&L, latency |
| `/api/v1/start` | POST | Start trading |
| `/api/v1/stop` | POST | Stop trading |
| `/api/v1/pause` | POST | Pause trading |
| `/api/v1/capital` | POST | Set capital allocation |
| `/api/v1/markets/subscribe` | POST | Subscribe to a market |
| `/api/v1/connect` | POST | Connect WebSocket |
| `/api/v1/stats` | GET | Full statistics |
| `/api/v1/stats/pnl` | GET | P&L summary |
| `/api/v1/circuit-breaker/reset` | POST | Reset circuit breaker |

### Python Client Example

```python
from src.hft import HftClient

async with HftClient() as client:
    # Check status
    status = await client.get_status()
    print(f"State: {status.state}, P&L: ${status.current_pnl_usd}")

    # Allocate capital
    await client.set_capital(5000.0, max_position=500.0)

    # Start trading
    await client.start()

    # Monitor stats
    stats = await client.get_stats()
    print(f"Win rate: {stats.pnl.win_rate:.1%}")
```

## API Keys

| Service | Purpose | Get it from |
|---------|---------|-------------|
| Polymarket | Trading | Export from MetaMask |
| Google Gemini | LLM analysis | [aistudio.google.com](https://aistudio.google.com/) |
| Grok (xAI) | X/Twitter search | [x.ai](https://x.ai/) |
| NewsAPI | News headlines | [newsapi.org](https://newsapi.org/) |
| GNews | News headlines | [gnews.io](https://gnews.io/) |

## Safety Notes

1. **Start with paper trading** - Set `paper_trading: true` in config.yaml
2. **Use small sizes** - Start with $10-50 per position
3. **Monitor actively** - Don't run unattended initially
4. **Understand HFT risks** - Circuit breaker exists for a reason
5. **Never share private keys** - Keep `.env` out of version control
6. **Test on testnet first** - If available

## Development

```bash
# Python
pip install -e ".[dev]"
pytest
pyright src/
ruff check src/

# Rust
cd rust-hft
cargo test
cargo clippy
cargo build --release
```

## Disclaimer

This software is for educational and research purposes. Trading prediction markets involves substantial risk of loss. Past performance does not guarantee future results. Always start with paper trading and use only funds you can afford to lose.

## Contributing

Contributions welcome. See [CONTRIBUTING.md](CONTRIBUTING.md) for guidelines.

## License

MIT - see [LICENSE.md](LICENSE.md)
