"""
HFT Client - REST client for Rust HFT trading module.

Provides async interface for controlling the HFT module:
- Start/stop/pause trading
- Set capital allocation
- Get status and statistics
- Subscribe to markets
"""

import httpx
from dataclasses import dataclass
from decimal import Decimal
from typing import Optional
from datetime import datetime


@dataclass
class HftStatus:
    """Current status of the HFT module."""
    state: str
    uptime_seconds: int
    connected: bool
    subscribed_markets: int
    opportunities_detected: int
    trades_executed: int
    current_pnl_usd: Decimal
    latency_avg_us: int
    latency_max_us: int
    circuit_breaker_tripped: bool


@dataclass
class RiskStats:
    """Risk management statistics."""
    total_exposure: Decimal
    available_capital: Decimal
    position_count: int
    checks_passed: int
    checks_failed: int


@dataclass
class PnlSummary:
    """P&L summary statistics."""
    realized_pnl: Decimal
    unrealized_pnl: Decimal
    total_pnl: Decimal
    wins: int
    losses: int
    win_rate: float
    total_trades: int
    avg_profit: Decimal
    max_win: Decimal
    max_loss: Decimal
    profit_factor: float


@dataclass
class CircuitBreakerState:
    """Circuit breaker state."""
    is_tripped: bool
    trip_reason: Optional[str]
    tripped_at: Optional[datetime]
    daily_pnl: Decimal
    consecutive_failures: int


@dataclass
class HftStats:
    """Complete HFT statistics."""
    risk: RiskStats
    pnl: PnlSummary
    circuit_breaker: CircuitBreakerState
    opportunities_detected: int
    trades_executed: int
    latency_avg_us: int
    latency_max_us: int


@dataclass
class BalanceInfo:
    """Balance allocation information for dynamic capital management."""
    account_balance: Decimal  # Total account balance
    max_percentage: Decimal   # Max percentage to use (e.g., 0.5 for 50%)
    max_allowed: Decimal      # Maximum allowed capital
    current_exposure: Decimal # Current exposure in positions
    available_capital: Decimal # Available for new trades
    utilization: Decimal      # Current utilization (exposure / max_allowed)


