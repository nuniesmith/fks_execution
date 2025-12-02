use axum::{routing::{get, post}, Router, Json, extract::{State, Path, Query}, http::StatusCode};
use clap::Parser;
use serde::Serialize;
use std::{net::SocketAddr, time::{Instant, Duration}, sync::Arc};
use tokio::signal;
use serde::Deserialize;

// Plugin framework
mod plugins;
mod health;
use plugins::{
    registry::PluginRegistry, 
    ccxt::CCXTPlugin,
    bybit::BybitPlugin,
    kucoin::KuCoinPlugin,
    Order, OrderSide, OrderType,
    ExecutionPlugin
};

#[derive(Parser, Debug)]
#[command(version, about="FKS Execution API")] 
struct Cli { 
    #[arg(long, default_value="0.0.0.0:8005")] 
    listen: String 
}

#[derive(Serialize, Clone)] struct Signal { symbol: String, rsi: f64, ema: f64, risk_allowance: f64, latency_ms: u128 }

#[derive(Deserialize)] struct SignalRequest { symbol: Option<String>, prices: Option<Vec<f64>> }

#[derive(Serialize)] struct Health { service: String, status: String }

#[derive(Clone)]
struct AppState { 
    start: Instant,
    registry: Arc<PluginRegistry>
}

#[derive(Deserialize)]
struct TradingViewWebhook {
    symbol: String,
    action: String, // "buy" or "sell"
    order_type: Option<String>, // "market", "limit"
    quantity: f64,
    price: Option<f64>,
    stop_loss: Option<f64>,
    take_profit: Option<f64>,
    confidence: Option<f64>,
}

#[derive(Serialize)]
struct WebhookResponse {
    success: bool,
    order_id: Option<String>,
    error: Option<String>,
}

/// Order creation request
#[derive(Deserialize)]
struct CreateOrderRequest {
    exchange: String,
    symbol: String,
    side: String, // "buy" or "sell"
    order_type: String, // "market", "limit", etc.
    quantity: f64,
    price: Option<f64>,
    leverage: Option<i32>,
    stop_loss: Option<f64>,
    take_profit: Option<f64>,
    category: Option<String>, // For Bybit: "linear", "spot", etc.
}

/// Order creation response
#[derive(Serialize)]
struct CreateOrderResponse {
    success: bool,
    order_id: Option<String>,
    filled_quantity: f64,
    average_price: f64,
    error: Option<String>,
    timestamp: i64,
}

/// Set leverage request
#[derive(Deserialize)]
struct SetLeverageRequest {
    symbol: String,
    leverage: i32,
    category: Option<String>,
}

/// Set leverage response
#[derive(Serialize)]
struct SetLeverageResponse {
    success: bool,
    error: Option<String>,
}

/// Position query parameters
#[derive(Deserialize)]
struct PositionQuery {
    exchange: String,
    symbol: Option<String>,
}

/// Position response
#[derive(Serialize)]
struct PositionResponse {
    symbol: String,
    side: String,
    size: f64,
    entry_price: f64,
    mark_price: f64,
    unrealized_pnl: f64,
    leverage: i32,
    margin: f64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("[execution] main_enter");
    // Install a panic hook to surface any silent panics
    std::panic::set_hook(Box::new(|info| {
        eprintln!("[panic] {info}");
    }));
    eprintln!("[execution] initializing tracing");
    tracing_subscriber::fmt::init();
    eprintln!("[execution] tracing initialized");
    tracing::info!("startup_begin");
    eprintln!("[execution] parsing CLI");
    let mut cli = Cli::parse();
    eprintln!("[execution] CLI parsed: listen={}", cli.listen);
    
    // Override listen address with SERVICE_PORT if set
    if let Ok(port) = std::env::var("SERVICE_PORT") {
        cli.listen = format!("0.0.0.0:{}", port);
    }
    
    tracing::info!(listen = %cli.listen, "parsed_cli");
    
    // Initialize plugin registry
    let registry = Arc::new(PluginRegistry::new());
    
