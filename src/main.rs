use axum::{routing::get, Router, Json};
use clap::Parser;
use serde::Serialize;
use std::{net::SocketAddr, time::{Instant, Duration}};

#[derive(Parser, Debug)]
#[command(version, about="FKS Execution API")] struct Cli { #[arg(long, default_value="0.0.0.0:4700")] listen: String }

#[derive(Serialize)] struct Signal { symbol: String, rsi: f64, ema: f64, risk_allowance: f64, latency_ms: u128 }

#[derive(Serialize)] struct Health { service: String, status: String }

#[tokio::main]
async fn main() -> anyhow::Result<()> { 
    tracing_subscriber::fmt::init(); 
    let cli = Cli::parse(); 
    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/execute/signal", get(signal_handler)); 
    let addr: SocketAddr = cli.listen.parse()?; 
    tracing::info!(%addr, "execution api listening"); 
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?; 
    Ok(()) 
}

async fn signal_handler() -> Json<Signal> { let start=Instant::now(); let prices = [4420.0,4422.0,4419.5,4425.0,4424.0]; let rsi = 55.0; let ema = prices.iter().sum::<f64>()/prices.len() as f64; let risk_allowance = 150000.0 * 0.01; tokio::time::sleep(Duration::from_millis(5)).await; Json(Signal{ symbol:"ES".into(), rsi, ema, risk_allowance, latency_ms:start.elapsed().as_millis() }) }

async fn health_handler() -> Json<Health> {
    Json(Health {
        service: "fks-execution".to_string(),
        status: "healthy".to_string(),
    })
}
