# Polymarket HFT Module

High-frequency trading module for YES/NO mispricing arbitrage on Polymarket.

## Prerequisites

1. **Install Rust**: https://rustup.rs/
   ```bash
   # Windows (PowerShell)
   winget install Rustlang.Rustup

   # Or download from https://rustup.rs/
   ```

2. **Set environment variables**:
   ```bash
   # Required
   export POLYMARKET_PRIVATE_KEY="your_private_key"
   export POLYMARKET_API_KEY="your_api_key"
   export POLYMARKET_API_SECRET="your_api_secret"
   export POLYMARKET_API_PASSPHRASE="your_passphrase"

   # Optional
   export HFT_CAPITAL_USD="5000"  # Initial capital allocation
   export CONFIG_PATH="config/production.toml"  # Config file path
   ```

## Build & Run

```bash
cd rust-hft

# Development build
cargo build

# Release build (optimized for production)
cargo build --release

# Run the server
cargo run --release
```

## API Endpoints

The server runs on `http://127.0.0.1:8080` by default.

| Endpoint | Method | Description |
|----------|--------|-------------|
| `/health` | GET | Health check |
| `/api/v1/status` | GET | Current status, P&L, latency |
| `/api/v1/start` | POST | Start trading |
| `/api/v1/stop` | POST | Stop trading |
| `/api/v1/pause` | POST | Pause trading |
| `/api/v1/capital` | POST | Set capital allocation |
| `/api/v1/stats` | GET | Full statistics |
| `/api/v1/stats/pnl` | GET | P&L summary |
| `/api/v1/circuit-breaker/reset` | POST | Reset circuit breaker |

## Configuration

Edit `config/default.toml` or `config/production.toml`:

```toml
[arbitrage]
min_spread_bps = 30        # Minimum 0.30% edge
max_position_size_usd = 500.0
cooldown_ms = 100          # 100ms between same-market trades

[risk]
max_total_exposure_usd = 5000.0
max_daily_loss_usd = 500.0
max_positions = 10
```

## Python Integration

```python
from src.hft import HftClient

async with HftClient() as client:
    # Check status
    status = await client.get_status()
    print(f"State: {status.state}")

    # Set capital (50% of total)
    await client.set_capital(5000.0, max_position=500.0)

    # Start trading
    await client.start()

    # Monitor
    stats = await client.get_stats()
    print(f"P&L: ${stats.pnl.total_pnl}")
```

## Architecture

```
rust-hft/
├── crates/
│   ├── hft-core/       # Types, orderbook, arbitrage detection
│   ├── hft-websocket/  # Polymarket WebSocket client
│   ├── hft-executor/   # Order execution, EIP-712 signing
│   ├── hft-risk/       # Position limits, circuit breaker
│   ├── hft-api/        # REST API (Axum)
│   └── hft-metrics/    # Prometheus metrics, logging
└── src/main.rs         # Entry point
```

## Testing

```bash
# Run Rust tests
cargo test

# Test Python client (requires running server)
python -m src.hft.test_client
```

## Amsterdam Deployment

For Amsterdam server, use production config:

```bash
export CONFIG_PATH="config/production.toml"
cargo run --release
```

Production config has:
- More aggressive spread threshold (25 bps)
- Larger position sizes ($1000)
- Faster cooldown (50ms)
- JSON logging enabled
