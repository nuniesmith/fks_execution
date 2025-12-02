//! CCXT Plugin for Crypto/Forex Exchanges
//!
//! Integrates with external CCXT services via HTTP API calls.
//! The CCXT service should be running separately and accessible via HTTP.

use super::{ExecutionPlugin, ExecutionResult, MarketData, Order, OrderSide, OrderType};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::sync::Arc;
use tokio::sync::RwLock;

/// Configuration for CCXT plugin
#[derive(Debug, Clone, Deserialize)]
pub struct CCXTConfig {
    /// Base URL of the CCXT service (e.g., "http://localhost:8000")
    pub base_url: String,
    
    /// Webhook secret for signature verification
    pub webhook_secret: String,
    
    /// Exchange name (e.g., "binance", "coinbase")
    #[serde(default = "default_exchange")]
    pub exchange: String,
    
    /// Whether to use testnet
    #[serde(default)]
    pub testnet: bool,
}

fn default_exchange() -> String {
    "binance".to_string()
}

/// TradingView webhook payload format
#[derive(Debug, Serialize)]
struct WebhookPayload {
    timestamp: i64,
    symbol: String,
    action: String, // "buy" or "sell"
    order_type: String,
    quantity: f64,
    price: Option<f64>,
    stop_loss: Option<f64>,
    take_profit: Option<f64>,
    confidence: f64,
}

/// Response from CCXT webhook
#[derive(Debug, Deserialize)]
struct WebhookResponse {
    status: String,
    message: Option<String>,
    order_id: Option<String>,
    filled_quantity: Option<f64>,
    average_price: Option<f64>,
}

/// CCXT Plugin implementation
pub struct CCXTPlugin {
    name: String,
    config: Arc<RwLock<Option<CCXTConfig>>>,
    client: Client,
}

impl CCXTPlugin {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            config: Arc::new(RwLock::new(None)),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("Failed to create HTTP client"),
        }
    }
    
    /// Generate HMAC-SHA256 signature for webhook
    fn generate_signature(payload: &str, secret: &str) -> String {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        
        type HmacSha256 = Hmac<Sha256>;
        
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(payload.as_bytes());
        let result = mac.finalize();
        hex::encode(result.into_bytes())
    }
}

#[async_trait]
impl ExecutionPlugin for CCXTPlugin {
    async fn init(&mut self, config: serde_json::Value) -> Result<(), Box<dyn Error + Send + Sync>> {
        let ccxt_config: CCXTConfig = serde_json::from_value(config)?;
        
        tracing::info!(
            plugin = %self.name,
            base_url = %ccxt_config.base_url,
            exchange = %ccxt_config.exchange,
            testnet = %ccxt_config.testnet,
            "Initializing CCXT plugin"
        );
        
        // Test connection to CCXT service (non-blocking, log warning if fails)
        let health_url = format!("{}/health", ccxt_config.base_url);
        match self.client.get(&health_url).send().await {
            Ok(response) => {
                if response.status().is_success() {
                    tracing::info!(plugin = %self.name, "CCXT service health check passed");
                } else {
                    tracing::warn!(
                        plugin = %self.name,
                        status = %response.status(),
                        "CCXT service health check returned non-success status"
                    );
                }
            },
            Err(e) => {
                tracing::warn!(
                    plugin = %self.name,
                    error = %e,
                    "CCXT service health check failed, will retry on first request"
                );
            }
        }
        
        *self.config.write().await = Some(ccxt_config);
        
        tracing::info!(plugin = %self.name, "CCXT plugin initialized successfully");
        Ok(())
    }
    
    async fn execute_order(
        &self,
        order: Order,
    ) -> Result<ExecutionResult, Box<dyn Error + Send + Sync>> {
        let config = self.config.read().await;
        let config = config.as_ref()
            .ok_or("Plugin not initialized")?;
        
        // Convert Order to webhook payload
        let action = match order.side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };
        
        let order_type_str = match order.order_type {
            OrderType::Market => "market",
            OrderType::Limit => "limit",
            OrderType::Stop => "stop",
            OrderType::StopLimit => "stop_limit",
            OrderType::TakeProfit => "take_profit",
            OrderType::StopLoss => "stop_loss",
        };
        
        let payload = WebhookPayload {
            timestamp: Utc::now().timestamp(),
            symbol: order.symbol.clone(),
            action: action.to_string(),
            order_type: order_type_str.to_string(),
            quantity: order.quantity,
            price: order.price,
            stop_loss: order.stop_loss,
            take_profit: order.take_profit,
            confidence: order.confidence,
        };
        
        let payload_json = serde_json::to_string(&payload)?;
        let signature = Self::generate_signature(&payload_json, &config.webhook_secret);
        
