"""
Daemon script for continuous bot operation.

Features:
1. Auto-restart on crash with exponential backoff
2. Graceful shutdown on SIGTERM/SIGINT
3. Health monitoring and logging
4. Configurable restart limits

Usage:
    python scripts/run_daemon.py

For Windows Task Scheduler:
    pythonw scripts/run_daemon.py  (runs without console window)
"""

import asyncio
import signal
import sys
import time
from datetime import datetime
from pathlib import Path

# Add project root to path
project_root = Path(__file__).parent.parent
sys.path.insert(0, str(project_root))

from src.config import get_settings
from src.main import PolymarketBot
from src.utils.logging import get_logger, setup_logging

logger = get_logger(__name__)


class BotDaemon:
    """
    Daemon wrapper for continuous bot operation.

    Handles:
    - Graceful shutdown
    - Auto-restart with backoff
    - Health monitoring
    """

    def __init__(self):
        self.settings = get_settings()
        self.bot: PolymarketBot | None = None
        self._running = False
        self._restart_count = 0
        self._max_restarts = 10  # Max restarts before giving up
        self._backoff_base = 5  # Initial backoff in seconds
        self._max_backoff = 300  # Max backoff (5 minutes)

    async def start(self) -> None:
        """Start the daemon loop."""
        self._running = True

        # Setup signal handlers
        signal.signal(signal.SIGINT, self._signal_handler)
        signal.signal(signal.SIGTERM, self._signal_handler)

        logger.info("Bot daemon starting...")

        while self._running and self._restart_count < self._max_restarts:
            try:
                # Create fresh bot instance
                self.bot = PolymarketBot(self.settings)
                self._log_status("STARTING")

                # Run the bot
                await self.bot.start()

                # If we get here cleanly, bot was stopped gracefully
                if not self._running:
                    self._log_status("STOPPED")
                    break

            except KeyboardInterrupt:
                self._log_status("INTERRUPTED")
                break

            except Exception as e:
                self._restart_count += 1
                backoff = min(
                    self._backoff_base * (2 ** (self._restart_count - 1)),
                    self._max_backoff
                )

                logger.error(
                    f"Bot crashed (attempt {self._restart_count}/{self._max_restarts})",
                    error=str(e),
                    next_restart_seconds=backoff,
                )
                self._log_status(f"CRASHED - restarting in {backoff}s")

                # Wait before restart
                await asyncio.sleep(backoff)

        if self._restart_count >= self._max_restarts:
            logger.error("Max restart attempts reached, daemon exiting")
            self._log_status("MAX_RESTARTS_EXCEEDED")

    def _signal_handler(self, signum, frame):
        """Handle shutdown signals."""
        logger.info(f"Received signal {signum}, shutting down...")
        self._running = False
        if self.bot:
            asyncio.create_task(self.bot.stop())

    def _log_status(self, status: str) -> None:
        """Log daemon status with timestamp."""
        timestamp = datetime.now().isoformat()
        logger.info(f"[DAEMON] {status} at {timestamp}")

        # Also write to status file for external monitoring
        status_file = project_root / "data" / "daemon_status.txt"
        status_file.parent.mkdir(parents=True, exist_ok=True)
        with open(status_file, "w") as f:
            f.write(f"{status}\n{timestamp}\n{self._restart_count}\n")


async def main():
    """Main entry point."""
    settings = get_settings()

    # Setup logging to file for daemon mode
    log_dir = project_root / "logs"
    log_dir.mkdir(parents=True, exist_ok=True)
    log_file = log_dir / f"daemon_{datetime.now().strftime('%Y%m%d')}.log"

    setup_logging(
        level=settings.logging.level,
        log_file=str(log_file),
    )

    daemon = BotDaemon()
    await daemon.start()


if __name__ == "__main__":
    asyncio.run(main())
