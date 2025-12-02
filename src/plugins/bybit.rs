//! Bybit Plugin for Futures Trading
//!
//! Direct integration with Bybit API for futures trading (linear contracts).
//! Supports order placement, leverage management, and position queries.

use super::{ExecutionPlugin, ExecutionResult, MarketData, Order, OrderSide, OrderType};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use std::error::Error;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

/// Configuration for Bybit plugin
#[derive(Debug, Clone, Deserialize)]
pub struct BybitConfig {
    /// Bybit API key
    pub api_key: String,
    
    /// Bybit API secret
    pub api_secret: String,
    
    /// Whether to use testnet (default: false)
    #[serde(default)]
    pub testnet: bool,
    
    /// Category: "linear" for futures (default)
    #[serde(default = "default_category")]
    pub category: String,
    
    /// Default leverage (default: 10)
    #[serde(default = "default_leverage")]
    pub leverage: i32,
}

fn default_category() -> String {
    "linear".to_string()
}

fn default_leverage() -> i32 {
    10
}

/// Bybit API response structure
#[derive(Debug, Deserialize)]
struct BybitResponse<T> {
    ret_code: i32,
    ret_msg: String,
    result: Option<T>,
    #[serde(rename = "retCode")]
    ret_code_alt: Option<i32>,
    #[serde(rename = "retMsg")]
    ret_msg_alt: Option<String>,
}

impl<T> BybitResponse<T> {
    fn ret_code(&self) -> i32 {
        self.ret_code_alt.unwrap_or(self.ret_code)
    }
    
    fn ret_msg(&self) -> &str {
        self.ret_msg_alt.as_deref().unwrap_or(&self.ret_msg)
    }
    
    fn is_success(&self) -> bool {
        self.ret_code() == 0
    }
}

/// Bybit order result
#[derive(Debug, Clone, Deserialize)]
struct BybitOrderResult {
    order_id: Option<String>,
    order_link_id: Option<String>,
}

/// Bybit position result
#[derive(Debug, Deserialize)]
struct BybitPositionResult {
    list: Option<Vec<BybitPosition>>,
}

/// Bybit position
#[derive(Debug, Clone, Deserialize)]
struct BybitPosition {
    symbol: String,
    side: String,
    size: String,
    entry_price: String,
    mark_price: String,
    #[serde(rename = "unrealisedPnl")]
    unrealized_pnl: Option<String>,
    leverage: String,
    position_value: Option<String>,
}

/// Bybit Plugin implementation
pub struct BybitPlugin {
    name: String,
    config: Arc<RwLock<Option<BybitConfig>>>,
    client: Client,
    base_url: String,
}

