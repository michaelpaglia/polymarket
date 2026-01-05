# 15-Minute Crypto Latency HFT - TODO

## Last Session: 2026-01-05 (Session 9 - Live Trading Infrastructure)

### Session 9 Progress
- [x] **Live order execution added** - OrderExecutor integrated into CryptoLatencyApp
- [x] **Auto-redemption verified** - Already runs every 60s when not in paper mode
- [x] **LIVE_TRADING env var** - Set `LIVE_TRADING=1` to enable real trades
- [x] **Position tracking** - LivePosition struct tracks live positions for P&L

### Live Trading Setup
```bash
# Only need private key - API credentials are derived automatically (like Python)
export POLYMARKET_PRIVATE_KEY="your_polygon_private_key"

# Enable live trading (default is paper mode)
export LIVE_TRADING=1

# Position size (default $1 for safety, adjust as needed)
export POSITION_SIZE_USD=1

# Run with Kraken price feed (works from US)
USE_KRAKEN=1 AGGRESSIVE_MODE=1 LIVE_TRADING=1 POSITION_SIZE_USD=1 cargo run --release --bin crypto-latency
```

### Testing Checklist
Before going live:
1. [ ] Verify wallet has USDC balance on Polygon
2. [ ] Verify wallet has MATIC for gas
3. [ ] Verify API credentials are correct (from Polymarket CLOB)
4. [ ] Start with paper mode to verify signals fire
5. [ ] Enable live mode with $1 position size for one window
6. [ ] Monitor auto-redemption after window resolves

### Auto-Redemption
- Runs every 60 seconds when `LIVE_TRADING=1`
- Checks for redeemable positions via Polymarket data API
- Submits redemption transactions to Polygon mainnet
- Requires `POLYMARKET_PRIVATE_KEY` or `POLYGON_PRIVATE_KEY`

---

## Previous Session: 2026-01-05 (Session 8 - FLOW Tuning & P&L Fix)

### Session 8 Progress
- [x] **Tested 10PM window** - 36 trades, 9 wins, 27 losses (25% win rate)
- [x] **FLOW threshold increased** - 0.6 → 0.8 (reduce noisy signals)
- [x] **FLOW rate limit increased** - 60s → 120s (fewer trades)
- [x] **P&L bug fixed** - Was parsing string with decimals, now uses round().to_i64()
- [x] **Tested 10:30PM window** - 30 trades, 29 wins, 1 loss (96.7% win rate), +$17.07 P&L

### 10PM Window Results (Before Fixes)
- BTC: Strike $93,026.60 → Final $92,961.70 = DOWN won
- ETH: Strike $3,192.32 → Final $3,185.83 = DOWN won
- **Problem**: FLOW signals flip-flopped, bought too many ETH UP trades
- **Net P&L**: ~-$6.66 (P&L tracking was bugged, showing $0.00)

### 10:30PM Window Results (After Fixes)
- **29 wins, 1 loss** (96.7% win rate)
- **Net P&L: +$17.07**
- TILT strategy correctly identified cheap DOWN tokens and stacked bets
- Tuned FLOW parameters prevented noisy signals

### Signal Hierarchy (Updated)
| Signal | Trigger | Rate Limit | Conviction |
|--------|---------|------------|------------|
| COMBO | TILT + FLOW agree | 10s | Highest |
| TILT | Side < 40% | 30s | Medium |
| FLOW | flow > 0.8, velocity > 10bps/s | 120s | Lower |

### Key Files Modified This Session
- `hft-server/src/crypto_latency.rs` - FLOW tuning, P&L fix

### Next Steps
1. [x] Test with new FLOW parameters (0.8 threshold, 120s rate limit) - SUCCESS!
2. [x] Add live trading infrastructure - DONE!
3. [ ] Test live trading for one 15-minute window with small size ($1)
4. [ ] Monitor auto-redemption working correctly
5. [ ] Deploy to NL server for lower latency + Binance price feed

---

## Previous Session: 2026-01-05 (Session 7 - Mechanical Trading System)

### Session 7 Progress
- [x] **Mechanical trading system** - Three signal types with different conviction levels
- [x] **Rate limiting by signal type** - COMBO=10s, TILT=30s, FLOW=60s per market
- [x] **Session statistics tracking** - Wins/losses/P&L displayed in STATUS logs
- [x] **STATUS log rate limiting** - Every 30 seconds instead of spamming