    // Initialize CCXT plugin (non-fatal - service can run without it)
    let mut ccxt = CCXTPlugin::new("binance");
    let ccxt_config = serde_json::json!({
        "base_url": std::env::var("CCXT_BASE_URL").unwrap_or_else(|_| "http://localhost:8000".to_string()),
        "webhook_secret": std::env::var("WEBHOOK_SECRET").unwrap_or_else(|_| "fks-tradingview-webhook-secret-dev-2025".to_string()),
        "exchange": std::env::var("EXCHANGE").unwrap_or_else(|_| "binance".to_string()),
        "testnet": std::env::var("TESTNET").unwrap_or_else(|_| "false".to_string()) == "true"
    });
    
    match ccxt.init(ccxt_config).await {
        Ok(_) => {
            registry.register("binance".to_string(), Arc::new(ccxt)).await;
            tracing::info!("ccxt_plugin_registered");
        }
        Err(e) => {
            tracing::warn!(error=%e, "ccxt_plugin_init_failed_continuing_without");
            // Continue without CCXT plugin - service can still run with other plugins
        }
    }
    
    // Initialize Bybit plugin (non-fatal - service can run without it)
    if let (Ok(api_key), Ok(api_secret)) = (
        std::env::var("BYBIT_API_KEY"),
        std::env::var("BYBIT_API_SECRET")
    ) {
        let mut bybit = BybitPlugin::new("bybit");
        let bybit_config = serde_json::json!({
            "api_key": api_key,
            "api_secret": api_secret,
            "testnet": std::env::var("BYBIT_TESTNET").unwrap_or_else(|_| "false".to_string()) == "true",
            "category": std::env::var("BYBIT_CATEGORY").unwrap_or_else(|_| "linear".to_string()),
            "leverage": std::env::var("BYBIT_LEVERAGE")
                .unwrap_or_else(|_| "10".to_string())
                .parse::<i32>()
                .unwrap_or(10)
        });
        
        match bybit.init(bybit_config).await {
            Ok(_) => {
                registry.register("bybit".to_string(), Arc::new(bybit)).await;
                tracing::info!("bybit_plugin_registered");
            }
            Err(e) => {
                tracing::warn!(error=%e, "bybit_plugin_init_failed_continuing_without");
                // Continue without Bybit plugin - service can still run with other plugins
            }
        }
    } else {
        tracing::info!("bybit_api_keys_not_configured_skipping_bybit_plugin");
    }
    
    // Initialize KuCoin plugin (Canada-compliant, non-fatal - service can run without it)
    if let (Ok(api_key), Ok(api_secret), Ok(api_passphrase)) = (
        std::env::var("KUCOIN_API_KEY"),
        std::env::var("KUCOIN_API_SECRET"),
        std::env::var("KUCOIN_API_PASSPHRASE")
    ) {
        let mut kucoin = KuCoinPlugin::new("kucoin");
        let kucoin_config = serde_json::json!({
            "api_key": api_key,
            "api_secret": api_secret,
            "api_passphrase": api_passphrase,
            "testnet": std::env::var("KUCOIN_TESTNET").unwrap_or_else(|_| "false".to_string()) == "true",
            "trading_type": std::env::var("KUCOIN_TRADING_TYPE").unwrap_or_else(|_| "futures".to_string()),
            "leverage": std::env::var("KUCOIN_LEVERAGE")
                .unwrap_or_else(|_| "10".to_string())
                .parse::<i32>()
                .unwrap_or(10)
        });
        
        match kucoin.init(kucoin_config).await {
            Ok(_) => {
                registry.register("kucoin".to_string(), Arc::new(kucoin)).await;
                tracing::info!("kucoin_plugin_registered");
            }
            Err(e) => {
                tracing::warn!(error=%e, "kucoin_plugin_init_failed_continuing_without");
                // Continue without KuCoin plugin - service can still run with other plugins
            }
        }
    } else {
        tracing::info!("kucoin_api_credentials_not_configured_skipping_kucoin_plugin");
    }
    
    let state = AppState { 
        start: Instant::now(),
        registry: registry.clone()
    };
    
    let signal_routes = Router::new()
        .route("/execute/signal", get(get_signal_handler))
        .route("/execute/signal", post(post_signal_handler));
    
    let webhook_routes = Router::new()
        .route("/webhook/tradingview", post(tradingview_webhook_handler));
    
    // Order execution API routes
    let order_routes = Router::new()
        .route("/api/v1/orders", post(create_order_handler))
        .route("/api/v1/exchanges/{exchange}/leverage", post(set_leverage_handler))
        .route("/api/v1/positions", get(get_positions_handler));

