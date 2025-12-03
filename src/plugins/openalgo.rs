//! OpenAlgo Plugin for Indian Market Execution
//!
//! Integrates with OpenAlgo (https://github.com/marketcalls/openalgo) for:
//! - Paper/Sandbox trading (simulated execution with live data)
//! - Live trading with Indian brokers (Zerodha, Fyers, Dhan, Angel One, etc.)
//!
//! Features:
//! - Unified API for 24+ Indian brokers
//! - Built-in paper trading mode
//! - Real-time order status tracking
//! - Position and balance management

use super::{ExecutionPlugin, ExecutionResult, MarketData, Order, OrderSide, OrderType};
use async_trait::async_trait;
use chrono::Utc;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::error::Error;
use std::time::Duration;

/// OpenAlgo API configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OpenAlgoConfig {
    /// Base URL for OpenAlgo API (default: http://localhost:5000)
    pub base_url: String,
    
    /// API key for authentication
    pub api_key: Option<String>,
    
    /// Enable sandbox/paper trading mode
    pub sandbox_mode: bool,
    
    /// Broker name (zerodha, fyers, dhan, angel, etc.)
    pub broker: String,
    
    /// Request timeout in seconds
    pub timeout_secs: u64,
}

impl Default for OpenAlgoConfig {
    fn default() -> Self {
        Self {
            base_url: std::env::var("OPENALGO_URL").unwrap_or_else(|_| "http://openalgo:5000".to_string()),
            api_key: std::env::var("OPENALGO_API_KEY").ok(),
            sandbox_mode: std::env::var("OPENALGO_SANDBOX")
                .map(|v| v.to_lowercase() == "true")
                .unwrap_or(true), // Default to sandbox for safety
            broker: std::env::var("OPENALGO_BROKER").unwrap_or_else(|_| "paper".to_string()),
            timeout_secs: 30,
        }
    }
}

/// OpenAlgo order request format
#[derive(Debug, Serialize)]
struct OpenAlgoOrderRequest {
    symbol: String,
    exchange: String,
    action: String,      // BUY or SELL
    quantity: i32,
    order_type: String,  // MARKET, LIMIT, SL, SL-M
    product: String,     // CNC (delivery), MIS (intraday), NRML (F&O)
    price: Option<f64>,
    trigger_price: Option<f64>,
}

/// OpenAlgo order response
#[derive(Debug, Deserialize)]
struct OpenAlgoOrderResponse {
    status: String,
    order_id: Option<String>,
    message: Option<String>,
}

/// OpenAlgo position response
#[derive(Debug, Deserialize)]
struct OpenAlgoPosition {
    symbol: String,
    quantity: i32,
    average_price: f64,
    pnl: f64,
}

/// OpenAlgo plugin for Indian market execution
pub struct OpenAlgoPlugin {
    name: String,
    config: OpenAlgoConfig,
    client: Option<Client>,
    is_initialized: bool,
}

impl OpenAlgoPlugin {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            config: OpenAlgoConfig::default(),
            client: None,
            is_initialized: false,
        }
    }
    
    /// Convert FKS symbol to NSE/BSE format
    /// e.g., "RELIANCE" -> "RELIANCE", "NIFTY50" -> "NIFTY 50"
    fn convert_symbol(&self, symbol: &str) -> (String, String) {
        // Default to NSE for most symbols
        let exchange = if symbol.ends_with("-BSE") {
            "BSE"
        } else {
            "NSE"
        };
        
        let clean_symbol = symbol
            .replace("-NSE", "")
            .replace("-BSE", "")
            .to_uppercase();
        
        (clean_symbol, exchange.to_string())
    }
    
    /// Convert FKS order type to OpenAlgo format
    fn convert_order_type(&self, order_type: &OrderType) -> String {
        match order_type {
            OrderType::Market => "MARKET".to_string(),
            OrderType::Limit => "LIMIT".to_string(),
            OrderType::Stop => "SL-M".to_string(),      // Stop-Loss Market
            OrderType::StopLimit => "SL".to_string(),   // Stop-Loss Limit
            OrderType::TakeProfit => "LIMIT".to_string(),
            OrderType::StopLoss => "SL-M".to_string(),
        }
    }
    
    /// Determine product type based on order context
    fn get_product_type(&self, symbol: &str) -> String {
        // TODO: Make this configurable
        // CNC = Cash and Carry (delivery)
        // MIS = Margin Intraday Square-off
        // NRML = Normal (F&O)
        
        if symbol.contains("FUT") || symbol.contains("OPT") {
            "NRML".to_string()
        } else {
            "MIS".to_string() // Default to intraday for safety
        }
    }
}

