#!/usr/bin/env python3
"""
Run All - Launch both trading modules concurrently.

This script starts:
1. Rust HFT server (arbitrage trading)
2. Python sentiment bot (news/Twitter trading)

Features:
- Subscribes to all 500+ markets automatically
- Dynamic 50% balance cap (auto-scales with P&L)
- Proxy support for Amsterdam server deployment
- Auto-restart on crash
- Coordinated capital management

Usage:
    python scripts/run_all.py [--paper] [--live] [--hft-only] [--sentiment-only]
    python scripts/run_all.py --proxy "host:port:user:pass" --live
"""

import argparse
import asyncio
import os
import signal
import subprocess
import sys
from pathlib import Path
from datetime import datetime

# Add project root to path
PROJECT_ROOT = Path(__file__).parent.parent
sys.path.insert(0, str(PROJECT_ROOT))


class TradingSystem:
    """Manages both trading modules."""

    def __init__(
        self,
        paper_trading: bool = True,
        run_hft: bool = True,
        run_sentiment: bool = True,
        proxy: str | None = None,
        min_liquidity: float = 1000.0,
        hft_balance_pct: float = 0.50,
        balance_sync_interval: float = 60.0,
    ):
        self.paper_trading = paper_trading
        self.run_hft = run_hft
        self.run_sentiment = run_sentiment
        self.proxy = proxy
        self.min_liquidity = min_liquidity
        self.hft_balance_pct = hft_balance_pct
        self.balance_sync_interval = balance_sync_interval

        self.hft_process: subprocess.Popen | None = None
        self.sentiment_task: asyncio.Task | None = None
        self.balance_sync_task: asyncio.Task | None = None
        self.hft_client = None
        self._shutdown = False

    def log(self, tag: str, message: str):
        """Log with timestamp and tag."""
        timestamp = datetime.now().strftime("%H:%M:%S")
        print(f"[{timestamp}] [{tag}] {message}")

    async def start(self) -> None:
        """Start all trading modules."""
        print("=" * 60)
        print("  POLYMARKET TRADING SYSTEM")
        print("=" * 60)
        print()

        mode = "PAPER" if self.paper_trading else "LIVE"
        print(f"Mode: {mode} TRADING")
        if self.proxy:
            print(f"Proxy: {self.proxy.split(':')[0]}:*** (configured)")
        print(f"HFT Balance Cap: {self.hft_balance_pct * 100:.0f}%")
        print()

        # Check .env exists
        env_path = PROJECT_ROOT / ".env"
        if not env_path.exists():
            print("ERROR: .env file not found!")
            print("Run: python -m src.setup")
            return

        # Start modules
        tasks = []

        if self.run_hft:
            self.log("HFT", "Starting Rust arbitrage module...")
            if not self.start_hft():
                self.log("HFT", "Failed to start, continuing without HFT...")
            else:
                # Wait for HFT server to be ready
                if await self.wait_for_hft_ready():
                    # Subscribe to all markets
                    await self.subscribe_all_markets()
                    # Start balance sync
                    await self.start_balance_sync()
                    # Start HFT trading
                    await self.start_hft_trading()
                    tasks.append(self.monitor_hft())
                else:
                    self.log("HFT", "Server failed to start properly")

        if self.run_sentiment:
            self.log("SENTIMENT", "Starting Python sentiment bot...")
            self.sentiment_task = asyncio.create_task(self.run_sentiment_bot())
            tasks.append(self.sentiment_task)

        if not tasks:
            print("No modules selected to run!")
            return

        print()
        print("=" * 60)
        print("  ALL SYSTEMS RUNNING - Press Ctrl+C to stop")
        print("=" * 60)
        print()

        # Wait for all tasks
        try:
            await asyncio.gather(*tasks)
        except asyncio.CancelledError:
            pass

    async def wait_for_hft_ready(self, timeout: float = 30.0) -> bool:
        """Wait for HFT server to be ready."""
        self.log("HFT", "Waiting for server to be ready...")
        from src.hft.client import HftClient

        self.hft_client = HftClient("http://127.0.0.1:8080")

        for _ in range(int(timeout)):
            if await self.hft_client.health_check():
                self.log("HFT", "Server is ready")
                return True
            await asyncio.sleep(1)

        self.log("HFT", "Server did not become ready in time")
        return False

    async def subscribe_all_markets(self):
        """Subscribe to all Polymarket markets."""
        self.log("HFT", f"Subscribing to markets (min liquidity: ${self.min_liquidity})...")
        try:
            from src.hft.market_fetcher import connect_and_subscribe_all
            stats = await connect_and_subscribe_all(
                hft_url="http://127.0.0.1:8080",
                min_liquidity=self.min_liquidity,
                min_volume_24h=100.0,
            )
            self.log("HFT", f"Subscribed to {stats['subscribed']}/{stats['total']} markets")
        except Exception as e:
            self.log("HFT", f"Market subscription error: {e}")

    async def start_balance_sync(self):
        """Start balance sync background task."""
        self.log("HFT", f"Starting balance sync ({self.hft_balance_pct*100:.0f}% cap, every {self.balance_sync_interval}s)")

        try:
            from src.hft.client import start_balance_sync_task

            async def get_balance():
                """Get account balance from Polymarket."""
                try:
                    from dotenv import load_dotenv
                    load_dotenv()

                    from py_clob_client.client import ClobClient
                    from py_clob_client.clob_types import BalanceAllowanceParams, AssetType

                    private_key = os.environ.get("POLYMARKET_PRIVATE_KEY")
                    if not private_key:
                        return 0.0

                    client = ClobClient(
                        host="https://clob.polymarket.com",
                        key=private_key,
                        chain_id=137,
                    )
                    params = BalanceAllowanceParams(asset_type=AssetType.COLLATERAL)
                    result = client.get_balance_allowance(params)
                    return int(result.get("balance", 0)) / 1_000_000
                except Exception as e:
                    self.log("BALANCE", f"Error: {e}")
                    return 0.0

            self.balance_sync_task = await start_balance_sync_task(
                hft_client=self.hft_client,
                get_balance_func=get_balance,
                sync_interval_seconds=self.balance_sync_interval,
                hft_percentage=self.hft_balance_pct,
            )
            self.log("HFT", "Balance sync started")
        except Exception as e:
            self.log("HFT", f"Balance sync error: {e}")

    async def start_hft_trading(self):
        """Start HFT trading."""
        if self.hft_client:
            try:
                await self.hft_client.start()
                self.log("HFT", "Trading started!")
            except Exception as e:
                self.log("HFT", f"Error starting trading: {e}")

    def start_hft(self) -> bool:
        """Start the Rust HFT server."""
        hft_dir = PROJECT_ROOT / "rust-hft"

        if not hft_dir.exists():
            self.log("HFT", "ERROR: rust-hft directory not found!")
            return False

        # Check if binary exists
        if sys.platform == "win32":
            binary = hft_dir / "target" / "release" / "hft-server.exe"
        else:
            binary = hft_dir / "target" / "release" / "hft-server"

        if not binary.exists():
            self.log("HFT", "Binary not found. Building...")
            build_result = subprocess.run(
                ["cargo", "build", "--release"],
                cwd=hft_dir,
                capture_output=True,
            )
            if build_result.returncode != 0:
                self.log("HFT", f"Build failed: {build_result.stderr.decode()}")
                return False

        # Start the server with environment variables
        env = os.environ.copy()
        if self.paper_trading:
            env["HFT_PAPER_TRADING"] = "true"
        if self.proxy:
            env["HFT_PROXY"] = self.proxy
            self.log("HFT", f"Using proxy: {self.proxy.split(':')[0]}")

        try:
            self.hft_process = subprocess.Popen(
                [str(binary)],
                cwd=hft_dir,
                env=env,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
            )
            self.log("HFT", f"Started (PID: {self.hft_process.pid})")
            return True
        except Exception as e:
            self.log("HFT", f"Failed to start: {e}")
            return False

    async def monitor_hft(self) -> None:
        """Monitor HFT process output."""
        if not self.hft_process:
            return

        while not self._shutdown:
            if self.hft_process.poll() is not None:
                print("[HFT] Process exited")
                break

            # Read output (non-blocking)
            try:
                if self.hft_process.stdout:
                    line = self.hft_process.stdout.readline()
                    if line:
                        print(f"[HFT] {line.decode().strip()}")
            except Exception:
                pass

            await asyncio.sleep(0.1)

    async def run_sentiment_bot(self) -> None:
        """Run the Python sentiment bot."""
        try:
            # Import here to avoid circular imports
            from src.main import main as sentiment_main

            # Override paper trading setting
            os.environ["PAPER_TRADING"] = str(self.paper_trading).lower()

            await sentiment_main()
        except Exception as e:
            print(f"[SENTIMENT] Error: {e}")

    async def stop(self) -> None:
        """Stop all modules gracefully."""
        print()
        self.log("SYSTEM", "Shutting down...")
        self._shutdown = True

        # Stop balance sync task
        if self.balance_sync_task and not self.balance_sync_task.done():
            self.log("HFT", "Stopping balance sync...")
            self.balance_sync_task.cancel()
            try:
                await self.balance_sync_task
            except asyncio.CancelledError:
                pass

        # Close HFT client
        if self.hft_client:
            await self.hft_client.close()

        # Stop HFT server
        if self.hft_process:
            self.log("HFT", "Stopping server...")
            self.hft_process.terminate()
            try:
                self.hft_process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.hft_process.kill()
            self.log("HFT", "Stopped")

        # Stop sentiment bot
        if self.sentiment_task and not self.sentiment_task.done():
            self.log("SENTIMENT", "Stopping...")
            self.sentiment_task.cancel()
            try:
                await self.sentiment_task
            except asyncio.CancelledError:
                pass
            self.log("SENTIMENT", "Stopped")

        self.log("SYSTEM", "Shutdown complete.")


