"""
HFT Module - Python client for Rust HFT trading module.

This module provides a REST client for communicating with the
Rust HFT server for high-frequency arbitrage trading.
"""

from .client import HftClient, HftStatus, HftStats, PnlSummary

__all__ = ["HftClient", "HftStatus", "HftStats", "PnlSummary"]
