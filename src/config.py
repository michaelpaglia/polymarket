"""Configuration management for the Polymarket bot."""

from pathlib import Path
from typing import Any

import yaml
from dotenv import load_dotenv
from pydantic import Field
from pydantic_settings import BaseSettings, SettingsConfigDict

# Load .env file at module import
load_dotenv()


class PolymarketSettings(BaseSettings):
    """Polymarket API credentials."""

    model_config = SettingsConfigDict(env_file=".env", extra="ignore")

    private_key: str = Field(default="", alias="POLYMARKET_PRIVATE_KEY")
    funder_address: str = Field(default="", alias="POLYMARKET_FUNDER_ADDRESS")
    host: str = "https://clob.polymarket.com"
    chain_id: int = 137  # Polygon mainnet


class LLMSettings(BaseSettings):
    """LLM API settings."""

    model_config = SettingsConfigDict(env_file=".env", extra="ignore")

    google_api_key: str = Field(default="", alias="GOOGLE_API_KEY")
    provider: str = "google"
    model: str = "gemini-3-flash-preview"


class NewsSettings(BaseSettings):
    """News API settings."""

    model_config = SettingsConfigDict(env_file=".env", extra="ignore")

    newsapi_key: str = Field(default="", alias="NEWSAPI_KEY")
    gnews_api_key: str = Field(default="", alias="GNEWS_API_KEY")
    grok_api_key: str = Field(default="", alias="GROK_API_KEY")
    poll_interval_seconds: int = 60
    sources: list[str] = ["newsapi", "gnews", "grok"]


class MarketSettings(BaseSettings):
    """Market matching settings."""

    refresh_interval_minutes: int = 15
    min_liquidity_usd: float = 1000.0
    max_days_to_resolution: int = 30
    top_k_matches: int = 5


class SignalSettings(BaseSettings):
    """Signal generation settings."""

    # Threshold aligned with signal model:
    # - 0.10-0.20: Moderate signal (25% position)
    # - 0.20-0.35: Good signal (50% position)
    # - 0.35-0.50: Strong signal (75% position)
    # - 0.50+: Very strong signal (100% position)
    confidence_threshold: float = 0.10
    require_multi_source: bool = False  # X sentiment is enough


class RiskSettings(BaseSettings):
    """Risk management settings."""

    max_position_per_market_usd: float = 25.0  # Max $25 per trade
    max_position_pct: float = 0.25  # Max 25% of capital per trade
    max_portfolio_exposure_usd: float = 500.0  # Will be overridden by balance * allocation
    max_daily_trades: int = 20
    stop_loss_pct: float = 0.25
    # Balance allocation for Python module (use --allocation flag to override)
    balance_allocation_pct: float = 1.0  # 100% default


class ProxySettings(BaseSettings):
    """Proxy configuration for bypassing geo-restrictions."""

    model_config = SettingsConfigDict(env_file=".env", extra="ignore")

    enabled: bool = True
    # Primary residential proxy (from .env)
    host: str = Field(default="", alias="PROXY_HOST")
    port: str = Field(default="", alias="PROXY_PORT")
    user: str = Field(default="", alias="PROXY_USER")
    password: str = Field(default="", alias="PROXY_PASS")
    # Fallback proxy file
    fallback_file: str = "eu_proxies.txt"
    _current_fallback_index: int = 0

    def get_proxy_url(self) -> str:
        """Get formatted proxy URL for httpx/requests."""
        if self.host and self.port and self.user and self.password:
            return f"http://{self.user}:{self.password}@{self.host}:{self.port}"
        return ""

    def get_fallback_proxies(self) -> list[str]:
        """Load fallback proxies from file."""
        from pathlib import Path
        proxy_path = Path(self.fallback_file)
        if not proxy_path.exists():
            return []

        proxies = []
        lines = proxy_path.read_text().strip().split("\n")
        for line in lines:
            line = line.strip()
            if line and not line.startswith("#"):
                parts = line.split(":")
                if len(parts) >= 4:
                    host, port, username = parts[0], parts[1], parts[2]
                    password = ":".join(parts[3:])
                    proxies.append(f"http://{username}:{password}@{host}:{port}")
        return proxies

    def get_next_fallback(self) -> str:
        """Get next fallback proxy (rotates through list)."""
        fallbacks = self.get_fallback_proxies()
        if not fallbacks:
            return ""
        proxy = fallbacks[self._current_fallback_index % len(fallbacks)]
        self._current_fallback_index += 1
        return proxy

    def apply_to_environment(self) -> None:
        """Set HTTP_PROXY and HTTPS_PROXY environment variables."""
        import os
        proxy_url = self.get_proxy_url()
        if proxy_url:
            os.environ["HTTP_PROXY"] = proxy_url
            os.environ["HTTPS_PROXY"] = proxy_url

    def apply_fallback_to_environment(self) -> str:
        """Apply next fallback proxy to environment. Returns the proxy URL."""
        import os
        proxy_url = self.get_next_fallback()
        if proxy_url:
            os.environ["HTTP_PROXY"] = proxy_url
            os.environ["HTTPS_PROXY"] = proxy_url
        return proxy_url


