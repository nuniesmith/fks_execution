//! KuCoin Plugin for Spot and Futures Trading
//!
//! Direct integration with KuCoin API for spot and futures trading.
//! Supports order placement, leverage management, and position queries.
//! Canada-compliant exchange for live trading.

use super::{ExecutionPlugin, ExecutionResult, MarketData, Order, OrderSide, OrderType};
use async_trait::async_trait;
use reqwest::Client;
use serde::Deserialize;
use std::error::Error;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use tokio::sync::RwLock;

/// Configuration for KuCoin plugin
#[derive(Debug, Clone, Deserialize)]
pub struct KuCoinConfig {
    /// KuCoin API key
    pub api_key: String,
    
    /// KuCoin API secret
    pub api_secret: String,
    
    /// KuCoin API passphrase (required for KuCoin)
    pub api_passphrase: String,
    
    /// Whether to use sandbox/testnet (default: false)
    #[serde(default)]
    pub testnet: bool,
    
    /// Trading type: "spot" or "futures" (default: "futures")
    #[serde(default = "default_trading_type")]
    pub trading_type: String,
    
    /// Default leverage (default: 10, for futures)
    #[serde(default = "default_leverage")]
    pub leverage: i32,
}

fn default_trading_type() -> String {
    "futures".to_string()
}

fn default_leverage() -> i32 {
    10
}

/// KuCoin API response structure
#[derive(Debug, Deserialize)]
struct KuCoinResponse<T> {
    code: Option<String>,
    data: Option<T>,
    msg: Option<String>,
}

impl<T> KuCoinResponse<T> {
    fn is_success(&self) -> bool {
        self.code.as_deref() == Some("200000") || self.code.is_none()
    }
    
    fn error_msg(&self) -> String {
        self.msg.clone().unwrap_or_else(|| "Unknown error".to_string())
    }
}

/// KuCoin order result
#[derive(Debug, Deserialize)]
struct KuCoinOrderResult {
    order_id: Option<String>,
    #[serde(rename = "orderId")]
    order_id_alt: Option<String>,
}

impl KuCoinOrderResult {
    fn order_id(&self) -> Option<String> {
        self.order_id.clone().or_else(|| self.order_id_alt.clone())
    }
}

/// KuCoin position result
#[derive(Debug, Deserialize)]
struct KuCoinPositionResult {
    positions: Option<Vec<KuCoinPosition>>,
    items: Option<Vec<KuCoinPosition>>,
}

/// KuCoin position
#[derive(Debug, Clone, Deserialize)]
struct KuCoinPosition {
    symbol: String,
    side: String,
    #[serde(rename = "currentQty")]
    current_qty: Option<String>,
    size: Option<String>,
    #[serde(rename = "avgEntryPrice")]
    avg_entry_price: Option<String>,
    #[serde(rename = "markPrice")]
    mark_price: Option<String>,
    #[serde(rename = "unrealisedPnl")]
    unrealized_pnl: Option<String>,
    leverage: Option<String>,
    #[serde(rename = "currentCost")]
    current_cost: Option<String>,
}

/// KuCoin Plugin implementation
pub struct KuCoinPlugin {
    name: String,
    config: Arc<RwLock<Option<KuCoinConfig>>>,
    client: Client,
    base_url: String,
}

