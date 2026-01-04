//! SIMD-accelerated JSON message parser

use crate::messages::{BookLevel, BookMessage, LastTradeMessage, PriceChangeMessage, TickSizeMessage, WsMessage};
use hft_core::HftError;
use simd_json::prelude::*;
use tracing::warn;

/// Parse WebSocket message with SIMD acceleration
pub fn parse_message(text: &str) -> Result<WsMessage, HftError> {
    // Check if message is an array (Polymarket sends arrays of book updates)
    let trimmed = text.trim();
    if trimmed.starts_with('[') {
        return parse_array_message(text);
    }

    // First, try to detect message type from raw text (faster than parsing)
    let event_type = detect_event_type(text);

    match event_type.as_deref() {
        Some("book") => parse_book_message(text),
        Some("price_change") => parse_price_change(text),
        Some("last_trade_price") => parse_last_trade(text),
        Some("tick_size_change") => parse_tick_size(text),
        // Batch price_changes - silently ignore (we use book updates for orderbook state)
        Some("price_changes_batch") => Ok(WsMessage::Unknown(text.to_string())),
        // Trade notifications - silently ignore (we only care about book updates)
        Some("trade_notification") => Ok(WsMessage::Unknown(text.to_string())),
        _ => {
            warn!(text = &text[..text.len().min(200)], "Unknown message type");
            Ok(WsMessage::Unknown(text.to_string()))
        }
    }
}

/// Parse array of messages (Polymarket often sends book updates as arrays)
fn parse_array_message(text: &str) -> Result<WsMessage, HftError> {
    let mut text_bytes = text.as_bytes().to_vec();

    let array: Vec<simd_json::OwnedValue> = simd_json::to_owned_value(&mut text_bytes)
        .map_err(|e| HftError::MessageParse(e.to_string()))?
        .into_array()
        .unwrap_or_default();

    // Process first book message we find
    for value in array {
        let event_type = value
            .get("event_type")
            .and_then(|v| v.as_str());

        if event_type == Some("book") {
            let asset_id = value
                .get("asset_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let market = value
                .get("market")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let timestamp = value
                .get("timestamp")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let hash = value
                .get("hash")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let bids = parse_levels_from_value(value.get("bids"));
            let asks = parse_levels_from_value(value.get("asks"));

            return Ok(WsMessage::Book(BookMessage {
                asset_id,
                market,
                bids,
                asks,
                timestamp,
                hash,
            }));
        }
    }

    Ok(WsMessage::Unknown(text.to_string()))
}

/// Parse all book messages from an array
pub fn parse_all_books(text: &str) -> Vec<BookMessage> {
    let mut books = Vec::new();

    let trimmed = text.trim();
    if !trimmed.starts_with('[') {
        // Single message
        if let Ok(WsMessage::Book(book)) = parse_message(text) {
            books.push(book);
        }
        return books;
    }

    let mut text_bytes = text.as_bytes().to_vec();

    let array: Vec<simd_json::OwnedValue> = match simd_json::to_owned_value(&mut text_bytes) {
        Ok(v) => v.into_array().unwrap_or_default(),
        Err(_) => return books,
    };

    for value in array {
        let event_type = value.get("event_type").and_then(|v| v.as_str());

        if event_type == Some("book") {
            let asset_id = value
                .get("asset_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let market = value
                .get("market")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let timestamp = value
                .get("timestamp")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            let hash = value
                .get("hash")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string());

            let bids = parse_levels_from_value(value.get("bids"));
            let asks = parse_levels_from_value(value.get("asks"));

            books.push(BookMessage {
                asset_id,
                market,
                bids,
                asks,
                timestamp,
                hash,
            });
        }
    }

    books
}

fn parse_levels_from_value(value: Option<&simd_json::OwnedValue>) -> Vec<BookLevel> {
    let mut levels = Vec::new();

    if let Some(arr) = value.and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(obj) = item.as_object() {
                let price = obj
                    .get("price")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0")
                    .to_string();
                let size = obj
                    .get("size")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0")
                    .to_string();
                levels.push(BookLevel { price, size });
            }
        }
    }

    levels
}

