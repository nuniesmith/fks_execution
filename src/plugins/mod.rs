//! ExecutionPlugin Trait and Core Types
//!
//! Defines the plugin interface for modular execution backends (NinjaTrader, MT5, CCXT, etc.)

pub mod bybit;
pub mod ccxt;
pub mod kucoin;
pub mod mock;
pub mod registry;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use std::error::Error;

/// Order side (buy or sell)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OrderSide {
    Buy,
    Sell,
}

/// Order type
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum OrderType {
    Market,
    Limit,
    Stop,
    StopLimit,
    TakeProfit,
    StopLoss,
}

/// Order structure for execution
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Order {
    /// Trading symbol (e.g., "BTC/USDT", "ES", "EURUSD")
    pub symbol: String,
    
    /// Order side
    pub side: OrderSide,
    
    /// Order type
    pub order_type: OrderType,
    
    /// Order quantity/amount
    pub quantity: f64,
    
    /// Limit price (required for Limit orders)
    pub price: Option<f64>,
    
    /// Stop-loss price
    pub stop_loss: Option<f64>,
    
    /// Take-profit price
    pub take_profit: Option<f64>,
    
    /// Confidence score (0-1) from agent system
    #[serde(default = "default_confidence")]
    pub confidence: f64,
}

fn default_confidence() -> f64 {
    0.6
}

/// Execution result from a plugin
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionResult {
    /// Whether the order was successfully executed
    pub success: bool,
    
    /// Broker/exchange order ID
    pub order_id: Option<String>,
    
    /// Filled quantity
    pub filled_quantity: f64,
    
    /// Average execution price
    pub average_price: f64,
    
    /// Error message if execution failed
    pub error: Option<String>,
    
    /// Execution timestamp (Unix millis)
    pub timestamp: i64,
}

/// Market data snapshot
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MarketData {
    /// Trading symbol
    pub symbol: String,
    
    /// Bid price
    pub bid: f64,
    
    /// Ask price
    pub ask: f64,
    
    /// Last traded price
    pub last: f64,
    
    /// 24h volume
    pub volume: f64,
    
    /// Timestamp (Unix millis)
    pub timestamp: i64,
    
    /// Optional additional fields (exchange-specific)
    #[serde(default)]
    pub extra: serde_json::Value,
}

/// ExecutionPlugin trait - implemented by all execution backends
#[async_trait]
pub trait ExecutionPlugin: Send + Sync {
    /// Initialize the plugin with configuration
    ///
    /// # Arguments
    /// * `config` - Plugin-specific configuration as JSON
    ///
    /// # Example Config
    /// ```json
    /// {
    ///   "exchange": "binance",
    ///   "api_key": "...",
    ///   "api_secret": "...",
    ///   "testnet": false
    /// }
    /// ```
    async fn init(&mut self, config: serde_json::Value) -> Result<(), Box<dyn Error + Send + Sync>>;
    
    /// Execute an order
    ///
    /// # Arguments
    /// * `order` - Order to execute
    ///
    /// # Returns
    /// * `ExecutionResult` - Result of the execution
    async fn execute_order(
        &self,
        order: Order,
    ) -> Result<ExecutionResult, Box<dyn Error + Send + Sync>>;
    
    /// Fetch current market data for a symbol
    ///
    /// # Arguments
    /// * `symbol` - Trading symbol
    ///
    /// # Returns
    /// * `MarketData` - Current market snapshot
    async fn fetch_data(&self, symbol: &str) -> Result<MarketData, Box<dyn Error + Send + Sync>>;
    
    /// Get plugin name/identifier
    fn name(&self) -> &str;
    
    /// Health check - verify plugin is operational
    ///
    /// # Returns
    /// * `true` if plugin is healthy, `false` otherwise
    async fn health_check(&self) -> Result<bool, Box<dyn Error + Send + Sync>>;
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_order_serialization() {
        let order = Order {
            symbol: "BTC/USDT".to_string(),
            side: OrderSide::Buy,
            order_type: OrderType::Limit,
            quantity: 0.1,
            price: Some(67500.0),
            stop_loss: Some(67000.0),
            take_profit: Some(69000.0),
            confidence: 0.75,
        };
        
        let json = serde_json::to_string(&order).unwrap();
        let deserialized: Order = serde_json::from_str(&json).unwrap();
        
        assert_eq!(deserialized.symbol, "BTC/USDT");
        assert_eq!(deserialized.side, OrderSide::Buy);
        assert_eq!(deserialized.confidence, 0.75);
    }
    
    #[test]
    fn test_default_confidence() {
        let json = r#"{
            "symbol": "ES",
            "side": "buy",
            "order_type": "market",
            "quantity": 1.0
        }"#;
        
        let order: Order = serde_json::from_str(json).unwrap();
        assert_eq!(order.confidence, 0.6); // Default
    }
    
    #[test]
    fn test_execution_result() {
        let result = ExecutionResult {
            success: true,
            order_id: Some("12345".to_string()),
            filled_quantity: 0.1,
            average_price: 67520.0,
            error: None,
            timestamp: 1699113600000,
        };
        
        assert!(result.success);
        assert!(result.error.is_none());
    }
}
