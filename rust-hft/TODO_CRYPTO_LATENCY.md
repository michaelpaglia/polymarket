# 15-Minute Crypto Latency HFT - TODO

## Last Session: 2026-01-05 (Session 6 - Orderbook Flow Detection)

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

### Next Steps
1. [ ] Enable FLOW-based paper trades (currently just logging)
2. [ ] Compare FLOW signals with resolution outcomes
3. [ ] Add rate limiting to FLOW signals (currently spammy)
4. [ ] Combine FLOW + TILT for higher conviction trades

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
