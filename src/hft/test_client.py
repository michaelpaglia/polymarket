"""
Test script for HFT client.

Run this after starting the Rust HFT server to verify connectivity.

Usage:
    python -m src.hft.test_client
"""

import asyncio
from .client import HftClient, coordinate_capital


async def test_health():
    """Test health check endpoint."""
    print("Testing health check...")
    async with HftClient() as client:
        healthy = await client.health_check()
        print(f"  Health check: {'OK' if healthy else 'FAILED'}")
        return healthy


async def test_status():
    """Test status endpoint."""
    print("Testing status...")
    async with HftClient() as client:
        try:
            status = await client.get_status()
            print(f"  State: {status.state}")
            print(f"  Uptime: {status.uptime_seconds}s")
            print(f"  Connected: {status.connected}")
            print(f"  Subscribed markets: {status.subscribed_markets}")
            print(f"  Opportunities: {status.opportunities_detected}")
            print(f"  Trades: {status.trades_executed}")
            print(f"  P&L: ${status.current_pnl_usd}")
            print(f"  Latency avg: {status.latency_avg_us}us")
            print(f"  Circuit breaker: {'TRIPPED' if status.circuit_breaker_tripped else 'OK'}")
            return True
        except Exception as e:
            print(f"  ERROR: {e}")
            return False


async def test_capital():
    """Test capital allocation."""
    print("Testing capital allocation...")
    async with HftClient() as client:
        try:
            result = await client.set_capital(
                allocation_usd=1000.0,
                max_position_size_usd=100.0,
                max_exposure_usd=1000.0,
            )
            print(f"  Allocation: ${result.get('allocation_usd', 0)}")
            print(f"  Available: ${result.get('available_usd', 0)}")
            return True
        except Exception as e:
            print(f"  ERROR: {e}")
            return False


async def test_start_stop():
    """Test start/stop controls."""
    print("Testing start/stop...")
    async with HftClient() as client:
        try:
            # Start trading
            started = await client.start()
            print(f"  Start: {'OK' if started else 'FAILED'}")

            # Check status
            status = await client.get_status()
            print(f"  State after start: {status.state}")

            # Pause trading
            paused = await client.pause()
            print(f"  Pause: {'OK' if paused else 'FAILED'}")

            # Stop trading
            stopped = await client.stop()
            print(f"  Stop: {'OK' if stopped else 'FAILED'}")

            return True
        except Exception as e:
            print(f"  ERROR: {e}")
            return False


async def test_stats():
    """Test statistics endpoint."""
    print("Testing statistics...")
    async with HftClient() as client:
        try:
            stats = await client.get_stats()
            print(f"  Risk exposure: ${stats.risk.total_exposure}")
            print(f"  Available capital: ${stats.risk.available_capital}")
            print(f"  Position count: {stats.risk.position_count}")
            print(f"  P&L realized: ${stats.pnl.realized_pnl}")
            print(f"  P&L unrealized: ${stats.pnl.unrealized_pnl}")
            print(f"  Win rate: {stats.pnl.win_rate:.1%}")
            print(f"  Circuit breaker: {'TRIPPED' if stats.circuit_breaker.is_tripped else 'OK'}")
            return True
        except Exception as e:
            print(f"  ERROR: {e}")
            return False


async def test_coordinate_capital():
    """Test capital coordination helper."""
    print("Testing capital coordination...")
    async with HftClient() as client:
        try:
            result = await coordinate_capital(
                hft_client=client,
                total_capital=10000.0,
                sentiment_pct=0.5,
                hft_pct=0.5,
            )
            print(f"  Total: ${result['total_capital']}")
            print(f"  Sentiment: ${result['sentiment_capital']}")
            print(f"  HFT: ${result['hft_capital']}")
            print(f"  HFT available: ${result['hft_available']}")
            return True
        except Exception as e:
            print(f"  ERROR: {e}")
            return False


async def test_paper_trade():
    """Test paper trading endpoint."""
    print("Testing paper trade...")
    async with HftClient() as client:
        try:
            # Test 1: Default arbitrage opportunity (YES=0.45, NO=0.52 = 0.97 sum, 3% spread)
            result = await client.paper_trade()
            print(f"  Default trade:")
            print(f"    Success: {result['success']}")
            print(f"    Market: {result['market_id']}")
            print(f"    YES: ${result['yes_price']} + NO: ${result['no_price']} = {result['price_sum']}")
            print(f"    Spread: {result['spread_bps']} bps")
            print(f"    Profit: ${result['expected_profit_usd']}")
            print(f"    Execution: {result['execution_time_us']}us")
            print()

            # Test 2: Custom profitable arbitrage
            result2 = await client.paper_trade(
                market_id="custom-market",
                yes_price=0.40,
                no_price=0.50,
                size_usd=200.0
            )
            print(f"  Custom trade (10% edge):")
            print(f"    Success: {result2['success']}")
            print(f"    Profit: ${result2['expected_profit_usd']}")
            print()

            # Test 3: No arbitrage (prices sum to > 1.0)
            result3 = await client.paper_trade(
                yes_price=0.55,
                no_price=0.50
            )
            print(f"  No arbitrage test:")
            print(f"    Success: {result3['success']} (expected: False)")
            print(f"    Reason: {result3['message']}")

            return result['success'] and result2['success'] and not result3['success']
        except Exception as e:
            print(f"  ERROR: {e}")
            return False


async def run_all_tests():
    """Run all tests."""
    print("=" * 50)
    print("HFT Client Test Suite")
    print("=" * 50)
    print()

    results = {}

    # Test health first
    results["health"] = await test_health()
    print()

    if not results["health"]:
        print("Health check failed - is the Rust HFT server running?")
        print()
        print("To start the server:")
        print("  cd rust-hft")
        print("  cargo run --release")
        print()
        return results

    # Run other tests
    results["status"] = await test_status()
    print()

    results["capital"] = await test_capital()
    print()

    results["start_stop"] = await test_start_stop()
    print()

    results["stats"] = await test_stats()
    print()

    results["coordinate"] = await test_coordinate_capital()
    print()

    results["paper_trade"] = await test_paper_trade()
    print()

    # Summary
    print("=" * 50)
    print("Test Summary")
    print("=" * 50)
    passed = sum(1 for v in results.values() if v)
    total = len(results)
    print(f"Passed: {passed}/{total}")
    for name, result in results.items():
        status = "PASS" if result else "FAIL"
        print(f"  {name}: {status}")

    return results


if __name__ == "__main__":
    asyncio.run(run_all_tests())