        tracing::info!(
            plugin = %self.name,
            symbol = %order.symbol,
            side = ?order.side,
            quantity = %order.quantity,
            "Sending order to CCXT service"
        );
        
        // Send webhook to CCXT service
        let webhook_url = format!("{}/webhook/tradingview", config.base_url);
        let response = self.client
            .post(&webhook_url)
            .header("X-Webhook-Signature", signature)
            .header("Content-Type", "application/json")
            .body(payload_json)
            .send()
            .await?;
        
        let status_code = response.status();
        let webhook_response: WebhookResponse = response.json().await?;
        
        // Map webhook response to ExecutionResult
        let success = status_code.is_success() && webhook_response.status != "error";
        
        Ok(ExecutionResult {
            success,
            order_id: webhook_response.order_id,
            filled_quantity: webhook_response.filled_quantity.unwrap_or(0.0),
            average_price: webhook_response.average_price.unwrap_or(0.0),
            error: if !success { webhook_response.message } else { None },
            timestamp: Utc::now().timestamp_millis(),
        })
    }
    
    async fn fetch_data(&self, symbol: &str) -> Result<MarketData, Box<dyn Error + Send + Sync>> {
        let config = self.config.read().await;
        let config = config.as_ref()
            .ok_or("Plugin not initialized")?;
        
        // Fetch ticker data from CCXT service
        let ticker_url = format!("{}/ticker/{}", config.base_url, symbol);
        let response = self.client.get(&ticker_url).send().await?;
        
        if !response.status().is_success() {
            return Err(format!("Failed to fetch ticker for {}: {}", symbol, response.status()).into());
        }
        
        #[derive(Deserialize)]
        struct TickerResponse {
            symbol: String,
            bid: Option<f64>,
            ask: Option<f64>,
            last: f64,
            volume: Option<f64>,
            timestamp: Option<i64>,
        }
        
        let ticker: TickerResponse = response.json().await?;
        
        Ok(MarketData {
            symbol: ticker.symbol,
            bid: ticker.bid.unwrap_or(ticker.last * 0.9999),
            ask: ticker.ask.unwrap_or(ticker.last * 1.0001),
            last: ticker.last,
            volume: ticker.volume.unwrap_or(0.0),
            timestamp: ticker.timestamp.unwrap_or_else(|| Utc::now().timestamp_millis()),
            extra: serde_json::json!({
                "exchange": config.exchange,
                "testnet": config.testnet
            }),
        })
    }
    
    fn name(&self) -> &str {
        &self.name
    }
    
    async fn health_check(&self) -> Result<bool, Box<dyn Error + Send + Sync>> {
        let config = self.config.read().await;
        let config = match config.as_ref() {
            Some(c) => c,
            None => return Ok(false), // Not initialized
        };
        
        // Check CCXT service health
        let health_url = format!("{}/health", config.base_url);
        match self.client.get(&health_url).send().await {
            Ok(response) => Ok(response.status().is_success()),
            Err(_) => Ok(false),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plugins::OrderSide;
    
    #[test]
    fn test_signature_generation() {
        let payload = r#"{"timestamp":1699113600,"symbol":"BTC/USDT","action":"buy"}"#;
        let secret = "test-secret";
        
        let sig1 = CCXTPlugin::generate_signature(payload, secret);
        let sig2 = CCXTPlugin::generate_signature(payload, secret);
        
        // Same payload + secret should produce same signature
        assert_eq!(sig1, sig2);
        assert!(!sig1.is_empty());
    }
    
    #[tokio::test]
    async fn test_ccxt_plugin_not_initialized() {
        let plugin = CCXTPlugin::new("test-ccxt");
        
        let order = Order {
            symbol: "BTC/USDT".to_string(),
            side: OrderSide::Buy,
            order_type: OrderType::Market,
            quantity: 0.1,
            price: None,
            stop_loss: None,
            take_profit: None,
            confidence: 0.75,
        };
        
        // Should fail - not initialized
        let result = plugin.execute_order(order).await;
        assert!(result.is_err());
    }
    
    #[test]
    fn test_webhook_payload_serialization() {
        let payload = WebhookPayload {
            timestamp: 1699113600,
            symbol: "BTC/USDT".to_string(),
            action: "buy".to_string(),
            order_type: "market".to_string(),
            quantity: 0.1,
            price: Some(67500.0),
            stop_loss: Some(67000.0),
            take_profit: Some(69000.0),
            confidence: 0.75,
        };
        
        let json = serde_json::to_string(&payload).unwrap();
        assert!(json.contains("BTC/USDT"));
        assert!(json.contains("buy"));
        assert!(json.contains("0.75"));
    }
}