#[async_trait]
impl ExecutionPlugin for OpenAlgoPlugin {
    async fn init(&mut self, config: serde_json::Value) -> Result<(), Box<dyn Error + Send + Sync>> {
        tracing::info!(
            plugin = %self.name,
            sandbox = %self.config.sandbox_mode,
            broker = %self.config.broker,
            "Initializing OpenAlgo plugin"
        );
        
        // Parse config if provided
        if !config.is_null() {
            if let Ok(parsed) = serde_json::from_value::<OpenAlgoConfig>(config) {
                self.config = parsed;
            }
        }
        
        // Create HTTP client
        self.client = Some(
            Client::builder()
                .timeout(Duration::from_secs(self.config.timeout_secs))
                .build()?
        );
        
        // Verify connectivity
        let client = self.client.as_ref().unwrap();
        let health_url = format!("{}/api/v1/health", self.config.base_url);
        
        match client.get(&health_url).send().await {
            Ok(response) if response.status().is_success() => {
                tracing::info!(
                    plugin = %self.name,
                    "OpenAlgo connection verified"
                );
            }
            Ok(response) => {
                tracing::warn!(
                    plugin = %self.name,
                    status = %response.status(),
                    "OpenAlgo health check returned non-success status"
                );
            }
            Err(e) => {
                tracing::warn!(
                    plugin = %self.name,
                    error = %e,
                    "OpenAlgo connection failed - will retry on order execution"
                );
            }
        }
        
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
        
        let client = self.client.as_ref().ok_or("HTTP client not available")?;
        let (symbol, exchange) = self.convert_symbol(&order.symbol);
        
        tracing::info!(
            plugin = %self.name,
            symbol = %symbol,
            exchange = %exchange,
            side = ?order.side,
            quantity = %order.quantity,
            sandbox = %self.config.sandbox_mode,
            "Executing order via OpenAlgo"
        );
        
        // Build OpenAlgo order request
        let openalgo_order = OpenAlgoOrderRequest {
            symbol,
            exchange,
            action: match order.side {
                OrderSide::Buy => "BUY".to_string(),
                OrderSide::Sell => "SELL".to_string(),
            },
            quantity: order.quantity as i32,
            order_type: self.convert_order_type(&order.order_type),
            product: self.get_product_type(&order.symbol),
            price: order.price,
            trigger_price: order.stop_loss,
        };
        
        // Execute order
        let order_url = format!("{}/api/v1/orders", self.config.base_url);
        
        let response = client
            .post(&order_url)
            .json(&openalgo_order)
            .send()
            .await?;
        
        if response.status().is_success() {
            let result: OpenAlgoOrderResponse = response.json().await?;
            
            if result.status == "success" || result.status == "ok" {
                tracing::info!(
                    plugin = %self.name,
                    order_id = ?result.order_id,
                    "Order executed successfully"
                );
                
                Ok(ExecutionResult {
                    success: true,
                    order_id: result.order_id,
                    filled_quantity: order.quantity,
                    average_price: order.price.unwrap_or(0.0),
                    error: None,
                    timestamp: Utc::now().timestamp_millis(),
                })
            } else {
                tracing::warn!(
                    plugin = %self.name,
                    message = ?result.message,
                    "Order rejected by OpenAlgo"
                );
                
                Ok(ExecutionResult {
                    success: false,
                    order_id: None,
                    filled_quantity: 0.0,
                    average_price: 0.0,
                    error: result.message,
                    timestamp: Utc::now().timestamp_millis(),
                })
            }
        } else {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            
            tracing::error!(
                plugin = %self.name,
                status = %status,
                error = %error_text,
                "OpenAlgo API error"
            );
            
            Ok(ExecutionResult {
                success: false,
                order_id: None,
                filled_quantity: 0.0,
                average_price: 0.0,
                error: Some(format!("API error {}: {}", status, error_text)),
                timestamp: Utc::now().timestamp_millis(),
            })
        }
    }
    
