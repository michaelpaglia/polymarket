"""Tests for configuration module."""

import pytest

from src.config import Settings, get_settings


def test_default_settings():
    """Test that default settings are valid."""
    settings = Settings()

    assert settings.paper_trading is True
    assert settings.risk.max_position_per_market_usd > 0
    assert settings.signals.confidence_threshold > 0


def test_paper_trading_default():
    """Test that paper trading is enabled by default."""
    settings = Settings()
    assert settings.paper_trading is True, "Paper trading should be enabled by default"
