//! Mock Plugin for Testing
//!
//! Simulates order execution without real broker/exchange connections

use super::{ExecutionPlugin, ExecutionResult, MarketData, Order};
use async_trait::async_trait;
use chrono::Utc;
use std::error::Error;

/// Mock plugin for testing and development
pub struct MockPlugin {
    name: String,
    is_initialized: bool,
}

impl MockPlugin {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            is_initialized: false,
        }
    }
}

#[async_trait]
impl ExecutionPlugin for MockPlugin {
    async fn init(&mut self, _config: serde_json::Value) -> Result<(), Box<dyn Error + Send + Sync>> {
        tracing::info!(plugin = %self.name, "Initializing mock plugin");
        self.is_initialized = true;
        Ok(())
    }
    
    async fn execute_order(
        &self,
        order: Order,
    ) -> Result<ExecutionResult, Box<dyn Error + Send + Sync>> {
        if !self.is_initialized {
            return Err("Plugin not initialized".into());
        }
        
        tracing::info!(
            plugin = %self.name,
            symbol = %order.symbol,
            side = ?order.side,
            quantity = %order.quantity,
            "Mock executing order"
        );
        
        // Simulate execution with slight slippage
        let base_price = order.price.unwrap_or(67500.0);
        let slippage = base_price * 0.0001; // 0.01% slippage
        let execution_price = match order.side {
            super::OrderSide::Buy => base_price + slippage,
            super::OrderSide::Sell => base_price - slippage,
        };
        
        // Simulate small delay
        tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        
        Ok(ExecutionResult {
            success: true,
            order_id: Some(format!("MOCK-{}", uuid::Uuid::new_v4())),
            filled_quantity: order.quantity,
            average_price: execution_price,
            error: None,
            timestamp: Utc::now().timestamp_millis(),
        })
    }
    
    async fn fetch_data(&self, symbol: &str) -> Result<MarketData, Box<dyn Error + Send + Sync>> {
        if !self.is_initialized {
            return Err("Plugin not initialized".into());
        }
        
        // Generate mock market data
        let base_price = match symbol {
            "BTC/USDT" | "BTCUSDT" => 67500.0,
            "ETH/USDT" | "ETHUSDT" => 3500.0,
            "ES" => 4420.0,
            "EURUSD" => 1.0850,
            _ => 100.0,
        };
        
        let spread = base_price * 0.0001; // 1 basis point spread
        
        Ok(MarketData {
            symbol: symbol.to_string(),
            bid: base_price - spread / 2.0,
            ask: base_price + spread / 2.0,
            last: base_price,
            volume: 1000000.0,
            timestamp: Utc::now().timestamp_millis(),
            extra: serde_json::json!({"source": "mock"}),
        })
    }
    
    fn name(&self) -> &str {
        &self.name
    }
    
    async fn health_check(&self) -> Result<bool, Box<dyn Error + Send + Sync>> {
        Ok(self.is_initialized)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::{OrderSide, OrderType};
    
    #[tokio::test]
    async fn test_mock_plugin_init() {
        let mut plugin = MockPlugin::new("test-mock");
        assert_eq!(plugin.name(), "test-mock");
        
        let result = plugin.init(serde_json::json!({})).await;
        assert!(result.is_ok());
        assert!(plugin.is_initialized);
    }
    
    #[tokio::test]
    async fn test_mock_plugin_execute_order() {
        let mut plugin = MockPlugin::new("test-mock");
        plugin.init(serde_json::json!({})).await.unwrap();
        
        let order = Order {
            symbol: "BTC/USDT".to_string(),
            side: OrderSide::Buy,
            order_type: OrderType::Market,
            quantity: 0.1,
            price: Some(67500.0),
            stop_loss: None,
            take_profit: None,
            confidence: 0.75,
        };
        
        let result = plugin.execute_order(order).await.unwrap();
        
        assert!(result.success);
        assert!(result.order_id.is_some());
        assert_eq!(result.filled_quantity, 0.1);
        assert!(result.average_price > 67500.0); // Buy has positive slippage
    }
    
    #[tokio::test]
    async fn test_mock_plugin_fetch_data() {
        let mut plugin = MockPlugin::new("test-mock");
        plugin.init(serde_json::json!({})).await.unwrap();
        
        let data = plugin.fetch_data("BTC/USDT").await.unwrap();
        
        assert_eq!(data.symbol, "BTC/USDT");
        assert!(data.bid > 0.0);
        assert!(data.ask > data.bid);
        assert!(data.last > 0.0);
    }
    
    #[tokio::test]
    async fn test_mock_plugin_health_check() {
        let mut plugin = MockPlugin::new("test-mock");
        
        // Not initialized
        let health = plugin.health_check().await.unwrap();
        assert!(!health);
        
        // Initialized
        plugin.init(serde_json::json!({})).await.unwrap();
        let health = plugin.health_check().await.unwrap();
        assert!(health);
    }
}