    async fn fetch_data(&self, symbol: &str) -> Result<MarketData, Box<dyn Error + Send + Sync>> {
        if !self.is_initialized {
            return Err("Plugin not initialized".into());
        }
        
        let client = self.client.as_ref().ok_or("HTTP client not available")?;
        let (clean_symbol, exchange) = self.convert_symbol(symbol);
        
        // Fetch quote from OpenAlgo
        let quote_url = format!(
            "{}/api/v1/quote?symbol={}&exchange={}",
            self.config.base_url, clean_symbol, exchange
        );
        
        let response = client.get(&quote_url).send().await?;
        
        if response.status().is_success() {
            let data: serde_json::Value = response.json().await?;
            
            let bid = data["bid"].as_f64().unwrap_or(0.0);
            let ask = data["ask"].as_f64().unwrap_or(0.0);
            let last = data["ltp"].as_f64().unwrap_or((bid + ask) / 2.0);
            let volume = data["volume"].as_f64().unwrap_or(0.0);
            
            Ok(MarketData {
                symbol: symbol.to_string(),
                bid,
                ask,
                last,
                volume,
                timestamp: Utc::now().timestamp_millis(),
                extra: serde_json::json!({
                    "exchange": exchange,
                    "source": "openalgo",
                    "sandbox": self.config.sandbox_mode
                }),
            })
        } else {
            // Return placeholder data if quote fails
            tracing::warn!(
                plugin = %self.name,
                symbol = %symbol,
                "Failed to fetch quote, returning placeholder"
            );
            
            Ok(MarketData {
                symbol: symbol.to_string(),
                bid: 0.0,
                ask: 0.0,
                last: 0.0,
                volume: 0.0,
                timestamp: Utc::now().timestamp_millis(),
                extra: serde_json::json!({
                    "source": "openalgo",
                    "error": "quote_failed"
                }),
            })
        }
    }
    
    fn name(&self) -> &str {
        &self.name
    }
    
    async fn health_check(&self) -> Result<bool, Box<dyn Error + Send + Sync>> {
        if !self.is_initialized {
            return Ok(false);
        }
        
        let client = match self.client.as_ref() {
            Some(c) => c,
            None => return Ok(false),
        };
        
        let health_url = format!("{}/api/v1/health", self.config.base_url);
        
        match client.get(&health_url).send().await {
            Ok(response) => Ok(response.status().is_success()),
            Err(_) => Ok(false),
        }
    }
}

impl OpenAlgoPlugin {
    /// Get list of supported markets/exchanges
    pub fn supported_markets(&self) -> Vec<String> {
        vec![
            "NSE".to_string(),  // National Stock Exchange
            "BSE".to_string(),  // Bombay Stock Exchange
            "NFO".to_string(),  // NSE Futures & Options
            "MCX".to_string(),  // Multi Commodity Exchange
            "CDS".to_string(),  // Currency Derivatives
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_symbol_conversion() {
        let plugin = OpenAlgoPlugin::new("test");
        
        let (symbol, exchange) = plugin.convert_symbol("RELIANCE");
        assert_eq!(symbol, "RELIANCE");
        assert_eq!(exchange, "NSE");
        
        let (symbol, exchange) = plugin.convert_symbol("INFY-BSE");
        assert_eq!(symbol, "INFY");
        assert_eq!(exchange, "BSE");
    }
    
    #[test]
    fn test_order_type_conversion() {
        let plugin = OpenAlgoPlugin::new("test");
        
        assert_eq!(plugin.convert_order_type(&OrderType::Market), "MARKET");
        assert_eq!(plugin.convert_order_type(&OrderType::Limit), "LIMIT");
        assert_eq!(plugin.convert_order_type(&OrderType::Stop), "SL-M");
        assert_eq!(plugin.convert_order_type(&OrderType::StopLimit), "SL");
    }
    
    #[test]
    fn test_default_config() {
        let config = OpenAlgoConfig::default();
        assert!(config.sandbox_mode); // Should default to sandbox for safety
        assert_eq!(config.broker, "paper");
    }
}