class HftClient:
    """
    Async REST client for the Rust HFT trading module.

    Example usage:
        async with HftClient() as client:
            status = await client.get_status()
            print(f"State: {status.state}")

            await client.set_capital(5000.0, max_position=500.0)
            await client.start()
    """

    def __init__(
        self,
        base_url: str = "http://127.0.0.1:8080",
        timeout: float = 5.0
    ):
        """
        Initialize HFT client.

        Args:
            base_url: Base URL of the HFT server
            timeout: Request timeout in seconds
        """
        self.base_url = base_url.rstrip("/")
        self.timeout = timeout
        self._client: Optional[httpx.AsyncClient] = None

    async def __aenter__(self) -> "HftClient":
        """Async context manager entry."""
        self._client = httpx.AsyncClient(
            base_url=self.base_url,
            timeout=self.timeout
        )
        return self

    async def __aexit__(self, exc_type, exc_val, exc_tb):
        """Async context manager exit."""
        if self._client:
            await self._client.aclose()
            self._client = None

    @property
    def client(self) -> httpx.AsyncClient:
        """Get HTTP client, creating if needed."""
        if self._client is None:
            self._client = httpx.AsyncClient(
                base_url=self.base_url,
                timeout=self.timeout
            )
        return self._client

    async def close(self):
        """Close the HTTP client."""
        if self._client:
            await self._client.aclose()
            self._client = None

    async def get_status(self) -> HftStatus:
        """
        Get current status of the HFT module.

        Returns:
            HftStatus with current state, P&L, latency, etc.
        """
        resp = await self.client.get("/api/v1/status")
        resp.raise_for_status()
        data = resp.json()

        return HftStatus(
            state=data["state"],
            uptime_seconds=data["uptime_seconds"],
            connected=data["connected"],
            subscribed_markets=data["subscribed_markets"],
            opportunities_detected=data["opportunities_detected"],
            trades_executed=data["trades_executed"],
            current_pnl_usd=Decimal(str(data["current_pnl_usd"])),
            latency_avg_us=data["latency_avg_us"],
            latency_max_us=data["latency_max_us"],
            circuit_breaker_tripped=data["circuit_breaker_tripped"],
        )

    async def start(self) -> bool:
        """
        Start trading.

        Returns:
            True if started successfully, False if already running or blocked.
        """
        resp = await self.client.post("/api/v1/start")
        resp.raise_for_status()
        return resp.json().get("success", False)

    async def stop(self) -> bool:
        """
        Stop trading.

        Returns:
            True if stopped successfully.
        """
        resp = await self.client.post("/api/v1/stop")
        resp.raise_for_status()
        return resp.json().get("success", False)

    async def pause(self) -> bool:
        """
        Pause trading (can be resumed with start).

        Returns:
            True if paused successfully.
        """
        resp = await self.client.post("/api/v1/pause")
        resp.raise_for_status()
        return resp.json().get("success", False)

    async def set_capital(
        self,
        allocation_usd: float,
        max_position_size_usd: Optional[float] = None,
        max_exposure_usd: Optional[float] = None
    ) -> dict:
        """
        Set capital allocation for HFT trading.

        Args:
            allocation_usd: Total capital to allocate to HFT
            max_position_size_usd: Optional max size per position
            max_exposure_usd: Optional max total exposure

        Returns:
            Dict with allocation and available capital
        """
        payload = {"allocation_usd": allocation_usd}
        if max_position_size_usd is not None:
            payload["max_position_size_usd"] = max_position_size_usd
        if max_exposure_usd is not None:
            payload["max_exposure_usd"] = max_exposure_usd

        resp = await self.client.post("/api/v1/capital", json=payload)
        resp.raise_for_status()
        return resp.json()

    async def get_stats(self) -> HftStats:
        """
        Get comprehensive statistics.

        Returns:
            HftStats with risk, P&L, circuit breaker, and performance stats.
        """
        resp = await self.client.get("/api/v1/stats")
        resp.raise_for_status()
        data = resp.json()

        risk = RiskStats(
            total_exposure=Decimal(str(data["risk"]["total_exposure"])),
            available_capital=Decimal(str(data["risk"]["available_capital"])),
            position_count=data["risk"]["position_count"],
            checks_passed=data["risk"]["checks_passed"],
            checks_failed=data["risk"]["checks_failed"],
        )

        pnl = PnlSummary(
            realized_pnl=Decimal(str(data["pnl"]["realized_pnl"])),
            unrealized_pnl=Decimal(str(data["pnl"]["unrealized_pnl"])),
            total_pnl=Decimal(str(data["pnl"]["total_pnl"])),
            wins=data["pnl"]["wins"],
            losses=data["pnl"]["losses"],
            win_rate=data["pnl"]["win_rate"],
            total_trades=data["pnl"]["total_trades"],
            avg_profit=Decimal(str(data["pnl"]["avg_profit"])),
            max_win=Decimal(str(data["pnl"]["max_win"])),
            max_loss=Decimal(str(data["pnl"]["max_loss"])),
            profit_factor=data["pnl"]["profit_factor"],
        )

        cb_data = data["circuit_breaker"]
        circuit_breaker = CircuitBreakerState(
            is_tripped=cb_data["is_tripped"],
            trip_reason=cb_data.get("trip_reason"),
            tripped_at=datetime.fromisoformat(cb_data["tripped_at"]) if cb_data.get("tripped_at") else None,
            daily_pnl=Decimal(str(cb_data["daily_pnl"])),
            consecutive_failures=cb_data["consecutive_failures"],
        )

        return HftStats(
            risk=risk,
            pnl=pnl,
            circuit_breaker=circuit_breaker,
            opportunities_detected=data["opportunities_detected"],
            trades_executed=data["trades_executed"],
            latency_avg_us=data["latency_avg_us"],
            latency_max_us=data["latency_max_us"],
        )

    async def get_pnl(self) -> PnlSummary:
        """
        Get P&L summary.

        Returns:
            PnlSummary with realized/unrealized P&L, win rate, etc.
        """
        resp = await self.client.get("/api/v1/stats/pnl")
        resp.raise_for_status()
        data = resp.json()

        return PnlSummary(
            realized_pnl=Decimal(str(data["realized_pnl"])),
            unrealized_pnl=Decimal(str(data["unrealized_pnl"])),
            total_pnl=Decimal(str(data["total_pnl"])),
            wins=data["wins"],
            losses=data["losses"],
            win_rate=data["win_rate"],
            total_trades=data["total_trades"],
            avg_profit=Decimal(str(data["avg_profit"])),
            max_win=Decimal(str(data["max_win"])),
            max_loss=Decimal(str(data["max_loss"])),
            profit_factor=data["profit_factor"],
        )

    async def reset_circuit_breaker(self) -> bool:
        """
        Reset the circuit breaker.

        Returns:
            True if reset successfully.
        """
        resp = await self.client.post("/api/v1/circuit-breaker/reset")
        resp.raise_for_status()
        return resp.json().get("success", False)

    async def health_check(self) -> bool:
        """
        Check if HFT server is healthy.

        Returns:
            True if server is responding.
        """
        try:
            resp = await self.client.get("/health")
            return resp.status_code == 200
        except Exception:
            return False

    async def is_running(self) -> bool:
        """
        Check if HFT trading is currently running.

        Returns:
            True if trading is active.
        """
        try:
            status = await self.get_status()
            return status.state == "running"
        except Exception:
            return False

    async def sync_balance(self, account_balance_usd: float) -> BalanceInfo:
        """
        Sync account balance with HFT module.

        This updates the available capital based on the 50% cap rule.
        Call this periodically or after deposits/withdrawals.

        Args:
            account_balance_usd: Total account balance in USD

        Returns:
            BalanceInfo with updated allocation details
        """
        resp = await self.client.post(
            "/api/v1/balance/sync",
            json={"account_balance_usd": account_balance_usd}
        )
        resp.raise_for_status()
        data = resp.json()["balance_info"]

        return BalanceInfo(
            account_balance=Decimal(str(data["account_balance"])),
            max_percentage=Decimal(str(data["max_percentage"])),
            max_allowed=Decimal(str(data["max_allowed"])),
            current_exposure=Decimal(str(data["current_exposure"])),
            available_capital=Decimal(str(data["available_capital"])),
            utilization=Decimal(str(data["utilization"])),
        )

    async def get_balance_info(self) -> BalanceInfo:
        """
        Get current balance allocation info.

        Returns:
            BalanceInfo with current allocation state
        """
        resp = await self.client.get("/api/v1/balance/info")
        resp.raise_for_status()
        data = resp.json()

        return BalanceInfo(
            account_balance=Decimal(str(data["account_balance"])),
            max_percentage=Decimal(str(data["max_percentage"])),
            max_allowed=Decimal(str(data["max_allowed"])),
            current_exposure=Decimal(str(data["current_exposure"])),
            available_capital=Decimal(str(data["available_capital"])),
            utilization=Decimal(str(data["utilization"])),
        )

    async def set_balance_percentage(self, percentage: float) -> BalanceInfo:
        """
        Set the maximum percentage of account balance to use.

        Args:
            percentage: Percentage as decimal (0.5 = 50%, 0.25 = 25%)

        Returns:
            BalanceInfo with updated allocation
        """
        resp = await self.client.post(
            "/api/v1/balance/percentage",
            json={"percentage": percentage}
        )
        resp.raise_for_status()
        data = resp.json()["balance_info"]

        return BalanceInfo(
            account_balance=Decimal(str(data["account_balance"])),
            max_percentage=Decimal(str(data["max_percentage"])),
            max_allowed=Decimal(str(data["max_allowed"])),
            current_exposure=Decimal(str(data["current_exposure"])),
            available_capital=Decimal(str(data["available_capital"])),
            utilization=Decimal(str(data["utilization"])),
        )

    async def paper_trade(
        self,
        market_id: Optional[str] = None,
        yes_price: Optional[float] = None,
        no_price: Optional[float] = None,
        size_usd: Optional[float] = None,
    ) -> dict:
        """
        Execute a simulated paper trade for testing.

        This tests the arbitrage logic without placing real orders.
        Default values show a profitable 3% spread.

        Args:
            market_id: Market identifier (default: "paper-test-market")
            yes_price: YES token price (default: 0.45)
            no_price: NO token price (default: 0.52)
            size_usd: Trade size in USD (default: 100)

        Returns:
            Dict with execution details including:
            - success: Whether arbitrage opportunity existed
            - spread_bps: Spread in basis points
            - expected_profit_usd: Expected profit
            - execution_time_us: Execution time in microseconds
            - message: Human-readable description
        """
        payload = {}
        if market_id is not None:
            payload["market_id"] = market_id
        if yes_price is not None:
            payload["yes_price"] = yes_price
        if no_price is not None:
            payload["no_price"] = no_price
        if size_usd is not None:
            payload["size_usd"] = size_usd

        resp = await self.client.post("/api/v1/test/paper-trade", json=payload)
        resp.raise_for_status()
        return resp.json()


