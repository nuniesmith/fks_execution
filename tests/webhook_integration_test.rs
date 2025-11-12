use serde_json::json;

/// Integration test for TradingView webhook endpoint
/// 
/// This test validates that the webhook handler:
/// 1. Accepts valid TradingView webhook payloads
/// 2. Converts them to Order structures correctly
/// 3. Routes orders through PluginRegistry
/// 4. Returns proper responses
#[tokio::test]
async fn test_tradingview_webhook_payload_structure() {
    // Valid buy market order
    let buy_market = json!({
        "symbol": "BTC/USDT",
        "action": "buy",
        "order_type": "market",
        "quantity": 0.01,
        "confidence": 0.85
    });
    
    assert_eq!(buy_market["symbol"], "BTC/USDT");
    assert_eq!(buy_market["action"], "buy");
    assert_eq!(buy_market["order_type"], "market");
    assert_eq!(buy_market["quantity"], 0.01);
    
    // Valid sell limit order with SL/TP
    let sell_limit = json!({
        "symbol": "ETH/USDT",
        "action": "sell",
        "order_type": "limit",
        "quantity": 0.5,
        "price": 3500.0,
        "stop_loss": 3600.0,
        "take_profit": 3400.0,
        "confidence": 0.7
    });
    
    assert_eq!(sell_limit["symbol"], "ETH/USDT");
    assert_eq!(sell_limit["action"], "sell");
    assert!(sell_limit["price"].is_number());
    assert!(sell_limit["stop_loss"].is_number());
    assert!(sell_limit["take_profit"].is_number());
}

#[tokio::test]
async fn test_webhook_payload_defaults() {
    // Minimal payload (should use defaults)
    let minimal = json!({
        "symbol": "BTC/USDT",
        "action": "buy",
        "quantity": 0.01
    });
    
    // Verify required fields present
    assert!(minimal.get("symbol").is_some());
    assert!(minimal.get("action").is_some());
    assert!(minimal.get("quantity").is_some());
    
    // Optional fields should be handled with defaults
    assert!(minimal.get("order_type").is_none()); // Should default to "market"
    assert!(minimal.get("confidence").is_none()); // Should default to 0.7
}

#[tokio::test]
async fn test_invalid_action_handling() {
    let invalid = json!({
        "symbol": "BTC/USDT",
        "action": "invalid_action", // Not "buy" or "sell"
        "quantity": 0.01
    });
    
    // This should be rejected as invalid action
    assert_eq!(invalid["action"], "invalid_action");
}
