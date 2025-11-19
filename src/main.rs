use axum::{routing::{get, post}, Router, Json, extract::State, http::StatusCode};
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

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("[execution] main_enter");
    // Install a panic hook to surface any silent panics
    std::panic::set_hook(Box::new(|info| {
        eprintln!("[panic] {info}");
    }));
    tracing_subscriber::fmt::init();
    tracing::info!("startup_begin");
    let mut cli = Cli::parse();
    
    // Override listen address with SERVICE_PORT if set
    if let Ok(port) = std::env::var("SERVICE_PORT") {
        cli.listen = format!("0.0.0.0:{}", port);
    }
    
    tracing::info!(listen = %cli.listen, "parsed_cli");
    
    // Initialize plugin registry
    let registry = Arc::new(PluginRegistry::new());
    
    // Initialize CCXT plugin
    let mut ccxt = CCXTPlugin::new("binance");
    let ccxt_config = serde_json::json!({
        "base_url": std::env::var("CCXT_BASE_URL").unwrap_or_else(|_| "http://localhost:8000".to_string()),
        "webhook_secret": std::env::var("WEBHOOK_SECRET").unwrap_or_else(|_| "fks-tradingview-webhook-secret-dev-2025".to_string()),
        "exchange": std::env::var("EXCHANGE").unwrap_or_else(|_| "binance".to_string()),
        "testnet": std::env::var("TESTNET").unwrap_or_else(|_| "false".to_string()) == "true"
    });
    
    if let Err(e) = ccxt.init(ccxt_config).await {
        tracing::error!(error=%e, "ccxt_plugin_init_failed");
        return Err(anyhow::anyhow!("CCXT plugin init failed: {}", e));
    }
    
    registry.register("binance".to_string(), Arc::new(ccxt)).await;
    tracing::info!("ccxt_plugin_registered");
    
    let state = AppState { 
        start: Instant::now(),
        registry: registry.clone()
    };
    
    let signal_routes = Router::new()
        .route("/execute/signal", get(get_signal_handler))
        .route("/execute/signal", post(post_signal_handler));
    
    let webhook_routes = Router::new()
        .route("/webhook/tradingview", post(tradingview_webhook_handler));

    let app = Router::new()
        .merge(health::health_routes())
        .merge(signal_routes)
        .merge(webhook_routes)
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
            tracing::error!(error = %e, "plugin_execution_error");
            Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(WebhookResponse {
                    success: false,
                    order_id: None,
                    error: Some(e.to_string()),
                })
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    include!("../tests/webhook_integration_test.rs");
}