/// Fast event type detection without full parsing
fn detect_event_type(text: &str) -> Option<String> {
    // Look for "event_type":"xxx" pattern
    if let Some(start) = text.find("\"event_type\"") {
        let rest = &text[start + 13..]; // Skip past "event_type":"
        if let Some(colon) = rest.find(':') {
            let after_colon = rest[colon + 1..].trim_start();
            if after_colon.starts_with('"') {
                if let Some(end) = after_colon[1..].find('"') {
                    return Some(after_colon[1..end + 1].to_string());
                }
            }
        }
    }

    // Check for batch price_changes format (no event_type field)
    // Format: {"market":"0x...", "price_changes":[...]}
    if text.contains("\"price_changes\"") {
        return Some("price_changes_batch".to_string());
    }

    // Check for book update without event_type field
    // New format: {"market":"0x...", "asset_id":"...", "bids":[...], "asks":[...]}
    // OR just one side: {"market":"0x...", "asset_id":"...", "bids":[...]}
    // IMPORTANT: Must check for bids/asks BEFORE checking for price/size (trade messages also have asset_id)
    if text.contains("\"bids\"") || text.contains("\"asks\"") {
        return Some("book".to_string());
    }

    // Check for single price/trade update: {"market":"...", "asset_id":"...", "price":"...", "size":"..."}
    // These are trade notifications, not orderbook updates - silently ignore them as we use books
    // Also matches newer format with fee_rate_bps field
    let has_price = text.contains("\"price\"");
    let has_size = text.contains("\"size\"");
    let has_asset = text.contains("\"asset_id\"");
    if has_price && has_size && has_asset {
        return Some("trade_notification".to_string());
    }

    // Also check for simpler format without all fields (still a trade if it has market + price)
    if text.contains("\"market\"") && has_price && !text.contains("\"bids\"") && !text.contains("\"asks\"") {
        return Some("trade_notification".to_string());
    }

    None
}

fn parse_book_message(text: &str) -> Result<WsMessage, HftError> {
    // Use simd-json for faster parsing
    let mut text_bytes = text.as_bytes().to_vec();

    let value: simd_json::OwnedValue = simd_json::to_owned_value(&mut text_bytes)
        .map_err(|e| HftError::MessageParse(e.to_string()))?;

    let asset_id = value
        .get("asset_id")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let market = value
        .get("market")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let timestamp = value
        .get("timestamp")
        .and_then(|v| v.as_str())
        .unwrap_or("")
        .to_string();

    let hash = value
        .get("hash")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());

    let bids = parse_levels(value.get("bids"));
    let asks = parse_levels(value.get("asks"));

    Ok(WsMessage::Book(BookMessage {
        asset_id,
        market,
        bids,
        asks,
        timestamp,
        hash,
    }))
}

fn parse_levels(value: Option<&simd_json::OwnedValue>) -> Vec<BookLevel> {
    let mut levels = Vec::new();

    if let Some(arr) = value.and_then(|v| v.as_array()) {
        for item in arr {
            if let Some(obj) = item.as_object() {
                let price = obj
                    .get("price")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0")
                    .to_string();
                let size = obj
                    .get("size")
                    .and_then(|v| v.as_str())
                    .unwrap_or("0")
                    .to_string();
                levels.push(BookLevel { price, size });
            }
        }
    }

    levels
}

fn parse_price_change(text: &str) -> Result<WsMessage, HftError> {
    let msg: PriceChangeMessage = serde_json::from_str(text)
        .map_err(|e| HftError::MessageParse(e.to_string()))?;
    Ok(WsMessage::PriceChange(msg))
}

fn parse_last_trade(text: &str) -> Result<WsMessage, HftError> {
    let msg: LastTradeMessage = serde_json::from_str(text)
        .map_err(|e| HftError::MessageParse(e.to_string()))?;
    Ok(WsMessage::LastTrade(msg))
}

fn parse_tick_size(text: &str) -> Result<WsMessage, HftError> {
    let msg: TickSizeMessage = serde_json::from_str(text)
        .map_err(|e| HftError::MessageParse(e.to_string()))?;
    Ok(WsMessage::TickSize(msg))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_detect_event_type() {
        let msg = r#"{"event_type":"book","asset_id":"123"}"#;
        assert_eq!(detect_event_type(msg), Some("book".to_string()));

        let msg2 = r#"{"event_type":"price_change","price":"0.5"}"#;
        assert_eq!(detect_event_type(msg2), Some("price_change".to_string()));
    }

    #[test]
    fn test_parse_book_message() {
        let msg = r#"{
            "event_type": "book",
            "asset_id": "token123",
            "market": "market456",
            "timestamp": "1234567890",
            "bids": [{"price": "0.45", "size": "100"}],
            "asks": [{"price": "0.55", "size": "200"}]
        }"#;

        let result = parse_message(msg).unwrap();
        if let WsMessage::Book(book) = result {
            assert_eq!(book.asset_id, "token123");
            assert_eq!(book.bids.len(), 1);
            assert_eq!(book.asks.len(), 1);
        } else {
            panic!("Expected Book message");
        }
    }
}