impl BybitPlugin {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            config: Arc::new(RwLock::new(None)),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("Failed to create HTTP client"),
            base_url: "https://api.bybit.com".to_string(),
        }
    }
    
    /// Get base URL (testnet or mainnet)
    fn get_base_url(&self, testnet: bool) -> &str {
        if testnet {
            "https://api-testnet.bybit.com"
        } else {
            "https://api.bybit.com"
        }
    }
    
    /// Generate HMAC-SHA256 signature for Bybit API
    fn generate_signature(secret: &str, message: &str) -> String {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        
        type HmacSha256 = Hmac<Sha256>;
        
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(message.as_bytes());
        let result = mac.finalize();
        hex::encode(result.into_bytes())
    }
    
    /// Create authenticated request headers for POST requests (JSON body)
    async fn create_headers_post(
        &self,
        api_key: &str,
        api_secret: &str,
        recv_window: u64,
        json_body: &str,
    ) -> Result<reqwest::header::HeaderMap, Box<dyn Error + Send + Sync>> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        // For POST: timestamp + api_key + recv_window + json_body
        let message = format!("{}{}{}{}", timestamp, api_key, recv_window, json_body);
        let signature = Self::generate_signature(api_secret, &message);
        
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("X-BAPI-API-KEY", api_key.parse()?);
        headers.insert("X-BAPI-TIMESTAMP", timestamp.to_string().parse()?);
        headers.insert("X-BAPI-SIGN", signature.parse()?);
        headers.insert("X-BAPI-RECV-WINDOW", recv_window.to_string().parse()?);
        headers.insert("X-BAPI-SIGN-TYPE", "2".parse()?); // HMAC-SHA256
        headers.insert("Content-Type", "application/json".parse()?);
        
        Ok(headers)
    }
    
    /// Create authenticated request headers for GET requests (query string)
    async fn create_headers_get(
        &self,
        api_key: &str,
        api_secret: &str,
        recv_window: u64,
        query_string: &str,
    ) -> Result<reqwest::header::HeaderMap, Box<dyn Error + Send + Sync>> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as u64;
        
        // For GET: timestamp + api_key + recv_window + query_string
        let message = format!("{}{}{}{}", timestamp, api_key, recv_window, query_string);
        let signature = Self::generate_signature(api_secret, &message);
        
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("X-BAPI-API-KEY", api_key.parse()?);
        headers.insert("X-BAPI-TIMESTAMP", timestamp.to_string().parse()?);
        headers.insert("X-BAPI-SIGN", signature.parse()?);
        headers.insert("X-BAPI-RECV-WINDOW", recv_window.to_string().parse()?);
        headers.insert("X-BAPI-SIGN-TYPE", "2".parse()?); // HMAC-SHA256
        
        Ok(headers)
    }
    
    /// Set leverage for a symbol (Bybit-specific)
    pub async fn set_leverage(
        &self,
        symbol: &str,
        leverage: i32,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let config = self.config.read().await;
        let config = config.as_ref()
            .ok_or("Plugin not initialized")?;
        
        let base_url = self.get_base_url(config.testnet);
        let endpoint = format!("{}/v5/position/set-leverage", base_url);
        
        let params = serde_json::json!({
            "category": config.category,
            "symbol": symbol,
            "buyLeverage": leverage.to_string(),
            "sellLeverage": leverage.to_string(),
        });
        
        // For POST requests, signature is calculated from JSON body
        let json_body = serde_json::to_string(&params)?;
        let headers = self.create_headers_post(
            &config.api_key,
            &config.api_secret,
            5000,
            &json_body,
        ).await?;
        
        let response = self.client
            .post(&endpoint)
            .headers(headers)
            .json(&params)
            .send()
            .await?;
        
        let status = response.status();
        let text = response.text().await?;
        
        if !status.is_success() {
            return Err(format!("Bybit API error ({}): {}", status, text).into());
        }
        
        let bybit_resp: BybitResponse<serde_json::Value> = serde_json::from_str(&text)?;
        
        if !bybit_resp.is_success() {
            return Err(format!("Bybit API error: {} - {}", bybit_resp.ret_code(), bybit_resp.ret_msg()).into());
        }
        
        tracing::info!(plugin = %self.name, symbol = %symbol, leverage = %leverage, "Leverage set successfully");
        Ok(())
    }
    
    /// Get positions for a symbol
    pub async fn get_position(
        &self,
        symbol: &str,
    ) -> Result<Option<BybitPosition>, Box<dyn Error + Send + Sync>> {
        let config = self.config.read().await;
        let config = config.as_ref()
            .ok_or("Plugin not initialized")?;
        
        let base_url = self.get_base_url(config.testnet);
        let endpoint = format!("{}/v5/position/list", base_url);
        
        let params = serde_json::json!({
            "category": config.category,
            "symbol": symbol,
        });
        
        let query_string = serde_qs::to_string(&params)?;
        let headers = self.create_headers_get(
            &config.api_key,
            &config.api_secret,
            5000,
            &query_string,
        ).await?;
        
        let response = self.client
            .get(&endpoint)
            .headers(headers)
            .query(&params)
            .send()
            .await?;
        
        let status = response.status();
        let text = response.text().await?;
        
        if !status.is_success() {
            return Err(format!("Bybit API error ({}): {}", status, text).into());
        }
        
        let bybit_resp: BybitResponse<BybitPositionResult> = serde_json::from_str(&text)?;
        
        if !bybit_resp.is_success() {
            return Err(format!("Bybit API error: {} - {}", bybit_resp.ret_code(), bybit_resp.ret_msg()).into());
        }
        
        if let Some(result) = bybit_resp.result {
            if let Some(list) = result.list {
                if let Some(position) = list.first() {
                    return Ok(Some(position.clone()));
                }
            }
        }
        
        Ok(None)
    }
}

