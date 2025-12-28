"""
Position Tracker - Track open positions and manage exits.

Handles:
1. Position tracking (paper and live)
2. P&L calculation
3. Exit signals (profit-taking, stop-loss, time-based)
4. Portfolio exposure monitoring
"""

import json
from dataclasses import dataclass, field
from datetime import datetime, timezone, timedelta
from pathlib import Path
from typing import Optional
from enum import Enum

from src.utils.logging import get_logger

logger = get_logger(__name__)


class PositionSide(str, Enum):
    """Position side."""
    YES = "YES"
    NO = "NO"


class ExitReason(str, Enum):
    """Why a position was closed."""
    PROFIT_TARGET = "PROFIT_TARGET"
    STOP_LOSS = "STOP_LOSS"
    TIME_DECAY = "TIME_DECAY"  # Near expiration
    SIGNAL_REVERSAL = "SIGNAL_REVERSAL"  # New signal contradicts position
    MANUAL = "MANUAL"
    MARKET_CLOSED = "MARKET_CLOSED"


@dataclass
class Position:
    """An open position in a market."""

    # Identification
    position_id: str
    market_id: str  # condition_id
    market_question: str

    # Position details
    side: PositionSide
    entry_price: float  # Price we bought at (0-1)
    size_usd: float  # Amount invested in USD
    shares: float = 0.0  # Number of shares (size_usd / entry_price)

    # Timing
    opened_at: datetime = field(default_factory=lambda: datetime.now(timezone.utc))
    market_end_date: Optional[datetime] = None

    # Current state
    current_price: float = 0.0
    last_updated: datetime = field(default_factory=lambda: datetime.now(timezone.utc))

    # Paper/Live
    is_paper: bool = True

    # Entry context
    entry_signal_id: str = ""
    entry_reasoning: str = ""
    entry_confidence: float = 0.0

    # Exit parameters - AGGRESSIVE MODE
    # No profit target - let winners run until market closes
    # No stop loss - we're betting on information edge, not price action
    # No max hold - hold until resolution for short-term markets

    @property
    def unrealized_pnl_usd(self) -> float:
        """Calculate unrealized P&L in USD."""
        if self.entry_price == 0:
            return 0.0
        # P&L = shares * (current_price - entry_price)
        return self.shares * (self.current_price - self.entry_price)

    @property
    def unrealized_pnl_pct(self) -> float:
        """Calculate unrealized P&L as percentage."""
        if self.size_usd == 0:
            return 0.0
        return self.unrealized_pnl_usd / self.size_usd

    @property
    def current_value_usd(self) -> float:
        """Current value of position."""
        return self.shares * self.current_price

    @property
    def hold_time_hours(self) -> float:
        """How long we've held this position."""
        delta = datetime.now(timezone.utc) - self.opened_at
        return delta.total_seconds() / 3600

    @property
    def hours_to_expiry(self) -> Optional[float]:
        """Hours until market closes."""
        if self.market_end_date is None:
            return None
        delta = self.market_end_date - datetime.now(timezone.utc)
        return max(0, delta.total_seconds() / 3600)

    def check_exit_conditions(self) -> Optional[ExitReason]:
        """
        Check if any exit condition is met.

        AGGRESSIVE MODE: Only exit when market is about to close.
        We're betting on information edge - hold until resolution.
        """
        # Only exit when market is about to close (< 2 hours)
        # This gives us time to exit before final resolution
        if self.hours_to_expiry is not None and self.hours_to_expiry < 2:
            return ExitReason.TIME_DECAY

        # No profit target - let winners run
        # No stop loss - we trust our edge
        # No max hold - hold until near resolution

        return None


@dataclass
class ClosedPosition:
    """A closed position with final P&L."""

    position: Position
    closed_at: datetime
    exit_price: float
    exit_reason: ExitReason
    realized_pnl_usd: float
    realized_pnl_pct: float