impl KuCoinPlugin {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            config: Arc::new(RwLock::new(None)),
            client: Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .build()
                .expect("Failed to create HTTP client"),
            base_url: "https://api.kucoin.com".to_string(),
        }
    }
    
    /// Get base URL (sandbox or mainnet)
    fn get_base_url(&self, testnet: bool) -> &str {
        if testnet {
            "https://openapi-sandbox.kucoin.com"
        } else {
            "https://api.kucoin.com"
        }
    }
    
    /// Generate HMAC-SHA256 signature and base64 encode
    fn generate_signature(secret: &str, message: &str) -> String {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        use base64::{Engine as _, engine::general_purpose};
        
        type HmacSha256 = Hmac<Sha256>;
        
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(message.as_bytes());
        let result = mac.finalize();
        general_purpose::STANDARD.encode(result.into_bytes())
    }
    
    /// Encrypt passphrase using HMAC-SHA256 with API secret, then base64 encode
    fn encrypt_passphrase(secret: &str, passphrase: &str) -> String {
        use hmac::{Hmac, Mac};
        use sha2::Sha256;
        use base64::{Engine as _, engine::general_purpose};
        
        type HmacSha256 = Hmac<Sha256>;
        
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes())
            .expect("HMAC can take key of any size");
        mac.update(passphrase.as_bytes());
        let result = mac.finalize();
        general_purpose::STANDARD.encode(result.into_bytes())
    }
    
    /// Create authenticated request headers for KuCoin API
    async fn create_headers(
        &self,
        method: &str,
        endpoint: &str,
        body: &str,
        api_key: &str,
        api_secret: &str,
        api_passphrase: &str,
    ) -> Result<reqwest::header::HeaderMap, Box<dyn Error + Send + Sync>> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis()
            .to_string();
        
        // Prehash string: timestamp + method + endpoint + body
        let prehash_string = format!("{}{}{}{}", timestamp, method, endpoint, body);
        
        // Generate signature
        let signature = Self::generate_signature(api_secret, &prehash_string);
        
        // Encrypt passphrase
        let encrypted_passphrase = Self::encrypt_passphrase(api_secret, api_passphrase);
        
        let mut headers = reqwest::header::HeaderMap::new();
        headers.insert("KC-API-KEY", api_key.parse()?);
        headers.insert("KC-API-SIGN", signature.parse()?);
        headers.insert("KC-API-TIMESTAMP", timestamp.parse()?);
        headers.insert("KC-API-PASSPHRASE", encrypted_passphrase.parse()?);
        headers.insert("KC-API-KEY-VERSION", "2".parse()?);
        headers.insert("Content-Type", "application/json".parse()?);
        
        Ok(headers)
    }
    
    /// Set leverage for a symbol (KuCoin Futures-specific)
    pub async fn set_leverage(
        &self,
        symbol: &str,
        leverage: i32,
    ) -> Result<(), Box<dyn Error + Send + Sync>> {
        let config = self.config.read().await;
        let config = config.as_ref()
            .ok_or("Plugin not initialized")?;
        
        if config.trading_type != "futures" {
            return Err("Leverage setting only available for futures trading".into());
        }
        
        let base_url = self.get_base_url(config.testnet);
        let endpoint = "/api/v1/leverage";
        
        let params = serde_json::json!({
            "symbol": symbol,
            "leverage": leverage.to_string(),
        });
        
        let body = serde_json::to_string(&params)?;
        let headers = self.create_headers(
            "POST",
            endpoint,
            &body,
            &config.api_key,
            &config.api_secret,
            &config.api_passphrase,
        ).await?;
        
        let url = format!("{}{}", base_url, endpoint);
        let response = self.client
            .post(&url)
            .headers(headers)
            .body(body)
            .send()
            .await?;
        
        let status = response.status();
        let text = response.text().await?;
        
        if !status.is_success() {
            return Err(format!("KuCoin API error ({}): {}", status, text).into());
        }
        
        let kucoin_resp: KuCoinResponse<serde_json::Value> = serde_json::from_str(&text)?;
        
        if !kucoin_resp.is_success() {
            return Err(format!("KuCoin API error: {} - {}", kucoin_resp.code.as_deref().unwrap_or("unknown"), kucoin_resp.error_msg()).into());
        }
        
        tracing::info!(plugin = %self.name, symbol = %symbol, leverage = %leverage, "Leverage set successfully");
        Ok(())
    }
    
    /// Get positions for a symbol (KuCoin Futures)
    pub async fn get_position(
        &self,
        symbol: &str,
    ) -> Result<Option<KuCoinPosition>, Box<dyn Error + Send + Sync>> {
        let config = self.config.read().await;
        let config = config.as_ref()
            .ok_or("Plugin not initialized")?;
        
        if config.trading_type != "futures" {
            return Err("Position queries only available for futures trading".into());
        }
        
        let base_url = self.get_base_url(config.testnet);
        let endpoint = format!("/api/v1/positions?symbol={}", symbol);
        
        let headers = self.create_headers(
            "GET",
            &endpoint,
            "",
            &config.api_key,
            &config.api_secret,
            &config.api_passphrase,
        ).await?;
        
        let url = format!("{}{}", base_url, endpoint);
        let response = self.client
            .get(&url)
            .headers(headers)
            .send()
            .await?;
        
        let status = response.status();
        let text = response.text().await?;
        
        if !status.is_success() {
            return Err(format!("KuCoin API error ({}): {}", status, text).into());
        }
        
        let kucoin_resp: KuCoinResponse<KuCoinPositionResult> = serde_json::from_str(&text)?;
        
        if !kucoin_resp.is_success() {
            return Err(format!("KuCoin API error: {} - {}", kucoin_resp.code.as_deref().unwrap_or("unknown"), kucoin_resp.error_msg()).into());
        }
        
        if let Some(data) = kucoin_resp.data {
            let positions = data.positions.or(data.items).unwrap_or_default();
            if let Some(position) = positions.first() {
                // Filter for active positions (non-zero size)
                let size = position.current_qty.as_ref()
                    .or(position.size.as_ref())
                    .and_then(|s| s.parse::<f64>().ok())
                    .unwrap_or(0.0);
                
                if size.abs() > 0.0 {
                    return Ok(Some(position.clone()));
                }
            }
        }
        
        Ok(None)
    }
}

