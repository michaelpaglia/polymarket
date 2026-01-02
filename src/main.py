"""Main entry point for the Polymarket trading bot."""

import argparse
import asyncio
import os
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
from src.trading.positions import PositionTracker, PositionSide, ExitReason
from src.utils.logging import get_logger, setup_logging
from src.intelligence.dynamic_graph import DynamicKnowledgeGraph
from src.intelligence.twitter_intel import TwitterIntelligence, TweetSignal

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

        # Load proxy configuration
        self.proxy_url = settings.proxy.get_proxy_url() if settings.proxy.enabled else ""
        if self.proxy_url:
            logger.info("EU proxy enabled for API calls")

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
            proxy_url=self.proxy_url,  # EU proxy for geo-bypass
        )

        # Market matcher (initialized after markets are loaded)
        self.market_matcher: Optional[MarketMatcher] = None

        # Dynamic knowledge graph - auto-expands from market data
        self.knowledge_graph = DynamicKnowledgeGraph()

        # Twitter intelligence for alpha scanning
        self.twitter_intel: Optional[TwitterIntelligence] = None
        if settings.news.grok_api_key:
            self.twitter_intel = TwitterIntelligence(
                grok_api_key=settings.news.grok_api_key,
                knowledge_graph=None,  # Will use dynamic topics
            )

        # Position tracker - Python module gets 50% of balance allocation
        # (Rust module will use the other 50%)
        max_exposure = settings.risk.max_portfolio_exposure_usd
        self.position_tracker = PositionTracker(
            max_positions=50,  # Can hold up to 50 positions
            max_exposure_usd=max_exposure,  # Will be updated with 50% of balance
        )
        self.balance_allocation_pct = settings.risk.balance_allocation_pct

        # Track paper trades
        self.paper_trades: list[TradingSignal] = []
        self.trade_decisions: list[TradeDecision] = []

        # Scan intervals (seconds)
        self._last_alpha_scan: Optional[float] = None
        self._alpha_scan_interval = 180  # 3 minutes (more frequent)
        self._last_position_check: Optional[float] = None
        self._position_check_interval = 60  # Check positions every minute
        self._last_balance_refresh: Optional[float] = None
        self._balance_refresh_interval = 300  # Refresh balance every 5 minutes

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

                # Show balance and set 50% allocation for live trading
                if not self.settings.paper_trading:
                    balance = self.polymarket_client.get_balance()
                    if balance > 0:
                        # Python module gets 50% of balance (Rust gets the other 50%)
                        python_allocation = balance * self.balance_allocation_pct
                        self.position_tracker.max_exposure_usd = python_allocation
                        console.print(f"[green]USDC Balance: ${balance:.2f}[/green]")
                        console.print(f"[cyan]Python module allocation: ${python_allocation:.2f} ({self.balance_allocation_pct:.0%})[/cyan]")
                        console.print(f"[cyan]Rust module reserved: ${balance - python_allocation:.2f}[/cyan]")
                    else:
                        console.print("[red]WARNING: No USDC balance detected![/red]")
                        console.print("[yellow]You need USDC on Polygon to trade.[/yellow]")
                        console.print("[yellow]Switching to paper trading mode.[/yellow]")
                        self.settings.paper_trading = True
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

        import time

        while self._running:
            try:
                current_time = time.time()

                # Refresh markets if needed
                if self.market_indexer.needs_refresh():
                    await self._refresh_markets()
                    if self.market_matcher:
                        self.market_matcher.update_markets(
                            {m.condition_id: m for m in self.market_indexer.markets}
                        )
                    # Extract entities from ALL markets dynamically
                    self.knowledge_graph.extract_entities_from_markets(
                        self.market_indexer.markets
                    )
                    console.print(f"[dim]Knowledge graph: {len(self.knowledge_graph.entities)} entities tracked[/dim]")

                # Fetch news if needed
                if self.news_aggregator.needs_fetch():
                    await self._process_news()

                # Check positions for exit conditions (every minute)
                if (
                    self._last_position_check is None
                    or current_time - self._last_position_check >= self._position_check_interval
                ):
                    await self._check_positions()
                    self._last_position_check = current_time

                # Twitter alpha scan (every 3 minutes)
                if self.twitter_intel and (
                    self._last_alpha_scan is None
                    or current_time - self._last_alpha_scan >= self._alpha_scan_interval
                ):
                    await self._scan_twitter_alpha()
                    self._last_alpha_scan = current_time

                # Refresh balance (every 5 minutes) - keeps allocation accurate
                if not self.settings.paper_trading and (
                    self._last_balance_refresh is None
                    or current_time - self._last_balance_refresh >= self._balance_refresh_interval
                ):
                    await self._refresh_balance()
                    self._last_balance_refresh = current_time

                # Wait before next iteration
                await asyncio.sleep(10)

            except asyncio.CancelledError:
                break
            except Exception as e:
                logger.error("Error in main loop", error=str(e))
                await asyncio.sleep(30)

        # Print summary on exit
        self._print_summary()

    async def _refresh_balance(self) -> None:
        """Refresh balance from Polymarket and update allocation.

        This keeps the available capital accurate during runtime,
        especially after opening/closing positions.
        """
        try:
            balance = await asyncio.to_thread(self.polymarket_client.get_balance)
            if balance > 0:
                old_allocation = self.position_tracker.max_exposure_usd
                new_allocation = balance * self.balance_allocation_pct
                self.position_tracker.max_exposure_usd = new_allocation

                # Only log if there's a meaningful change (> $0.50)
                if abs(new_allocation - old_allocation) > 0.50:
                    console.print(
                        f"[dim]Balance refreshed: ${balance:.2f} → "
                        f"Allocation: ${new_allocation:.2f} ({self.balance_allocation_pct:.0%})[/dim]"
                    )
            else:
                logger.warning("Balance refresh returned $0")
        except Exception as e:
            logger.error("Failed to refresh balance", error=str(e))

    async def _check_positions(self) -> None:
        """Check all positions for exit conditions."""
        if not self.position_tracker.positions:
            return

        # Update prices from current market data
        market_prices = {}
        active_market_ids = set()
        for market in self.market_indexer.markets:
            market_prices[market.condition_id] = (market.yes_price, market.no_price)
            active_market_ids.add(market.condition_id)

        self.position_tracker.update_prices(market_prices)

        # Check for resolved markets (position's market no longer active)
        resolved_positions = []
        for position in list(self.position_tracker.positions.values()):
            if position.market_id not in active_market_ids:
                resolved_positions.append(position)

        # Close resolved positions
        for position in resolved_positions:
            # Market resolved - use last known price or 1.0 if won, 0 if lost
            # Since we can't know outcome, use 0.5 as neutral (balance refresh will show actual)
            # Market resolved - balance refresh will show actual payout
            # Use 1.0 if likely won (current_price > 0.5), 0 if likely lost
            likely_won = position.current_price > 0.5
            exit_price = 1.0 if likely_won else 0.0
            closed = self.position_tracker.close_position(
                position.position_id,
                exit_price=exit_price,  # Approximate outcome based on last known price
                exit_reason=ExitReason.MARKET_CLOSED,
            )
            if closed:
                console.print(
                    f"[cyan]RESOLVED: {position.market_question[:40]}... "
                    f"Market closed - check balance for actual payout[/cyan]"
                )

        # Check for exits
        exits = self.position_tracker.check_all_exits()

        for position, exit_reason in exits:
            # Get current price for exit
            current_price = position.current_price

            # Close the position
            closed = self.position_tracker.close_position(
                position.position_id,
                exit_price=current_price,
                exit_reason=exit_reason,
            )

            if closed:
                console.print(
                    f"[yellow]CLOSED: {position.market_question[:40]}... "
                    f"P&L: ${closed.realized_pnl_usd:+.2f} ({closed.realized_pnl_pct:+.1%}) "
                    f"- {exit_reason.value}[/yellow]"
                )

        # Print position summary periodically
        if self.position_tracker.positions:
            summary = self.position_tracker.get_summary()
            paper_info = f"Paper: {summary['paper_positions']}" if summary['paper_positions'] > 0 else ""
            live_info = f"Live: {summary['live_positions']}" if summary['live_positions'] > 0 else ""
            positions_info = " | ".join(filter(None, [paper_info, live_info])) or "0"
            console.print(
                f"[dim]Positions: {positions_info} | "
                f"P&L: ${summary['unrealized_pnl_usd']:+.2f} | "
                f"Live Available: ${summary['available_capital_usd']:.2f}[/dim]"
            )

    async def _scan_twitter_alpha(self) -> None:
        """
        Scan Twitter/X for alpha signals.

        This runs independently of news articles - looks for:
        1. Breaking news before mainstream media
        2. Influencer posts that move markets
        3. Viral content that could shift sentiment
        """
        if not self.twitter_intel:
            return

        console.print("[dim]Scanning X for alpha...[/dim]")

        try:
            # Get top topics dynamically from knowledge graph
            # These are extracted from ALL market questions
            topics = self.knowledge_graph.get_search_topics(limit=30)

            if not topics:
                # Fallback to broad topics
                topics = ["politics", "crypto", "elections", "economy", "sports"]

            # Scan for signals
            signals = await self.twitter_intel.scan_for_alpha(topics=topics)

            if not signals:
                console.print("[dim]No alpha signals detected[/dim]")
                return

            console.print(f"[blue]Found {len(signals)} Twitter signals[/blue]")

            # Process each signal
            for signal in signals:
                await self._process_twitter_signal(signal)

        except Exception as e:
            logger.warning(f"Twitter alpha scan failed: {e}")

    async def _process_twitter_signal(self, signal: TweetSignal) -> None:
        """Process a Twitter signal and potentially generate a trade."""
        from datetime import datetime, timezone
        import uuid

        # Find relevant markets using knowledge graph
        relevant_markets = self.knowledge_graph.find_markets_for_text(
            signal.content_summary
        )
        # Convert to list of tuples (market_id, relevance)
        relevant_markets = [(mid, 0.8) for mid in relevant_markets]

        if not relevant_markets and self.market_matcher:
            # Fallback: use vector similarity
            from src.news.models import NewsArticle

            fake_article = NewsArticle(
                id=str(uuid.uuid4()),
                title=signal.content_summary,
                description=f"Twitter signal from {signal.source_handle}",
                content=signal.content_summary,
                source_name=f"X/{signal.source_handle}",
                url="",
                published_at=signal.timestamp,
            )

            matches = await self.market_matcher.match(fake_article)
            if matches:
                relevant_markets = [
                    (m.market.condition_id, m.llm_confidence) for m in matches[:3]
                ]

        if not relevant_markets:
            return

        # Create trade decision for high-urgency signals
        if signal.urgency in ["critical", "high"] and signal.relevance_score >= 0.7:
            # Get the top market
            market_id, relevance = relevant_markets[0]
            market = None

            for m in self.market_indexer.markets:
                if m.condition_id == market_id:
                    market = m
                    break

            if not market:
                return

            # Filter out extreme probabilities - no edge in already-decided markets
            # This prevents trades like buying NO at 99.8% where there's no edge
            if market.yes_price >= 0.95:
                console.print(f"[dim]Skipping: YES already at {market.yes_price:.0%} (no edge for bullish)[/dim]")
                return
            if market.yes_price <= 0.05:
                console.print(f"[dim]Skipping: YES already at {market.yes_price:.0%} (no edge for bearish)[/dim]")
                return

            # Determine action based on sentiment with price thresholds
            # (Aligned with signal_model.py direction logic)
            if signal.sentiment == "bullish":
                # Only go long if market hasn't priced it in
                if market.yes_price < 0.75:
                    action = "BUY_YES"
                else:
                    console.print(f"[dim]Skipping bullish: YES at {market.yes_price:.0%} (already priced in)[/dim]")
                    return
            elif signal.sentiment == "bearish":
                # Only go short if market hasn't priced it in
                if market.yes_price > 0.25:
                    action = "BUY_NO"
                else:
                    console.print(f"[dim]Skipping bearish: YES at {market.yes_price:.0%} (already priced in)[/dim]")
                    return
            else:
                action = "HOLD"

            if action == "HOLD":
                return

            # Calculate position size based on confidence
            position_size = min(
                self.settings.risk.max_position_per_market_usd * signal.relevance_score,
                self.settings.risk.max_position_per_market_usd,
            )

            # Check if we can open this position, adjust size if needed
            can_open, reason = self.position_tracker.can_open_position(
                position_size, market.condition_id
            )

            if not can_open:
                # Check if it's a capital issue - use what's available
                if "Insufficient capital" in reason:
                    available = self.position_tracker.available_capital_usd
                    min_trade = 5.0  # Minimum trade size
                    if available >= min_trade:
                        position_size = available
                        console.print(f"[yellow]Reduced Twitter position to ${position_size:.2f} (available)[/yellow]")
                    else:
                        console.print(f"[dim]Skipping Twitter signal: Not enough capital[/dim]")
                        return
                else:
                    console.print(f"[dim]Skipping Twitter signal: {reason}[/dim]")
                    return

            decision = TradeDecision(
                decision_id=str(uuid.uuid4())[:8],
                news_headline=f"[X/{signal.source_handle}] {signal.content_summary[:100]}",
                news_source=f"X/{signal.source_handle}",
                news_published_at=signal.timestamp,
                time_since_news_minutes=0.0,  # Real-time
                market_question=market.question,
                market_condition_id=market.condition_id,
                current_yes_price=market.yes_price,
                current_no_price=market.no_price,
                market_liquidity_usd=market.liquidity,
                action=action,
                position_size_usd=position_size,
                target_price=market.yes_price if action == "BUY_YES" else market.no_price,
                edge_summary=f"[{signal.urgency.upper()}] {signal.signal_type}: {signal.sentiment} sentiment ({signal.sentiment_score:.0%})",
                confidence_score=signal.relevance_score,
                reasoning=f"Twitter signal detected: {signal.signal_type} from {signal.source_handle}. "
                f"Sentiment: {signal.sentiment}. Virality: {signal.virality_potential:.0%}",
                news_freshness="BREAKING" if signal.urgency == "critical" else "RECENT",
                source_credibility="HIGH" if signal.relevance_score >= 0.8 else "MEDIUM",
                is_paper_trade=self.settings.paper_trading,
            )

            self.trade_decisions.append(decision)
            console.print(str(decision))

            # Open position in tracker
            entry_price = market.yes_price if action == "BUY_YES" else market.no_price
            position_side = PositionSide.YES if action == "BUY_YES" else PositionSide.NO

            position = self.position_tracker.open_position(
                position_id=decision.decision_id,
                market_id=market.condition_id,
                market_question=market.question,
                side=position_side,
                entry_price=entry_price,
                size_usd=position_size,
                market_end_date=market.end_date,
                is_paper=self.settings.paper_trading,
                signal_id=f"twitter-{signal.signal_type}",
                reasoning=decision.reasoning,
                confidence=signal.relevance_score,
            )

            if position:
                logger.info(
                    "Position opened from Twitter signal",
                    signal_type=signal.signal_type,
                    source=signal.source_handle,
                    action=action,
                    size=position_size,
                )

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

        # FILTER: Skip stale news (> 60 minutes old)
        max_news_age_minutes = 60.0
        if time_since_news > max_news_age_minutes:
            console.print(f"[dim]Skipping: News too old ({time_since_news:.0f} min > {max_news_age_minutes:.0f} min max)[/dim]")
            return

        # FILTER: Skip extreme probability markets (< 5% or > 95%)
        yes_price = signal.current_yes_price
        if yes_price < 0.05:
            console.print(f"[dim]Skipping: Extreme probability - YES at {yes_price:.1%} (< 5% threshold)[/dim]")
            return
        if yes_price > 0.95:
            console.print(f"[dim]Skipping: Extreme probability - YES at {yes_price:.1%} (> 95% threshold)[/dim]")
            return

        # Determine action
        if signal.direction.value == "YES":
            action = "BUY_YES"
        elif signal.direction.value == "NO":
            action = "BUY_NO"
        else:
            action = "HOLD"

        # Check if we can open this position, adjust size if needed
        trade_size = signal.suggested_size_usd
        can_open, reason = self.position_tracker.can_open_position(
            trade_size, signal.market_id
        )

        if not can_open:
            # Check if it's a capital issue - use what's available
            if "Insufficient capital" in reason:
                available = self.position_tracker.available_capital_usd
                min_trade = 5.0  # Minimum trade size
                if available >= min_trade:
                    trade_size = available
                    console.print(f"[yellow]Reduced position size to ${trade_size:.2f} (available capital)[/yellow]")
                else:
                    console.print(f"[dim]Skipping: Not enough capital (${available:.2f} < ${min_trade:.2f} min)[/dim]")
                    return
            else:
                console.print(f"[dim]Skipping: {reason}[/dim]")
                return

        # Create structured trade decision with adjusted size
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
            position_size_usd=trade_size,
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

        # Open position in tracker
        entry_price = signal.current_yes_price if action == "BUY_YES" else signal.current_no_price
        position_side = PositionSide.YES if action == "BUY_YES" else PositionSide.NO

        position = self.position_tracker.open_position(
            position_id=decision.decision_id,
            market_id=signal.market_id,
            market_question=signal.market_question,
            side=position_side,
            entry_price=entry_price,
            size_usd=trade_size,
            market_end_date=market.end_date if market else None,
            is_paper=self.settings.paper_trading,
            signal_id=signal.signal_id,
            reasoning=signal.reasoning,
            confidence=signal.confidence,
        )

        if position:
            self.paper_trades.append(signal)
            logger.info(
                "Position opened",
                market=signal.market_question[:50],
                direction=signal.direction.value,
                size=trade_size,
                edge=decision.edge_summary,
            )

        if not self.settings.paper_trading:
            # Live trade - use adjusted trade_size, not original suggested_size_usd
            await self._execute_trade(signal, trade_size)

    async def _execute_trade(self, signal: TradingSignal, amount_usd: float) -> None:
        """Execute a live trade."""
        if not signal.target_token_id:
            logger.warning("No target token ID for signal")
            return

        result = self.polymarket_client.place_market_order(
            token_id=signal.target_token_id,
            side="BUY",
            amount_usd=amount_usd,
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

        # Portfolio summary
        summary = self.position_tracker.get_summary()
        console.print("\n[bold]Portfolio Status:[/bold]")
        console.print(f"  Open Positions: {summary['open_positions']}")
        console.print(f"  Total Invested: ${summary['total_invested_usd']:.2f}")
        console.print(f"  Current Value: ${summary['total_value_usd']:.2f}")
        console.print(f"  Unrealized P&L: ${summary['unrealized_pnl_usd']:+.2f} ({summary['unrealized_pnl_pct']:+.1f}%)")
        console.print(f"  Closed Trades: {summary['closed_trades']}")
        console.print(f"  Realized P&L: ${summary['realized_pnl_usd']:+.2f}")
        console.print(f"  Win Rate: {summary['win_rate']:.1f}%")

        # Knowledge graph stats
        console.print(f"\n[bold]Knowledge Graph:[/bold]")
        console.print(f"  Entities Tracked: {len(self.knowledge_graph.entities)}")
        console.print(f"  Influencers Known: {len(self.knowledge_graph.influencers)}")

        if self.paper_trades:
            table = Table(title="Recent Trades")
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
            console.print(f"\nTotal trades this session: {len(self.paper_trades)}")
        else:
            console.print("\nNo trades executed this session")


async def run_bot(settings: Settings) -> None:
    """Run the bot with given settings (async entry point)."""
    bot = PolymarketBot(settings)

    try:
        await bot.start()
    except asyncio.CancelledError:
        console.print("\n[yellow]Cancelled[/yellow]")
    finally:
        await bot.stop()


def main() -> None:
    """Main entry point (CLI)."""
    # Parse command line arguments
    parser = argparse.ArgumentParser(description="Polymarket Trading Bot")
    parser.add_argument(
        "--live",
        action="store_true",
        help="Run in LIVE trading mode (uses real money!)",
    )
    parser.add_argument(
        "--paper",
        action="store_true",
        help="Run in paper trading mode (default)",
    )
    parser.add_argument(
        "--setup",
        action="store_true",
        help="Run interactive setup wizard to configure API keys",
    )
    parser.add_argument(
        "--allocation",
        type=float,
        default=None,
        help="Balance allocation percentage (0.0-1.0). Default: 1.0 (100%%)",
    )
    args = parser.parse_args()

    # Run setup wizard if requested
    if args.setup:
        from src.setup import setup
        setup()
        return

    # Load settings
    settings = get_settings()

    # Apply proxy settings BEFORE any API calls
    if settings.proxy.enabled:
        settings.proxy.apply_to_environment()
        proxy_url = settings.proxy.get_proxy_url()
        if proxy_url:
            console.print(f"[cyan]Proxy enabled: {settings.proxy.host}:{settings.proxy.port}[/cyan]")

    # Override paper_trading based on command line
    if args.live:
        settings.paper_trading = False
        console.print("[bold red]*** LIVE TRADING MODE ***[/bold red]")
        console.print("[yellow]Real money will be used. Press Ctrl+C to cancel.[/yellow]")
        import time
        time.sleep(3)  # Give user time to cancel
    elif args.paper:
        settings.paper_trading = True

    # Override allocation if specified
    if args.allocation is not None:
        if 0.0 <= args.allocation <= 1.0:
            settings.risk.balance_allocation_pct = args.allocation
            console.print(f"[cyan]Balance allocation set to {args.allocation:.0%}[/cyan]")
        else:
            console.print("[red]ERROR: --allocation must be between 0.0 and 1.0[/red]")
            sys.exit(1)

    # Setup logging
    setup_logging(
        level=settings.logging.level,
        log_file=settings.logging.file,
    )

    # Handle shutdown
    def shutdown_handler(sig, frame):
        console.print("\n[yellow]Shutting down...[/yellow]")

    signal.signal(signal.SIGINT, shutdown_handler)
    signal.signal(signal.SIGTERM, shutdown_handler)

    # Run
    try:
        asyncio.run(run_bot(settings))
    except KeyboardInterrupt:
        console.print("\n[yellow]Interrupted[/yellow]")
    except Exception as e:
        console.print(f"\n[red]Fatal error: {e}[/red]")
        logger.exception("Fatal error")
        sys.exit(1)


async def main_async() -> None:
    """Async entry point for use from run_all.py."""
    settings = get_settings()

    # Check for live trading env var (set by run_all.py)
    if os.environ.get("LIVE_TRADING", "").lower() == "true":
        settings.paper_trading = False

    # Apply proxy settings
    if settings.proxy.enabled:
        settings.proxy.apply_to_environment()

    # Setup logging
    setup_logging(
        level=settings.logging.level,
        log_file=settings.logging.file,
    )

    await run_bot(settings)


if __name__ == "__main__":
    main()