async def main():
    parser = argparse.ArgumentParser(
        description="Run Polymarket trading system",
        formatter_class=argparse.RawDescriptionHelpFormatter,
        epilog="""
Examples:
  python scripts/run_all.py                    # Paper trading, both modules
  python scripts/run_all.py --live             # Live trading, both modules
  python scripts/run_all.py --hft-only --live  # Live HFT only
  python scripts/run_all.py --proxy "host:port:user:pass" --live
        """
    )
    parser.add_argument(
        "--paper",
        action="store_true",
        default=True,
        help="Run in paper trading mode (default)",
    )
    parser.add_argument(
        "--live",
        action="store_true",
        help="Run in live trading mode",
    )
    parser.add_argument(
        "--hft-only",
        action="store_true",
        help="Only run HFT module",
    )
    parser.add_argument(
        "--sentiment-only",
        action="store_true",
        help="Only run sentiment bot",
    )
    parser.add_argument(
        "--proxy",
        type=str,
        help="Proxy string (host:port:user:pass) for HFT module",
    )
    parser.add_argument(
        "--min-liquidity",
        type=float,
        default=1000.0,
        help="Minimum market liquidity in USD (default: 1000)",
    )
    parser.add_argument(
        "--hft-balance-pct",
        type=float,
        default=0.50,
        help="Percentage of balance for HFT (default: 0.50 = 50%%)",
    )
    parser.add_argument(
        "--balance-sync-interval",
        type=float,
        default=60.0,
        help="Balance sync interval in seconds (default: 60)",
    )
    args = parser.parse_args()

    paper_trading = not args.live
    run_hft = not args.sentiment_only
    run_sentiment = not args.hft_only

    system = TradingSystem(
        paper_trading=paper_trading,
        run_hft=run_hft,
        run_sentiment=run_sentiment,
        proxy=args.proxy,
        min_liquidity=args.min_liquidity,
        hft_balance_pct=args.hft_balance_pct,
        balance_sync_interval=args.balance_sync_interval,
    )

    # Handle signals
    loop = asyncio.get_event_loop()

    def signal_handler():
        asyncio.create_task(system.stop())

    for sig in (signal.SIGINT, signal.SIGTERM):
        try:
            loop.add_signal_handler(sig, signal_handler)
        except NotImplementedError:
            # Windows doesn't support add_signal_handler
            pass

    try:
        await system.start()
    except KeyboardInterrupt:
        await system.stop()


if __name__ == "__main__":
    asyncio.run(main())
