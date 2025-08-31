use axum::{routing::{get, post}, Router, Json, extract::State};
use clap::Parser;
use serde::Serialize;
use std::{net::SocketAddr, time::{Instant, Duration}, sync::Arc};
use tokio::signal;
use serde::Deserialize;

#[derive(Parser, Debug)]
#[command(version, about="FKS Execution API")] struct Cli { #[arg(long, default_value="0.0.0.0:4700")] listen: String }

#[derive(Serialize, Clone)] struct Signal { symbol: String, rsi: f64, ema: f64, risk_allowance: f64, latency_ms: u128 }

#[derive(Deserialize)] struct SignalRequest { symbol: Option<String>, prices: Option<Vec<f64>> }

#[derive(Serialize)] struct Health { service: String, status: String }

#[derive(Clone)]
struct AppState { start: Instant }

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();
    let state = AppState { start: Instant::now() };
    let signal_routes = Router::new()
        .route("/execute/signal", get(get_signal_handler))
        .route("/execute/signal", post(post_signal_handler));

    let app = Router::new()
        .route("/health", get(health_handler))
        .merge(signal_routes)
        .with_state(Arc::new(state));
    let addr: SocketAddr = cli.listen.parse()?;
    tracing::info!(%addr, "execution api listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;

    let server = axum::serve(listener, app);
    tokio::select! {
        res = server => { res?; }
        _ = shutdown_signal() => {
            tracing::info!("shutdown signal received");
        }
    }
    Ok(())
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
    Json(Health { service: format!("fks-execution|uptime={uptime}s"), status: "healthy".into() })
}