    // Set up Prometheus metrics
    let (prometheus_layer, metric_handle) = {
        use axum_prometheus::PrometheusMetricLayer;
        use prometheus::{Gauge, IntGaugeVec, Registry, Encoder, TextEncoder};
        use std::env;
        
        let (layer, axum_handle) = PrometheusMetricLayer::pair();
        let registry = Registry::new();
        
        // Build info
        let commit = env::var("GIT_COMMIT")
            .or_else(|_| env::var("COMMIT_SHA"))
            .unwrap_or_else(|_| "unknown".to_string());
        let build_date = env::var("BUILD_DATE")
            .or_else(|_| env::var("BUILD_TIMESTAMP"))
            .unwrap_or_else(|_| "unknown".to_string());
        
        let build_info = Gauge::with_opts(
            prometheus::opts!(
                "fks_build_info",
                "Build information for FKS service"
            )
            .const_label("service", "fks_execution")
            .const_label("version", "0.1.0")
            .const_label("commit", &commit[..commit.len().min(8)])
            .const_label("build_date", &build_date),
        ).expect("Failed to create build_info metric");
        build_info.set(1.0);
        registry.register(Box::new(build_info)).expect("Failed to register build_info");
        
        // Service health
        let service_health = IntGaugeVec::new(
            prometheus::opts!("fks_service_health", "Service health status (1=healthy, 0=unhealthy)"),
            &["service"],
        ).expect("Failed to create service_health metric");
        service_health.with_label_values(&["fks_execution"]).set(1);
        registry.register(Box::new(service_health)).expect("Failed to register service_health");
        
        // Create combined handle
        struct MetricHandle {
            registry: Registry,
            axum_handle: axum_prometheus::MetricHandle,
        }
        impl MetricHandle {
            fn render(&self) -> String {
                let mut output = self.axum_handle.render();
                let encoder = TextEncoder::new();
                let metric_families = self.registry.gather();
                let mut buffer = Vec::new();
                if encoder.encode(&metric_families, &mut buffer).is_ok() {
                    if let Ok(metrics_text) = String::from_utf8(buffer) {
                        output.push_str(&metrics_text);
                    }
                }
                output
            }
        }
        let handle = MetricHandle { registry, axum_handle };
        (layer, handle)
    };
    
    let app = Router::new()
        .merge(health::health_routes())
        .merge(signal_routes)
        .merge(webhook_routes)
        .merge(order_routes)
        .route("/metrics", get(|| async move { metric_handle.render() }))
        .layer(prometheus_layer)
        .with_state(Arc::new(state));
    let addr: SocketAddr = match cli.listen.parse() { Ok(a) => a, Err(e) => { tracing::error!(error=%e, "addr_parse_failed"); return Err(e.into()); } };
    tracing::info!(%addr, "binding_listener");
    let listener = match tokio::net::TcpListener::bind(addr).await { Ok(l) => l, Err(e) => { tracing::error!(error=%e, "bind_failed"); return Err(e.into()); } };
    tracing::info!("listener_bound");
    let server = axum::serve(listener, app);
    tracing::info!("server_future_created");
    tokio::select! {
        res = server => {
            if let Err(e) = res { tracing::error!(error=%e, "server_terminated_error"); }
            tracing::warn!("server_future_completed_unexpectedly");
        }
        _ = shutdown_signal() => {
            tracing::info!("shutdown signal received");
        }
    }
    // If we get here the server ended unexpectedly; keep process alive for inspection
    tracing::warn!("execution_main_exiting_loop_enter");
    loop {
        tokio::time::sleep(Duration::from_secs(3600)).await;
    }
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.expect("failed to install Ctrl+C handler");
    };
    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();
    tokio::select! { _ = ctrl_c => {}, _ = terminate => {} }
}

async fn get_signal_handler() -> Json<Signal> {
    build_signal(None).await
}

async fn post_signal_handler(Json(req): Json<SignalRequest>) -> Json<Signal> {
    let symbol = req.symbol.clone();
    let prices = req.prices.clone();
    build_signal(symbol.zip(prices)).await
}