---

## Previous Session: 2026-01-05 (Session 6 - Orderbook Flow Detection)

### Session 6 Progress
- [x] **Orderbook sorting fixed** - Bids descending, asks ascending for correct best prices
- [x] **FLOW signal detection** - Detects buying pressure on UP/DOWN tokens
- [x] **Real orderbook data flowing** - Best bid/ask/depth now accurate

### Orderbook Flow Detection
```
FLOW: Buying pressure on DOWN (flow=1.00, velocity=789.8bps/s) market=eth-updown-15m
FLOW: Buying pressure on DOWN (flow=1.00, velocity=222.5bps/s) market=btc-updown-15m

ORDERBOOK: UP bids=["0.75@180", "0.74@1532"] asks=["0.76@91", "0.77@598"] depth=75/24
```

Flow detection uses:
- **flow_imbalance()**: -1.0 (selling) to +1.0 (buying) based on price movement direction
- **velocity_bps_per_sec()**: How fast the orderbook mid-price is moving
- Signals fire when flow > 0.6 AND velocity > 10bps/s

### Key Files Modified This Session
- `hft-websocket/src/client.rs` - Fixed orderbook sorting (bids desc, asks asc)
- `hft-server/src/crypto_latency.rs` - Added FLOW signal detection and orderbook logging

---

## Previous Session: 2026-01-04 (Session 5 - TILT Strategy Profitable!)

### Session 5 Progress
- [x] **TILT signal strategy working** - Buy cheap side when market tilted (< 0.40)
- [x] **Resolution tracking verified** - Win/loss calculated correctly at window end
- [x] **Full window paper trading** - Tested 5:00-5:15pm window with resolution

### Paper Trading Results (5:00-5:15pm ET Window)
| Asset | Direction | Entry | Result | PnL/Trade |
|-------|-----------|-------|--------|-----------|
| BTC | DOWN | $0.095 | WIN | +$0.905 |
| ETH | DOWN | $0.140 | LOSS | -$0.140 |

**Net P&L: +$7.65** (10 BTC wins, 10 ETH losses)

The TILT strategy worked - BTC Down was priced at only 9.5% (10.53x payout) and BTC actually went down!

### TILT Signal Logic
```
# Buy the CHEAP side when market is heavily tilted
if up_price < 0.40:
    BUY UP (contrarian - market expects DOWN)
if down_price < 0.40:
    BUY DOWN (contrarian - market expects UP)
```

### Commands to Test
```bash
# Full paper trading with TILT signals
HFT_PROXY_FILE=/nonexistent USE_KRAKEN=1 AGGRESSIVE_MODE=1 cargo run --release --bin crypto-latency

# Watch only key signals
HFT_PROXY_FILE=/nonexistent USE_KRAKEN=1 AGGRESSIVE_MODE=1 cargo run --release --bin crypto-latency 2>&1 | grep -E "(TILT|TRADE|RESOLUTION)"
```

### Key Files Modified This Session
- `hft-server/src/crypto_latency.rs` - TILT signal, disabled MOMENTUM (was conflicting)

### Next Steps
1. [ ] Tune TILT threshold (currently < 0.40, try < 0.35 for higher conviction)
2. [ ] Add position sizing based on tilt magnitude
3. [ ] Test on NL server with Binance for lower latency
4. [ ] Track win rate statistics over multiple windows

---

## Previous Sessions

### Session 3 (Complete)
- [x] Momentum tracking via discovery prices
- [x] Tilt detection (up_price < 0.45 or down_price < 0.45)
- [x] Paper trade execution ($1 position size)
- [x] Rate limiting (once per 5 seconds per market)
- [x] FlowTracker in orderbook (not actively used)

### Session 2 (Complete)
- [x] Kraken WebSocket client - works globally
- [x] `USE_KRAKEN=1` env var for Kraken price feed
- [x] `AGGRESSIVE_MODE=1` for lower thresholds
- [x] FIX: Orderbook subscription race condition
- [x] FIX: Trade notification parsing
- [x] NEW: start_time field on CryptoMarket
- [x] NEW: Strike price capture logic

### Branch
`feature/15m-crypto-hft`
