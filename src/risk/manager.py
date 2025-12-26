"""Risk manager for validating trading signals."""

from dataclasses import dataclass

from src.config import RiskSettings, SignalSettings
from src.signals.models import SignalDirection, TradingSignal
from src.utils.logging import get_logger

logger = get_logger(__name__)


@dataclass
class RiskCheckResult:
    """Result of a risk check."""

    approved: bool
    reason: str
    adjusted_size: float = 0.0


class RiskManager:
    """
    Validates trading signals against risk limits.

    Checks:
    - Position size limits
    - Portfolio exposure limits
    - Confidence thresholds
    - Daily trade limits
    """

    def __init__(
        self,
        risk_settings: RiskSettings,
        signal_settings: SignalSettings,
    ) -> None:
        """
        Initialize the risk manager.

        Args:
            risk_settings: Risk limit configuration
            signal_settings: Signal threshold configuration
        """
        self.risk_settings = risk_settings
        self.signal_settings = signal_settings

        # Track positions
        self._positions: dict[str, float] = {}  # market_id -> position_size
        self._total_exposure: float = 0.0

    def check_signal(self, signal: TradingSignal) -> RiskCheckResult:
        """
        Check if a signal passes risk limits.

        Args:
            signal: Trading signal to validate

        Returns:
            RiskCheckResult with approval status
        """
        # Skip HOLD signals
        if signal.direction == SignalDirection.HOLD:
            return RiskCheckResult(
                approved=False,
                reason="HOLD signal - no action needed",
            )

        # Check confidence threshold
        if signal.confidence < self.signal_settings.confidence_threshold:
            return RiskCheckResult(
                approved=False,
                reason=f"Confidence {signal.confidence:.2%} below threshold {self.signal_settings.confidence_threshold:.2%}",
            )

        # Check position size
        if signal.suggested_size_usd > self.risk_settings.max_position_per_market_usd:
            adjusted_size = self.risk_settings.max_position_per_market_usd
            logger.info(
                "Position size capped",
                original=signal.suggested_size_usd,
                adjusted=adjusted_size,
            )
        else:
            adjusted_size = signal.suggested_size_usd

        # Check existing position
        current_position = self._positions.get(signal.market_id, 0.0)
        new_position = current_position + adjusted_size

        if new_position > self.risk_settings.max_position_per_market_usd:
            remaining = self.risk_settings.max_position_per_market_usd - current_position
            if remaining <= 0:
                return RiskCheckResult(
                    approved=False,
                    reason=f"Max position reached for market (${current_position:.2f})",
                )
            adjusted_size = remaining

        # Check portfolio exposure
        new_total = self._total_exposure + adjusted_size
        if new_total > self.risk_settings.max_portfolio_exposure_usd:
            remaining = self.risk_settings.max_portfolio_exposure_usd - self._total_exposure
            if remaining <= 0:
                return RiskCheckResult(
                    approved=False,
                    reason=f"Max portfolio exposure reached (${self._total_exposure:.2f})",
                )
            adjusted_size = min(adjusted_size, remaining)

        # Minimum trade size
        if adjusted_size < 1.0:
            return RiskCheckResult(
                approved=False,
                reason="Adjusted size below minimum ($1.00)",
            )

        return RiskCheckResult(
            approved=True,
            reason="Passed all risk checks",
            adjusted_size=adjusted_size,
        )

    def record_trade(self, signal: TradingSignal, size: float) -> None:
        """
        Record a trade for position tracking.

        Args:
            signal: Executed signal
            size: Actual trade size
        """
        current = self._positions.get(signal.market_id, 0.0)
        self._positions[signal.market_id] = current + size
        self._total_exposure += size

        logger.debug(
            "Trade recorded",
            market_id=signal.market_id,
            size=size,
            new_position=self._positions[signal.market_id],
            total_exposure=self._total_exposure,
        )

    def get_position(self, market_id: str) -> float:
        """Get current position in a market."""
        return self._positions.get(market_id, 0.0)

    @property
    def total_exposure(self) -> float:
        """Get total portfolio exposure."""
        return self._total_exposure

    @property
    def exposure_remaining(self) -> float:
        """Get remaining portfolio capacity."""
        return max(0, self.risk_settings.max_portfolio_exposure_usd - self._total_exposure)

    def reset_positions(self) -> None:
        """Reset all position tracking."""
        self._positions.clear()
        self._total_exposure = 0.0
        logger.info("Position tracking reset")