async fn build_signal(input: Option<(String, Vec<f64>)>) -> Json<Signal> {
    let start = Instant::now();
    let (symbol, prices) = match input {
        Some((sym, p)) if !p.is_empty() => (sym, p),
        _ => ("ES".to_string(), vec![4420.0, 4422.0, 4419.5, 4425.0, 4424.0])
    };
    let rsi = 55.0; // placeholder
    let ema: f64 = prices.iter().sum::<f64>() / prices.len() as f64;
    let risk_allowance = 150000.0 * 0.01;
    tokio::time::sleep(Duration::from_millis(5)).await;
    Json(Signal { symbol, rsi, ema, risk_allowance, latency_ms: start.elapsed().as_millis() })
}

async fn health_handler(State(state): State<Arc<AppState>>) -> Json<Health> {
    let uptime = state.start.elapsed().as_secs();
    
    // Check all plugin health
    let plugins_healthy = state.registry.health_check_all().await.iter().all(|(_, healthy)| *healthy);
    let status = if plugins_healthy { "healthy" } else { "degraded" };
    
    Json(Health { 
        service: format!("fks-execution|uptime={uptime}s|plugins={}", state.registry.list_plugins().await.len()), 
        status: status.into() 
    })
}

async fn tradingview_webhook_handler(
    State(state): State<Arc<AppState>>,
    Json(webhook): Json<TradingViewWebhook>
) -> Result<Json<WebhookResponse>, (StatusCode, Json<WebhookResponse>)> {
    tracing::info!(symbol = %webhook.symbol, action = %webhook.action, "webhook_received");
    
    // Convert TradingView action to OrderSide
    let side = match webhook.action.to_lowercase().as_str() {
        "buy" => OrderSide::Buy,
        "sell" => OrderSide::Sell,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(WebhookResponse {
                    success: false,
                    order_id: None,
                    error: Some(format!("Invalid action: {}", webhook.action))
                })
            ));
        }
    };
    
    // Convert order type
    let order_type = match webhook.order_type.as_deref().unwrap_or("market") {
        "market" => OrderType::Market,
        "limit" => OrderType::Limit,
        "stop" => OrderType::Stop,
        "stop_limit" => OrderType::StopLimit,
        _ => OrderType::Market,
    };
    
    // Create order
    let order = Order {
        symbol: webhook.symbol.clone(),
        side,
        order_type,
        quantity: webhook.quantity,
        price: webhook.price,
        stop_loss: webhook.stop_loss,
        take_profit: webhook.take_profit,
        confidence: webhook.confidence.unwrap_or(0.7),
    };
    
    // Execute order via plugin registry (use default plugin)
    match state.registry.execute_order(order, None).await {
        Ok(result) => {
            if result.success {
                tracing::info!(order_id = ?result.order_id, filled = result.filled_quantity, "order_executed");
                Ok(Json(WebhookResponse {
                    success: true,
                    order_id: result.order_id,
                    error: None,
                }))
            } else {
                tracing::warn!(error = ?result.error, "order_failed");
                Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(WebhookResponse {
                        success: false,
                        order_id: None,
                        error: result.error,
                    })
                ))
            }
        },
        Err(e) => {
            tracing::error!(error = %e, "order_execution_error");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(WebhookResponse {
                    success: false,
                    order_id: None,
                    error: Some(format!("Execution error: {}", e))
                })
            ))
        }
    }
}

