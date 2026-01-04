# 15-Minute Crypto Latency HFT - TODO

## Last Session: 2026-01-04

### Completed
- [x] Kraken WebSocket client (`hft-binance/src/kraken.rs`) - works globally, no geo-restrictions
- [x] `USE_KRAKEN=1` env var to switch between Binance/Coinbase/Kraken
- [x] `AGGRESSIVE_MODE=1` for lower signal thresholds (10bps vs 15bps)
- [x] Debug logging - periodic momentum and orderbook status every 10s
- [x] Parser fixes for Polymarket's new message formats (no event_type field)

### What's Working
- Price feeds connect and receive trades:
  - Kraken: `Kraken tick parsed asset=ETH price=3146.52`
  - Coinbase: `Momentum asset=ETH price=3146 change_1s_bps=-1`
- Market discovery finds 15M BTC/ETH up/down markets
- Paper trading loop runs and scans for signals

### Known Issues to Debug

1. **Orderbooks show "no data yet" for our 15M markets**
   - We subscribe to asset IDs for BTC/ETH up/down tokens
   - Book updates ARE coming from Polymarket, but for OTHER markets
   - Need to verify asset_ids we subscribe to match discovered markets
   - Check: `hft-websocket/src/client.rs` subscription logic

2. **Trade notification parsing still failing**
   - Some messages still log as "Unknown message type"
   - Format: `{"market":"0x...","asset_id":"...","price":"0.5","size":"5","fee_...`
   - Check: `hft-websocket/src/parser.rs` detect_event_type()

3. **No signals generated because:**
   - No orderbook data = can't compare PM price vs exchange price
   - Momentum is low (±1bps) - need volatile periods (>10-15bps in 1-5s)

### Next Steps

1. [ ] Debug orderbook subscription mismatch
   - Log the asset_ids we subscribe to
   - Compare with asset_ids in incoming book messages
   - May need to fetch orderbook snapshot via REST API first

2. [ ] Test on NL server with Binance (not blocked from EU)

3. [ ] Consider alternative strategy:
   - Use pure momentum/order flow from exchange as directional indicator
   - Don't need exact price comparison, just momentum direction
   - Could work even without orderbook data

### Commands to Test

```bash
# Test with Coinbase (US)
USE_COINBASE=1 cargo run --release --bin crypto-latency

# Test with Kraken (global)
USE_KRAKEN=1 cargo run --release --bin crypto-latency

# Aggressive mode (lower thresholds)
USE_COINBASE=1 AGGRESSIVE_MODE=1 cargo run --release --bin crypto-latency

# On NL server (Binance works)
cargo run --release --bin crypto-latency
```

### Key Files
- `hft-binance/src/kraken.rs` - Kraken WebSocket client
- `hft-binance/src/lib.rs` - PriceFeed enum (Binance/Coinbase/Kraken)
- `hft-server/src/crypto_latency.rs` - Main trading loop
- `hft-websocket/src/parser.rs` - Polymarket message parser
- `hft-discovery/src/discovery.rs` - 15M market discovery

### Branch
`feature/15m-crypto-hft` - commit `244110c`