#[async_trait]
impl ExecutionPlugin for BybitPlugin {
    async fn init(&mut self, config: serde_json::Value) -> Result<(), Box<dyn Error + Send + Sync>> {
        let bybit_config: BybitConfig = serde_json::from_value(config)?;
        
        tracing::info!(
            plugin = %self.name,
            testnet = %bybit_config.testnet,
            category = %bybit_config.category,
            "Initializing Bybit plugin"
        );
        
        // Validate API keys are not empty
        if bybit_config.api_key.is_empty() || bybit_config.api_secret.is_empty() {
            return Err("Bybit API key and secret must be provided".into());
        }
        
        // Update base URL
        self.base_url = self.get_base_url(bybit_config.testnet).to_string();
        
        // Test connection with a simple API call (non-blocking, log warning if fails)
        // We'll do this on first order execution
        
        *self.config.write().await = Some(bybit_config);
        
        tracing::info!(plugin = %self.name, "Bybit plugin initialized successfully");
        Ok(())
    }
    
    async fn execute_order(
        &self,
        order: Order,
    ) -> Result<ExecutionResult, Box<dyn Error + Send + Sync>> {
        let config = self.config.read().await;
        let config = config.as_ref()
            .ok_or("Plugin not initialized")?;
        
        let base_url = self.get_base_url(config.testnet);
        let endpoint = format!("{}/v5/order/create", base_url);
        
        // Convert Order to Bybit format
        let side = match order.side {
            OrderSide::Buy => "Buy",
            OrderSide::Sell => "Sell",
        };
        
        let order_type = match order.order_type {
            OrderType::Market => "Market",
            OrderType::Limit => "Limit",
            OrderType::Stop => "Stop",
            OrderType::StopLimit => "StopLimit",
            OrderType::TakeProfit => "TakeProfit",
            OrderType::StopLoss => "StopLoss",
        };
        
        // Build order parameters
        let mut params = serde_json::json!({
            "category": config.category,
            "symbol": order.symbol,
            "side": side,
            "orderType": order_type,
            "qty": format!("{}", order.quantity),
            "positionIdx": 0, // One-way mode
        });
        
        // Add price for limit orders
        if let Some(price) = order.price {
            params["price"] = serde_json::json!(format!("{}", price));
        }
        
        // Add stop-loss and take-profit if provided
        if let Some(stop_loss) = order.stop_loss {
            params["stopLoss"] = serde_json::json!(format!("{}", stop_loss));
        }
        
        if let Some(take_profit) = order.take_profit {
            params["takeProfit"] = serde_json::json!(format!("{}", take_profit));
        }
        
        // Set leverage from config (if not already set)
        if let Some(_lev) = params.get("leverage") {
            // Leverage already in params
        } else {
            params["leverage"] = serde_json::json!(format!("{}", config.leverage));
        }
        
        // For POST requests, signature is calculated from JSON body
        let json_body = serde_json::to_string(&params)?;
        let headers = self.create_headers_post(
            &config.api_key,
            &config.api_secret,
            5000,
            &json_body,
        ).await?;
        
        let response = self.client
            .post(&endpoint)
            .headers(headers)
            .json(&params)
            .send()
            .await?;
        
        let status = response.status();
        let text = response.text().await?;
        
        if !status.is_success() {
            return Ok(ExecutionResult {
                success: false,
                order_id: None,
                filled_quantity: 0.0,
                average_price: 0.0,
                error: Some(format!("HTTP {}: {}", status, text)),
                timestamp: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64,
            });
        }
        
        let bybit_resp: BybitResponse<BybitOrderResult> = serde_json::from_str(&text)?;
        
        if !bybit_resp.is_success() {
            return Ok(ExecutionResult {
                success: false,
                order_id: None,
                filled_quantity: 0.0,
                average_price: 0.0,
                error: Some(format!("Bybit API error: {} - {}", bybit_resp.ret_code(), bybit_resp.ret_msg())),
                timestamp: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64,
            });
        }
        
        // Extract order ID from result
        let order_id = bybit_resp.result.as_ref()
            .and_then(|r| r.order_id.clone())
            .or_else(|| bybit_resp.result.as_ref().and_then(|r| r.order_link_id.clone()));
        
        tracing::info!(
            plugin = %self.name,
            symbol = %order.symbol,
            side = %side,
            order_id = ?order_id,
            "Order placed successfully"
        );
        
        // For market orders, we assume immediate fill
        // For limit orders, filled_quantity will be 0 until filled
        let filled_quantity = match order.order_type {
            OrderType::Market => order.quantity,
            _ => 0.0,
        };
        
        // Get current market price as average_price (for market orders)
        let average_price = match order.order_type {
            OrderType::Market => {
                // Try to fetch current price
                if let Ok(market_data) = self.fetch_data(&order.symbol).await {
                    market_data.last
                } else {
                    order.price.unwrap_or(0.0)
                }
            },
            _ => order.price.unwrap_or(0.0),
        };
        
        Ok(ExecutionResult {
            success: true,
            order_id,
            filled_quantity,
            average_price,
            error: None,
            timestamp: SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_millis() as i64,
        })
    }
    