/// Create order endpoint: POST /api/v1/orders
async fn create_order_handler(
    State(state): State<Arc<AppState>>,
    Json(req): Json<CreateOrderRequest>
) -> Result<Json<CreateOrderResponse>, (StatusCode, Json<CreateOrderResponse>)> {
    tracing::info!(
        exchange = %req.exchange,
        symbol = %req.symbol,
        side = %req.side,
        order_type = %req.order_type,
        "create_order_request"
    );
    
    // Convert side
    let side = match req.side.to_lowercase().as_str() {
        "buy" => OrderSide::Buy,
        "sell" => OrderSide::Sell,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(CreateOrderResponse {
                    success: false,
                    order_id: None,
                    filled_quantity: 0.0,
                    average_price: 0.0,
                    error: Some(format!("Invalid side: {}", req.side)),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as i64,
                })
            ));
        }
    };
    
    // Convert order type
    let order_type = match req.order_type.to_lowercase().as_str() {
        "market" => OrderType::Market,
        "limit" => OrderType::Limit,
        "stop" => OrderType::Stop,
        "stop_limit" | "stoplimit" => OrderType::StopLimit,
        "take_profit" | "takeprofit" => OrderType::TakeProfit,
        "stop_loss" | "stoploss" => OrderType::StopLoss,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(CreateOrderResponse {
                    success: false,
                    order_id: None,
                    filled_quantity: 0.0,
                    average_price: 0.0,
                    error: Some(format!("Invalid order_type: {}", req.order_type)),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as i64,
                })
            ));
        }
    };
    
    // Create order
    let order = Order {
        symbol: req.symbol.clone(),
        side,
        order_type,
        quantity: req.quantity,
        price: req.price,
        stop_loss: req.stop_loss,
        take_profit: req.take_profit,
        confidence: 0.7, // Default confidence
    };
    
    // Execute order via specified plugin
    match state.registry.execute_order(order, Some(&req.exchange)).await {
        Ok(result) => {
            tracing::info!(
                exchange = %req.exchange,
                symbol = %req.symbol,
                order_id = ?result.order_id,
                filled = result.filled_quantity,
                "order_executed"
            );
            Ok(Json(CreateOrderResponse {
                success: result.success,
                order_id: result.order_id,
                filled_quantity: result.filled_quantity,
                average_price: result.average_price,
                error: result.error,
                timestamp: result.timestamp,
            }))
        },
        Err(e) => {
            tracing::error!(exchange = %req.exchange, error = %e, "order_execution_error");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(CreateOrderResponse {
                    success: false,
                    order_id: None,
                    filled_quantity: 0.0,
                    average_price: 0.0,
                    error: Some(format!("Execution error: {}", e)),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_millis() as i64,
                })
            ))
        }
    }
}

/// Set leverage endpoint: POST /api/v1/exchanges/{exchange}/leverage
async fn set_leverage_handler(
    State(state): State<Arc<AppState>>,
    Path(exchange): Path<String>,
    Json(req): Json<SetLeverageRequest>
) -> Result<Json<SetLeverageResponse>, (StatusCode, Json<SetLeverageResponse>)> {
    tracing::info!(
        exchange = %exchange,
        symbol = %req.symbol,
        leverage = %req.leverage,
        "set_leverage_request"
    );
    
    // Get plugin
    let _plugin = state.registry.get(&exchange).await
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(SetLeverageResponse {
                    success: false,
                    error: Some(format!("Exchange plugin '{}' not found", exchange)),
                })
            )
        })?;
    
    // Check if plugin is Bybit plugin (has set_leverage method)
    // We need to downcast to BybitPlugin to access set_leverage
    // For now, we'll try to get it as a trait object and call a method
    // This requires adding set_leverage to the ExecutionPlugin trait or using a type check
    
    // For now, return an error if not Bybit
    if exchange != "bybit" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(SetLeverageResponse {
                success: false,
                error: Some(format!("Leverage setting only supported for Bybit, got: {}", exchange)),
            })
        ));
    }
    
    // Try to get Bybit plugin from registry and call set_leverage
    // This is a limitation - we need to store plugin type information
    // For now, we'll require Bybit to be the exchange
    // TODO: Add set_leverage to ExecutionPlugin trait or use a plugin-specific registry
    
    // Since we can't easily downcast, we'll need to add this to the trait or use a workaround
    // For Day 17, we'll document this limitation and implement the basic structure
    
    Err((
        StatusCode::NOT_IMPLEMENTED,
        Json(SetLeverageResponse {
            success: false,
            error: Some("Leverage setting requires plugin-specific implementation. Use Bybit plugin directly.".to_string()),
        })
    ))
}

/// Get positions endpoint: GET /api/v1/positions?exchange=bybit&symbol=BTCUSDT
async fn get_positions_handler(
    State(state): State<Arc<AppState>>,
    Query(params): Query<PositionQuery>
) -> Result<Json<PositionResponse>, (StatusCode, Json<serde_json::Value>)> {
    tracing::info!(
        exchange = %params.exchange,
        symbol = ?params.symbol,
        "get_positions_request"
    );
    
    // Get plugin
    let _plugin = state.registry.get(&params.exchange).await
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(serde_json::json!({
                    "error": format!("Exchange plugin '{}' not found", params.exchange)
                }))
            )
        })?;
    
    // For now, position queries require plugin-specific implementation
    // This is similar to leverage - we need plugin-specific methods or trait extensions
    
    Err((
        StatusCode::NOT_IMPLEMENTED,
        Json(serde_json::json!({
            "error": "Position queries require plugin-specific implementation. Use Bybit plugin directly."
        }))
    ))
}
