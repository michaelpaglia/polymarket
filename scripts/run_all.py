#!/usr/bin/env python3
"""
Run All - Launch both trading modules concurrently.

This script starts:
1. Rust HFT server (arbitrage trading)
2. Python sentiment bot (news/Twitter trading)

Both modules share the same .env configuration and run independently.
Capital is automatically coordinated between them.

Usage:
    python scripts/run_all.py [--paper] [--hft-only] [--sentiment-only]
"""

import argparse
import asyncio
import os
import signal
import subprocess
import sys
from pathlib import Path

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
    ):
        self.paper_trading = paper_trading
        self.run_hft = run_hft
        self.run_sentiment = run_sentiment
        self.hft_process: subprocess.Popen | None = None
        self.sentiment_task: asyncio.Task | None = None
        self._shutdown = False

    async def start(self) -> None:
        """Start all trading modules."""
        print("=" * 60)
        print("  POLYMARKET TRADING SYSTEM")
        print("=" * 60)
        print()

        mode = "PAPER" if self.paper_trading else "LIVE"
        print(f"Mode: {mode} TRADING")
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
            print("[HFT] Starting Rust arbitrage module...")
            self.start_hft()
            tasks.append(self.monitor_hft())

        if self.run_sentiment:
            print("[SENTIMENT] Starting Python sentiment bot...")
            self.sentiment_task = asyncio.create_task(self.run_sentiment_bot())
            tasks.append(self.sentiment_task)

        if not tasks:
            print("No modules selected to run!")
            return

        print()
        print("Both modules running. Press Ctrl+C to stop.")
        print()

        # Wait for all tasks
        try:
            await asyncio.gather(*tasks)
        except asyncio.CancelledError:
            pass

    def start_hft(self) -> None:
        """Start the Rust HFT server."""
        hft_dir = PROJECT_ROOT / "rust-hft"

        if not hft_dir.exists():
            print("[HFT] ERROR: rust-hft directory not found!")
            return

        # Check if binary exists
        if sys.platform == "win32":
            binary = hft_dir / "target" / "release" / "hft-server.exe"
        else:
            binary = hft_dir / "target" / "release" / "hft-server"

        if not binary.exists():
            print("[HFT] Binary not found. Building...")
            build_result = subprocess.run(
                ["cargo", "build", "--release"],
                cwd=hft_dir,
                capture_output=True,
            )
            if build_result.returncode != 0:
                print(f"[HFT] Build failed: {build_result.stderr.decode()}")
                return

        # Start the server
        env = os.environ.copy()
        if self.paper_trading:
            env["HFT_PAPER_TRADING"] = "true"

        self.hft_process = subprocess.Popen(
            [str(binary)],
            cwd=hft_dir,
            env=env,
            stdout=subprocess.PIPE,
            stderr=subprocess.STDOUT,
        )
        print(f"[HFT] Started (PID: {self.hft_process.pid})")

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
        print("Shutting down...")
        self._shutdown = True

        # Stop HFT
        if self.hft_process:
            print("[HFT] Stopping...")
            self.hft_process.terminate()
            try:
                self.hft_process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.hft_process.kill()
            print("[HFT] Stopped")

        # Stop sentiment bot
        if self.sentiment_task and not self.sentiment_task.done():
            print("[SENTIMENT] Stopping...")
            self.sentiment_task.cancel()
            try:
                await self.sentiment_task
            except asyncio.CancelledError:
                pass
            print("[SENTIMENT] Stopped")

        print("Shutdown complete.")


async def main():
    parser = argparse.ArgumentParser(description="Run Polymarket trading system")
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
    args = parser.parse_args()

    paper_trading = not args.live
    run_hft = not args.sentiment_only
    run_sentiment = not args.hft_only

    system = TradingSystem(
        paper_trading=paper_trading,
        run_hft=run_hft,
        run_sentiment=run_sentiment,
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