async def coordinate_capital(
    hft_client: HftClient,
    total_capital: float,
    sentiment_pct: float = 0.5,
    hft_pct: float = 0.5,
) -> dict:
    """
    Coordinate capital allocation between sentiment bot and HFT.

    Args:
        hft_client: HFT client instance
        total_capital: Total capital available
        sentiment_pct: Percentage for sentiment trading (0.0 - 1.0)
        hft_pct: Percentage for HFT trading (0.0 - 1.0)

    Returns:
        Dict with allocation details
    """
    assert sentiment_pct + hft_pct <= 1.0, "Allocations cannot exceed 100%"

    sentiment_capital = total_capital * sentiment_pct
    hft_capital = total_capital * hft_pct

    # Set HFT capital allocation
    result = await hft_client.set_capital(
        allocation_usd=hft_capital,
        max_position_size_usd=hft_capital * 0.1,  # 10% max per position
        max_exposure_usd=hft_capital,
    )

    return {
        "total_capital": total_capital,
        "sentiment_capital": sentiment_capital,
        "hft_capital": hft_capital,
        "hft_available": result.get("available_usd", 0),
    }


async def start_balance_sync_task(
    hft_client: HftClient,
    get_balance_func,
    sync_interval_seconds: float = 60.0,
    hft_percentage: float = 0.50,
):
    """
    Start a background task that syncs account balance periodically.

    This ensures the HFT module always uses the correct 50% (or specified)
    of the account balance, automatically scaling with profits/losses.

    Args:
        hft_client: HFT client instance
        get_balance_func: Async function that returns current account balance in USD
        sync_interval_seconds: How often to sync (default: 60 seconds)
        hft_percentage: Percentage of balance for HFT (default: 0.50 for 50%)

    Returns:
        asyncio.Task that can be cancelled to stop syncing

    Example:
        async def get_my_balance():
            # Your logic to get account balance from Polymarket
            return 10000.0  # $10,000

        task = await start_balance_sync_task(hft_client, get_my_balance)
        # ... later ...
        task.cancel()
    """
    import asyncio

    # Set the percentage cap
    await hft_client.set_balance_percentage(hft_percentage)

    async def sync_loop():
        while True:
            try:
                balance = await get_balance_func()
                if balance and balance > 0:
                    info = await hft_client.sync_balance(balance)
                    print(f"[Balance Sync] Account: ${balance:.2f}, "
                          f"HFT Max: ${info.max_allowed:.2f}, "
                          f"Available: ${info.available_capital:.2f}, "
                          f"Utilization: {info.utilization * 100:.1f}%")
            except Exception as e:
                print(f"[Balance Sync] Error: {e}")

            await asyncio.sleep(sync_interval_seconds)

    return asyncio.create_task(sync_loop())
