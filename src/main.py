"""Main entry point for the Polymarket trading bot."""

import asyncio
import signal
import sys
from typing import Optional

from rich.console import Console
from rich.table import Table

from src.config import get_settings, Settings
from src.markets.embeddings import MarketEmbeddings
from src.markets.indexer import MarketIndexer
from src.markets.matcher import MarketMatcher
from src.news.aggregator import NewsAggregator
from src.signals.analyzer import SignalAnalyzer
from src.signals.models import TradingSignal, TradeDecision
from src.trading.client import PolymarketClient
from src.utils.logging import get_logger, setup_logging

logger = get_logger(__name__)
console = Console()


class PolymarketBot:
    """
    Main bot orchestrator.

    Coordinates all components:
    - Market indexing
    - News aggregation
    - Signal generation
    - Trade execution (paper or live)
    """

    def __init__(self, settings: Settings) -> None:
        """
        Initialize the bot with settings.

        Args:
            settings: Bot configuration
        """
        self.settings = settings
        self._running = False

        # Initialize components
        self.polymarket_client = PolymarketClient(settings.polymarket)
        self.market_indexer = MarketIndexer(settings.markets)
        self.embeddings = MarketEmbeddings()
        self.news_aggregator = NewsAggregator(settings.news)
        self.signal_analyzer = SignalAnalyzer(
            settings.llm,
            settings.signals,
            settings.risk,
            grok_api_key=settings.news.grok_api_key,  # Enable X sentiment edge
        )

        # Market matcher (initialized after markets are loaded)
        self.market_matcher: Optional[MarketMatcher] = None

        # Track paper trades
        self.paper_trades: list[TradingSignal] = []
        self.trade_decisions: list[TradeDecision] = []

    async def start(self) -> None:
        """Start the bot."""
        logger.info("Starting Polymarket Bot...")
        console.print("[bold green]Starting Polymarket Bot...[/bold green]")

        # Print configuration
        self._print_config()

        # Check API health
        if self.polymarket_client.is_healthy():
            console.print("[green]Polymarket API: OK[/green]")
        else:
            console.print("[red]Polymarket API: FAILED[/red]")
            return

        # Authenticate if credentials provided
        if self.settings.polymarket.private_key:
            if self.polymarket_client.authenticate():
                console.print("[green]Authentication: OK[/green]")
            else:
                console.print("[yellow]Authentication: FAILED (read-only mode)[/yellow]")

        # Initial market index
        await self._refresh_markets()

        # Initialize market matcher
        self.market_matcher = MarketMatcher(
            self.embeddings,
            {m.condition_id: m for m in self.market_indexer.markets},
            self.settings.llm,
            self.settings.markets,
        )

        # Start main loop
        self._running = True
        await self._main_loop()

    async def stop(self) -> None:
        """Stop the bot gracefully."""
        logger.info("Stopping bot...")
        self._running = False

    async def _main_loop(self) -> None:
        """Main processing loop."""
        console.print("\n[bold]Entering main loop...[/bold]")
        console.print("Press Ctrl+C to stop\n")

        while self._running:
            try:
                # Refresh markets if needed
                if self.market_indexer.needs_refresh():
                    await self._refresh_markets()
                    if self.market_matcher:
                        self.market_matcher.update_markets(
                            {m.condition_id: m for m in self.market_indexer.markets}
                        )

                # Fetch news if needed
                if self.news_aggregator.needs_fetch():
                    await self._process_news()

                # Wait before next iteration
                await asyncio.sleep(10)

            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Error in main loop", error=str(e))
                await asyncio.sleep(30)

        # Print summary on exit
        self._print_summary()

    async def _refresh_markets(self) -> None:
        """Refresh the market index."""
        console.print("[dim]Refreshing markets...[/dim]")

        await self.market_indexer.refresh()
        markets = self.market_indexer.markets

        if markets:
            # Index into vector DB
            self.embeddings.index_markets(markets)
            console.print(f"[green]Indexed {len(markets)} markets[/green]")
        else:
            console.print("[yellow]No markets found[/yellow]")

    async def _process_news(self) -> None:
        """Fetch and process news articles."""
        console.print("[dim]Fetching news...[/dim]")

        articles = await self.news_aggregator.fetch_all()

        if not articles:
            console.print("[dim]No new articles[/dim]")
            return

        console.print(f"[blue]Processing {len(articles)} new articles...[/blue]")

        for article in articles:
            await self._analyze_article(article)

    async def _analyze_article(self, article) -> None:
        """Analyze a single article and generate signals."""
        if not self.market_matcher:
            return

        # Match to markets
        matches = await self.market_matcher.match(article)

        if not matches:
            return

        # Generate signals
        result = await self.signal_analyzer.analyze(article, matches)

        if result.error:
            logger.warning(f"Analysis error: {result.error}")
            return

        # Process actionable signals
        for signal in result.actionable_signals:
            # Find the matching market for additional info
            market = None
            for match in matches:
                if match.market.condition_id == signal.market_id:
                    market = match.market
                    break

            await self._handle_signal(signal, article, market)

    async def _handle_signal(self, signal: TradingSignal, article=None, market=None) -> None:
        """Handle an actionable trading signal."""
        from datetime import datetime, timezone
        import uuid

        # Calculate news age
        time_since_news = 0.0
        if article and article.published_at:
            now = datetime.now(timezone.utc)
            # Handle both aware and naive datetimes
            if article.published_at.tzinfo is None:
                pub_time = article.published_at.replace(tzinfo=timezone.utc)
            else:
                pub_time = article.published_at
            time_since_news = (now - pub_time).total_seconds() / 60.0

        # Determine news freshness
        if time_since_news < 15:
            freshness = "BREAKING"
        elif time_since_news < 60:
            freshness = "RECENT"
        else:
            freshness = "STALE"

        # Determine action
        if signal.direction.value == "YES":
            action = "BUY_YES"
        elif signal.direction.value == "NO":
            action = "BUY_NO"
        else:
            action = "HOLD"

        # Create structured trade decision
        decision = TradeDecision(
            decision_id=str(uuid.uuid4())[:8],
            news_headline=article.title if article else "",
            news_source=article.source if article else "",
            news_published_at=article.published_at if article else None,
            time_since_news_minutes=time_since_news,
            market_question=signal.market_question,
            market_condition_id=signal.market_id,
            current_yes_price=signal.current_yes_price,
            current_no_price=signal.current_no_price,
            market_liquidity_usd=market.liquidity if market else 0.0,
            action=action,
            position_size_usd=signal.suggested_size_usd,
            target_price=signal.current_yes_price if action == "BUY_YES" else signal.current_no_price,
            edge_summary=f"{freshness} news impacts {signal.direction.value} probability",
            confidence_score=signal.confidence,
            reasoning=signal.reasoning,
            news_freshness=freshness,
            source_credibility="HIGH" if article and article.source in ["Reuters", "AP", "BBC", "CNN"] else "MEDIUM",
            is_paper_trade=self.settings.paper_trading,
        )

        # Store and print the decision
        self.trade_decisions.append(decision)
        console.print(str(decision))

        if self.settings.paper_trading:
            # Paper trade - just log
            self.paper_trades.append(signal)
            logger.info(
                "Paper trade recorded",
                market=signal.market_question[:50],
                direction=signal.direction.value,
                size=signal.suggested_size_usd,
                edge=decision.edge_summary,
            )
        else:
            # Live trade
            await self._execute_trade(signal)

    async def _execute_trade(self, signal: TradingSignal) -> None:
        """Execute a live trade."""
        if not signal.target_token_id:
            logger.warning("No target token ID for signal")
            return

        result = self.polymarket_client.place_market_order(
            token_id=signal.target_token_id,
            side="BUY",
            amount_usd=signal.suggested_size_usd,
        )

        if result.success:
            console.print(f"[green]Order placed: {result.order_id}[/green]")
        else:
            console.print(f"[red]Order failed: {result.message}[/red]")

    def _print_config(self) -> None:
        """Print configuration summary."""
        table = Table(title="Configuration")
        table.add_column("Setting", style="cyan")
        table.add_column("Value", style="green")

        table.add_row("Mode", "PAPER TRADING" if self.settings.paper_trading else "LIVE")
        table.add_row("LLM Model", self.settings.llm.model)
        table.add_row("Confidence Threshold", str(self.settings.signals.confidence_threshold))
        table.add_row("Max Position/Market", f"${self.settings.risk.max_position_per_market_usd}")
        table.add_row("Max Portfolio", f"${self.settings.risk.max_portfolio_exposure_usd}")
        table.add_row("News Poll Interval", f"{self.settings.news.poll_interval_seconds}s")

        console.print(table)
        console.print()

    def _print_signal(self, signal: TradingSignal) -> None:
        """Print a trading signal."""
        direction_color = "green" if signal.direction.value == "YES" else "red"

        console.print(f"\n[bold]SIGNAL: {signal.market_question[:60]}...[/bold]")
        console.print(f"  Direction: [{direction_color}]{signal.direction.value}[/{direction_color}]")
        console.print(f"  Confidence: {signal.confidence:.2%}")
        console.print(f"  Size: ${signal.suggested_size_usd:.2f}")
        console.print(f"  Reason: {signal.reasoning[:100]}...")

    def _print_summary(self) -> None:
        """Print session summary."""
        console.print("\n[bold]Session Summary[/bold]")

        if self.paper_trades:
            table = Table(title="Paper Trades")
            table.add_column("Market", style="cyan", max_width=40)
            table.add_column("Direction", style="green")
            table.add_column("Confidence")
            table.add_column("Size")

            for trade in self.paper_trades[-10:]:  # Last 10
                table.add_row(
                    trade.market_question[:40],
                    trade.direction.value,
                    f"{trade.confidence:.2%}",
                    f"${trade.suggested_size_usd:.2f}",
                )

            console.print(table)
            console.print(f"\nTotal paper trades: {len(self.paper_trades)}")
        else:
            console.print("No trades executed this session")


def main() -> None:
    """Main entry point."""
    # Load settings
    settings = get_settings()

    # Setup logging
    setup_logging(
        level=settings.logging.level,
        log_file=settings.logging.file,
    )

    # Create and run bot
    bot = PolymarketBot(settings)

    # Handle shutdown
    def shutdown_handler(sig, frame):
        console.print("\n[yellow]Shutting down...[/yellow]")
        asyncio.create_task(bot.stop())

    signal.signal(signal.SIGINT, shutdown_handler)
    signal.signal(signal.SIGTERM, shutdown_handler)

    # Run
    try:
        asyncio.run(bot.start())
    except KeyboardInterrupt:
        console.print("\n[yellow]Interrupted[/yellow]")
    except Exception as e:
        console.print(f"\n[red]Fatal error: {e}[/red]")
        logger.exception("Fatal error")
        sys.exit(1)


if __name__ == "__main__":
    main()