    async fn fetch_data(&self, symbol: &str) -> Result<MarketData, Box<dyn Error + Send + Sync>> {
        let config = self.config.read().await;
        let config = config.as_ref()
            .ok_or("Plugin not initialized")?;
        
        let base_url = self.get_base_url(config.testnet);
        let endpoint = format!("{}/v5/market/tickers", base_url);
        
        let params = serde_json::json!({
            "category": config.category,
            "symbol": symbol,
        });
        
        // Public endpoint, no authentication required
        let response = self.client
            .get(&endpoint)
            .query(&params)
            .send()
            .await?;
        
        let status = response.status();
        let text = response.text().await?;
        
        if !status.is_success() {
            return Err(format!("Bybit API error ({}): {}", status, text).into());
        }
        
        #[derive(Deserialize)]
        struct TickerResult {
            list: Option<Vec<Ticker>>,
        }
        
        #[derive(Deserialize)]
        struct Ticker {
            last_price: String,
            bid1_price: String,
            ask1_price: String,
            volume24h: Option<String>,
        }
        
        let bybit_resp: BybitResponse<TickerResult> = serde_json::from_str(&text)?;
        
        if !bybit_resp.is_success() {
            return Err(format!("Bybit API error: {} - {}", bybit_resp.ret_code(), bybit_resp.ret_msg()).into());
        }
        
        if let Some(result) = bybit_resp.result {
            if let Some(list) = result.list {
                if let Some(ticker) = list.first() {
                    let last = ticker.last_price.parse::<f64>()?;
                    let bid = ticker.bid1_price.parse::<f64>()?;
                    let ask = ticker.ask1_price.parse::<f64>()?;
                    let volume = ticker.volume24h
                        .as_ref()
                        .and_then(|v| v.parse::<f64>().ok())
                        .unwrap_or(0.0);
                    
                    return Ok(MarketData {
                        symbol: symbol.to_string(),
                        bid,
                        ask,
                        last,
                        volume,
                        timestamp: SystemTime::now()
                            .duration_since(UNIX_EPOCH)
                            .unwrap()
                            .as_millis() as i64,
                        extra: serde_json::json!({}),
                    });
                }
            }
        }
        
        Err(format!("No market data found for symbol: {}", symbol).into())
    }
    
    fn name(&self) -> &str {
        &self.name
    }
    
    async fn health_check(&self) -> Result<bool, Box<dyn Error + Send + Sync>> {
        // Check if plugin is initialized
        let config = self.config.read().await;
        if config.is_none() {
            return Ok(false);
        }
        
        // Try to fetch market data as a health check (public endpoint)
        // This verifies connectivity to Bybit API
        match self.fetch_data("BTCUSDT").await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false), // Return false but don't error
        }
    }
}
