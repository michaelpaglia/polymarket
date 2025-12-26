"""Trading executor for managing order execution."""

from dataclasses import dataclass, field
from datetime import datetime
from typing import Optional

from src.config import RiskSettings
from src.signals.models import SignalDirection, TradingSignal
from src.trading.client import OrderResult, PolymarketClient
from src.utils.logging import get_logger

logger = get_logger(__name__)


@dataclass
class TradeRecord:
    """Record of an executed trade."""

    signal: TradingSignal
    order_result: Optional[OrderResult]
    executed_at: datetime = field(default_factory=datetime.now)
    is_paper: bool = True


class TradingExecutor:
    """
    Manages trade execution with paper trading support.

    Features:
    - Paper trading mode (logs trades without execution)
    - Live trading via Polymarket client
    - Trade history tracking
    - Daily trade limits
    """

    def __init__(
        self,
        client: PolymarketClient,
        risk_settings: RiskSettings,
        paper_trading: bool = True,
    ) -> None:
        """
        Initialize the trading executor.

        Args:
            client: Polymarket client for live trades
            risk_settings: Risk management settings
            paper_trading: Whether to run in paper trading mode
        """
        self.client = client
        self.risk_settings = risk_settings
        self.paper_trading = paper_trading

        # Trade tracking
        self.trades: list[TradeRecord] = []
        self._daily_trade_count = 0
        self._last_trade_date: Optional[datetime] = None

    async def execute(self, signal: TradingSignal) -> TradeRecord:
        """
        Execute a trading signal.

        Args:
            signal: Trading signal to execute

        Returns:
            Trade record with execution details
        """
        # Check daily trade limit
        if not self._can_trade():
            logger.warning("Daily trade limit reached, skipping trade")
            return TradeRecord(
                signal=signal,
                order_result=OrderResult(
                    success=False,
                    order_id=None,
                    message="Daily trade limit reached",
                ),
                is_paper=self.paper_trading,
            )

        if self.paper_trading:
            return await self._paper_trade(signal)
        else:
            return await self._live_trade(signal)

    async def _paper_trade(self, signal: TradingSignal) -> TradeRecord:
        """Execute a paper trade (simulated)."""
        logger.info(
            "Paper trade executed",
            market=signal.market_question[:50],
            direction=signal.direction.value,
            size=signal.suggested_size_usd,
            confidence=signal.confidence,
        )

        self._record_trade()

        return TradeRecord(
            signal=signal,
            order_result=OrderResult(
                success=True,
                order_id=f"PAPER-{signal.signal_id[:8]}",
                message="Paper trade recorded",
            ),
            is_paper=True,
        )

    async def _live_trade(self, signal: TradingSignal) -> TradeRecord:
        """Execute a live trade."""
        if signal.direction == SignalDirection.HOLD:
            return TradeRecord(
                signal=signal,
                order_result=OrderResult(
                    success=False,
                    order_id=None,
                    message="HOLD signal - no trade",
                ),
                is_paper=False,
            )

        if not signal.target_token_id:
            return TradeRecord(
                signal=signal,
                order_result=OrderResult(
                    success=False,
                    order_id=None,
                    message="No target token ID",
                ),
                is_paper=False,
            )

        # Execute market order
        result = self.client.place_market_order(
            token_id=signal.target_token_id,
            side="BUY",
            amount_usd=signal.suggested_size_usd,
        )

        if result.success:
            self._record_trade()

        record = TradeRecord(
            signal=signal,
            order_result=result,
            is_paper=False,
        )

        self.trades.append(record)

        return record

    def _can_trade(self) -> bool:
        """Check if we can make another trade today."""
        today = datetime.now().date()

        # Reset counter if new day
        if self._last_trade_date is None or self._last_trade_date.date() != today:
            self._daily_trade_count = 0

        return self._daily_trade_count < self.risk_settings.max_daily_trades

    def _record_trade(self) -> None:
        """Record that a trade was made."""
        self._daily_trade_count += 1
        self._last_trade_date = datetime.now()

    @property
    def daily_trades_remaining(self) -> int:
        """Get remaining trades for today."""
        return max(0, self.risk_settings.max_daily_trades - self._daily_trade_count)

    def get_paper_pnl(self) -> float:
        """
        Calculate paper trading P&L.

        Note: This is a simplified calculation that assumes
        trades would have executed at the signal price.
        Real P&L would depend on actual execution prices.
        """
        # TODO: Implement actual P&L tracking with market price updates
        return 0.0