class LoggingSettings(BaseSettings):
    """Logging settings."""

    level: str = "INFO"
    file: str = "logs/bot.log"


class Settings(BaseSettings):
    """Main settings container."""

    model_config = SettingsConfigDict(
        env_file=".env",
        env_file_encoding="utf-8",
        extra="ignore",
    )

    # Sub-settings
    polymarket: PolymarketSettings = Field(default_factory=PolymarketSettings)
    llm: LLMSettings = Field(default_factory=LLMSettings)
    news: NewsSettings = Field(default_factory=NewsSettings)
    markets: MarketSettings = Field(default_factory=MarketSettings)
    signals: SignalSettings = Field(default_factory=SignalSettings)
    risk: RiskSettings = Field(default_factory=RiskSettings)
    logging: LoggingSettings = Field(default_factory=LoggingSettings)
    proxy: ProxySettings = Field(default_factory=ProxySettings)

    # Trading mode
    paper_trading: bool = True


def load_config(config_path: str = "config.yaml") -> dict[str, Any]:
    """Load configuration from YAML file."""
    path = Path(config_path)
    if path.exists():
        with open(path) as f:
            return yaml.safe_load(f)
    return {}


def get_settings(config_path: str = "config.yaml") -> Settings:
    """Get settings from environment and config file."""
    # Load YAML config
    yaml_config = load_config(config_path)

    # Create settings, merging YAML with env vars
    settings = Settings()

    # Override with YAML values if present
    if yaml_config:
        if "paper_trading" in yaml_config:
            settings.paper_trading = yaml_config["paper_trading"]

        if "news" in yaml_config:
            for key, value in yaml_config["news"].items():
                if hasattr(settings.news, key):
                    setattr(settings.news, key, value)

        if "markets" in yaml_config:
            for key, value in yaml_config["markets"].items():
                if hasattr(settings.markets, key):
                    setattr(settings.markets, key, value)

        if "signals" in yaml_config:
            for key, value in yaml_config["signals"].items():
                if hasattr(settings.signals, key):
                    setattr(settings.signals, key, value)
                if key == "llm_provider":
                    settings.llm.provider = value
                if key == "model":
                    settings.llm.model = value

        if "risk" in yaml_config:
            for key, value in yaml_config["risk"].items():
                if hasattr(settings.risk, key):
                    setattr(settings.risk, key, value)

        if "logging" in yaml_config:
            for key, value in yaml_config["logging"].items():
                if hasattr(settings.logging, key):
                    setattr(settings.logging, key, value)

    return settings


# Global settings instance
_settings: Settings | None = None


def get_global_settings() -> Settings:
    """Get or create global settings instance."""
    global _settings
    if _settings is None:
        _settings = get_settings()
    return _settings