#[async_trait]
impl ExecutionPlugin for KuCoinPlugin {
    async fn init(&mut self, config: serde_json::Value) -> Result<(), Box<dyn Error + Send + Sync>> {
        let kucoin_config: KuCoinConfig = serde_json::from_value(config)?;
        
        tracing::info!(
            plugin = %self.name,
            testnet = %kucoin_config.testnet,
            trading_type = %kucoin_config.trading_type,
            "Initializing KuCoin plugin"
        );
        
        // Validate API credentials
        if kucoin_config.api_key.is_empty() || kucoin_config.api_secret.is_empty() || kucoin_config.api_passphrase.is_empty() {
            return Err("KuCoin API key, secret, and passphrase must be provided".into());
        }
        
        // Update base URL
        self.base_url = self.get_base_url(kucoin_config.testnet).to_string();
        
        *self.config.write().await = Some(kucoin_config);
        
        tracing::info!(plugin = %self.name, "KuCoin plugin initialized successfully");
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
        
        // Choose endpoint based on trading type
        let endpoint = if config.trading_type == "futures" {
            "/api/v1/orders"
        } else {
            "/api/v1/orders"
        };
        
        // Convert Order to KuCoin format
        let side = match order.side {
            OrderSide::Buy => "buy",
            OrderSide::Sell => "sell",
        };
        
        let order_type = match order.order_type {
            OrderType::Market => "market",
            OrderType::Limit => "limit",
            OrderType::Stop => "stop",
            OrderType::StopLimit => "stopLimit",
            OrderType::TakeProfit => "takeProfit",
            OrderType::StopLoss => "stopLoss",
        };
        
        // Convert symbol format (BTCUSDT -> BTC-USDT for KuCoin)
        let kucoin_symbol = if order.symbol.contains("-") {
            order.symbol.clone()
        } else {
            // Try to split USDT pairs
            if order.symbol.ends_with("USDT") {
                let base = &order.symbol[..order.symbol.len() - 4];
                format!("{}-USDT", base)
            } else {
                order.symbol.clone()
            }
        };
        
        // Build order parameters
        let mut params = serde_json::json!({
            "clientOid": format!("fks-{}", SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_millis()),
            "side": side,
            "symbol": kucoin_symbol,
            "type": order_type,
        });
        
        // Add size (KuCoin uses "size" for spot, "size" for futures too)
        params["size"] = serde_json::json!(order.quantity.to_string());
        
        // Add price for limit orders
        if let Some(price) = order.price {
            params["price"] = serde_json::json!(price.to_string());
        }
        
        // Add stop-loss and take-profit if provided (futures only)
        if config.trading_type == "futures" {
            if let Some(stop_loss) = order.stop_loss {
                params["stop"] = serde_json::json!("down");
                params["stopPrice"] = serde_json::json!(stop_loss.to_string());
            }
            
            if let Some(take_profit) = order.take_profit {
                // KuCoin futures uses separate take profit orders
                // For now, we'll log it but not set it in the main order
                tracing::debug!(take_profit = %take_profit, "Take profit specified (may need separate order)");
            }
            
            // Add leverage if configured
            params["leverage"] = serde_json::json!(config.leverage.to_string());
        }
        
        let body = serde_json::to_string(&params)?;
        let headers = self.create_headers(
            "POST",
            endpoint,
            &body,
            &config.api_key,
            &config.api_secret,
            &config.api_passphrase,
        ).await?;
        
        let url = format!("{}{}", base_url, endpoint);
        let response = self.client
            .post(&url)
            .headers(headers)
            .body(body)
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
        
        let kucoin_resp: KuCoinResponse<KuCoinOrderResult> = serde_json::from_str(&text)?;
        
        if !kucoin_resp.is_success() {
            return Ok(ExecutionResult {
                success: false,
                order_id: None,
                filled_quantity: 0.0,
                average_price: 0.0,
                error: Some(format!("KuCoin API error: {} - {}", kucoin_resp.code.as_deref().unwrap_or("unknown"), kucoin_resp.error_msg())),
                timestamp: SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_millis() as i64,
            });
        }
        
        // Extract order ID from result
        let order_id = kucoin_resp.data
            .and_then(|r| r.order_id());
        
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
        
        // Convert symbol format if needed
        let kucoin_symbol = if symbol.contains("-") {
            symbol.to_string()
        } else if symbol.ends_with("USDT") {
            let base = &symbol[..symbol.len() - 4];
            format!("{}-USDT", base)
        } else {
            symbol.to_string()
        };
        
        // Use market data endpoint (public, no auth required)
        let endpoint = if config.trading_type == "futures" {
            format!("/api/v1/ticker?symbol={}", kucoin_symbol)
        } else {
            format!("/api/v1/market/orderbook/level1?symbol={}", kucoin_symbol)
        };
        
        let url = format!("{}{}", base_url, endpoint);
        let response = self.client
            .get(&url)
            .send()
            .await?;
        
        let status = response.status();
        let text = response.text().await?;
        
        if !status.is_success() {
            return Err(format!("KuCoin API error ({}): {}", status, text).into());
        }
        
        #[derive(Deserialize)]
        struct TickerData {
            price: Option<String>,
            #[serde(rename = "bestBid")]
            best_bid: Option<String>,
            #[serde(rename = "bestAsk")]
            best_ask: Option<String>,
            #[serde(rename = "last")]
            last_price: Option<String>,
            #[serde(rename = "bestBidSize")]
            best_bid_size: Option<String>,
            #[serde(rename = "bestAskSize")]
            best_ask_size: Option<String>,
            volume: Option<String>,
        }
        
        #[derive(Deserialize)]
        struct TickerResponse {
            data: Option<TickerData>,
        }
        
        let kucoin_resp: KuCoinResponse<TickerData> = serde_json::from_str(&text)?;
        
        if !kucoin_resp.is_success() {
            return Err(format!("KuCoin API error: {} - {}", kucoin_resp.code.as_deref().unwrap_or("unknown"), kucoin_resp.error_msg()).into());
        }
        
        if let Some(ticker) = kucoin_resp.data {
            let price_str = ticker.price
                .or(ticker.last_price)
                .ok_or("No price data available")?;
            
            let last = price_str.parse::<f64>()?;
            let bid = ticker.best_bid
                .and_then(|b| b.parse::<f64>().ok())
                .unwrap_or(last);
            let ask = ticker.best_ask
                .and_then(|a| a.parse::<f64>().ok())
                .unwrap_or(last);
            let volume = ticker.volume
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
        // This verifies connectivity to KuCoin API
        match self.fetch_data("BTC-USDT").await {
            Ok(_) => Ok(true),
            Err(_) => Ok(false), // Return false but don't error
        }
    }
}