class PositionTracker:
    """
    Tracks all positions and manages exits.

    Features:
    - Real-time P&L tracking
    - Exit signal generation
    - Portfolio exposure monitoring
    - Position persistence
    """

    def __init__(
        self,
        persist_path: str = "data/positions.json",
        max_positions: int = 50,
        max_exposure_usd: float = 500.0,
    ) -> None:
        self.persist_path = Path(persist_path)
        self.persist_path.parent.mkdir(parents=True, exist_ok=True)

        self.max_positions = max_positions
        self.max_exposure_usd = max_exposure_usd

        # Active positions
        self.positions: dict[str, Position] = {}

        # Closed positions (for P&L tracking)
        self.closed_positions: list[ClosedPosition] = []

        # Load existing state
        self._load()

        logger.info(f"Position tracker initialized: {len(self.positions)} open positions")

    @property
    def total_exposure_usd(self) -> float:
        """Total USD exposed across all positions (paper + live)."""
        return sum(p.current_value_usd for p in self.positions.values())

    @property
    def live_exposure_usd(self) -> float:
        """Total USD exposed in LIVE positions only."""
        return sum(p.current_value_usd for p in self.positions.values() if not p.is_paper)

    @property
    def paper_exposure_usd(self) -> float:
        """Total USD exposed in PAPER positions only."""
        return sum(p.current_value_usd for p in self.positions.values() if p.is_paper)

    @property
    def total_unrealized_pnl_usd(self) -> float:
        """Total unrealized P&L across all positions."""
        return sum(p.unrealized_pnl_usd for p in self.positions.values())

    @property
    def available_capital_usd(self) -> float:
        """Available capital for new LIVE positions.

        Only counts live positions against capital limit.
        Paper positions have their own simulated capital and don't
        affect the real available capital for live trading.
        """
        return max(0, self.max_exposure_usd - self.live_exposure_usd)

    def can_open_position(self, size_usd: float, market_id: str) -> tuple[bool, str]:
        """Check if we can open a new position."""
        # Check position limit
        if len(self.positions) >= self.max_positions:
            return False, f"Max positions reached ({self.max_positions})"

        # Check exposure limit
        if size_usd > self.available_capital_usd:
            return False, f"Insufficient capital (need ${size_usd:.2f}, have ${self.available_capital_usd:.2f})"

        # Check if we already have a position in this market
        for pos in self.positions.values():
            if pos.market_id == market_id:
                return False, f"Already have position in this market"

        return True, "OK"

    def open_position(
        self,
        position_id: str,
        market_id: str,
        market_question: str,
        side: PositionSide,
        entry_price: float,
        size_usd: float,
        market_end_date: Optional[datetime] = None,
        is_paper: bool = True,
        signal_id: str = "",
        reasoning: str = "",
        confidence: float = 0.0,
    ) -> Optional[Position]:
        """Open a new position."""
        can_open, reason = self.can_open_position(size_usd, market_id)
        if not can_open:
            logger.warning(f"Cannot open position: {reason}")
            return None

        # Calculate shares
        shares = size_usd / entry_price if entry_price > 0 else 0

        position = Position(
            position_id=position_id,
            market_id=market_id,
            market_question=market_question,
            side=side,
            entry_price=entry_price,
            size_usd=size_usd,
            shares=shares,
            market_end_date=market_end_date,
            is_paper=is_paper,
            current_price=entry_price,
            entry_signal_id=signal_id,
            entry_reasoning=reasoning,
            entry_confidence=confidence,
        )

        self.positions[position_id] = position
        self._save()

        logger.info(
            f"Opened {'paper' if is_paper else 'live'} position: "
            f"{side.value} {market_question[:40]}... @ {entry_price:.1%} for ${size_usd:.2f}"
        )

        return position

    def update_prices(self, market_prices: dict[str, tuple[float, float]]) -> None:
        """
        Update current prices for all positions.

        Args:
            market_prices: Dict of market_id -> (yes_price, no_price)
        """
        for position in self.positions.values():
            if position.market_id in market_prices:
                yes_price, no_price = market_prices[position.market_id]
                if position.side == PositionSide.YES:
                    position.current_price = yes_price
                else:
                    position.current_price = no_price
                position.last_updated = datetime.now(timezone.utc)

        self._save()

    def check_all_exits(self) -> list[tuple[Position, ExitReason]]:
        """Check exit conditions for all positions."""
        exits = []

        for position in self.positions.values():
            exit_reason = position.check_exit_conditions()
            if exit_reason:
                exits.append((position, exit_reason))
                logger.info(
                    f"Exit signal for {position.market_question[:40]}...: "
                    f"{exit_reason.value} (P&L: {position.unrealized_pnl_pct:+.1%})"
                )

        return exits

    def close_position(
        self,
        position_id: str,
        exit_price: float,
        exit_reason: ExitReason,
    ) -> Optional[ClosedPosition]:
        """Close a position."""
        if position_id not in self.positions:
            logger.warning(f"Position {position_id} not found")
            return None

        position = self.positions[position_id]

        # Calculate final P&L
        realized_pnl_usd = position.shares * (exit_price - position.entry_price)
        realized_pnl_pct = realized_pnl_usd / position.size_usd if position.size_usd > 0 else 0

        closed = ClosedPosition(
            position=position,
            closed_at=datetime.now(timezone.utc),
            exit_price=exit_price,
            exit_reason=exit_reason,
            realized_pnl_usd=realized_pnl_usd,
            realized_pnl_pct=realized_pnl_pct,
        )

        self.closed_positions.append(closed)
        del self.positions[position_id]
        self._save()

        logger.info(
            f"Closed position: {position.market_question[:40]}... "
            f"P&L: ${realized_pnl_usd:+.2f} ({realized_pnl_pct:+.1%}) - {exit_reason.value}"
        )

        return closed

    def get_positions_by_market(self, market_id: str) -> list[Position]:
        """Get all positions for a market."""
        return [p for p in self.positions.values() if p.market_id == market_id]

    def get_expiring_positions(self, hours: float = 48) -> list[Position]:
        """Get positions expiring within N hours."""
        expiring = []
        for position in self.positions.values():
            if position.hours_to_expiry is not None and position.hours_to_expiry < hours:
                expiring.append(position)
        return sorted(expiring, key=lambda p: p.hours_to_expiry or float('inf'))

    def get_summary(self) -> dict:
        """Get portfolio summary."""
        total_invested = sum(p.size_usd for p in self.positions.values())
        total_value = sum(p.current_value_usd for p in self.positions.values())
        total_pnl = self.total_unrealized_pnl_usd

        # Separate paper vs live
        paper_positions = [p for p in self.positions.values() if p.is_paper]
        live_positions = [p for p in self.positions.values() if not p.is_paper]

        # Closed position stats
        total_realized = sum(c.realized_pnl_usd for c in self.closed_positions)
        win_count = sum(1 for c in self.closed_positions if c.realized_pnl_usd > 0)
        loss_count = sum(1 for c in self.closed_positions if c.realized_pnl_usd <= 0)

        return {
            "open_positions": len(self.positions),
            "paper_positions": len(paper_positions),
            "live_positions": len(live_positions),
            "total_invested_usd": total_invested,
            "total_value_usd": total_value,
            "paper_exposure_usd": self.paper_exposure_usd,
            "live_exposure_usd": self.live_exposure_usd,
            "unrealized_pnl_usd": total_pnl,
            "unrealized_pnl_pct": (total_pnl / total_invested * 100) if total_invested > 0 else 0,
            "available_capital_usd": self.available_capital_usd,
            "closed_trades": len(self.closed_positions),
            "realized_pnl_usd": total_realized,
            "win_rate": (win_count / (win_count + loss_count) * 100) if (win_count + loss_count) > 0 else 0,
        }

    def _save(self) -> None:
        """Persist state to disk."""
        data = {
            "positions": {
                pid: {
                    "position_id": p.position_id,
                    "market_id": p.market_id,
                    "market_question": p.market_question,
                    "side": p.side.value,
                    "entry_price": p.entry_price,
                    "size_usd": p.size_usd,
                    "shares": p.shares,
                    "opened_at": p.opened_at.isoformat(),
                    "market_end_date": p.market_end_date.isoformat() if p.market_end_date else None,
                    "current_price": p.current_price,
                    "last_updated": p.last_updated.isoformat(),
                    "is_paper": p.is_paper,
                    "entry_signal_id": p.entry_signal_id,
                    "entry_reasoning": p.entry_reasoning,
                    "entry_confidence": p.entry_confidence,
                }
                for pid, p in self.positions.items()
            },
            "closed_count": len(self.closed_positions),
            "total_realized_pnl": sum(c.realized_pnl_usd for c in self.closed_positions),
            "updated_at": datetime.now(timezone.utc).isoformat(),
        }

        with open(self.persist_path, "w") as f:
            json.dump(data, f, indent=2)

    def _load(self) -> None:
        """Load state from disk."""
        if not self.persist_path.exists():
            return

        try:
            with open(self.persist_path) as f:
                data = json.load(f)

            for pid, pdata in data.get("positions", {}).items():
                self.positions[pid] = Position(
                    position_id=pdata["position_id"],
                    market_id=pdata["market_id"],
                    market_question=pdata["market_question"],
                    side=PositionSide(pdata["side"]),
                    entry_price=pdata["entry_price"],
                    size_usd=pdata["size_usd"],
                    shares=pdata.get("shares", 0),
                    opened_at=datetime.fromisoformat(pdata["opened_at"]),
                    market_end_date=datetime.fromisoformat(pdata["market_end_date"]) if pdata.get("market_end_date") else None,
                    current_price=pdata.get("current_price", pdata["entry_price"]),
                    last_updated=datetime.fromisoformat(pdata["last_updated"]) if "last_updated" in pdata else datetime.now(timezone.utc),
                    is_paper=pdata.get("is_paper", True),
                    entry_signal_id=pdata.get("entry_signal_id", ""),
                    entry_reasoning=pdata.get("entry_reasoning", ""),
                    entry_confidence=pdata.get("entry_confidence", 0),
                )

        except Exception as e:
            logger.error(f"Failed to load positions: {e}")
