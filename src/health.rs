/// Health check endpoints for FKS services
use axum::{response::{Json, IntoResponse}, routing::get, Router, http::StatusCode};
use serde_json::{json, Value};
use std::time::{SystemTime, UNIX_EPOCH};

pub fn health_routes<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/health", get(health_check))
        .route("/ready", get(readiness_check))
        .route("/live", get(liveness_check))
        .route("/metrics", get(metrics))
}

async fn health_check() -> Json<Value> {
    Json(json!({
        "status": "healthy",
        "service": "fks_execution",
        "timestamp": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }))
}

async fn readiness_check() -> Json<Value> {
    // TODO: Add dependency checks
    Json(json!({
        "status": "ready",
        "service": "fks_execution",
        "timestamp": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs(),
        "dependencies": {}
    }))
}

async fn liveness_check() -> Json<Value> {
    Json(json!({
        "status": "alive",
        "service": "fks_execution",
        "timestamp": SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }))
}

async fn metrics() -> impl IntoResponse {
    // Return Prometheus-format metrics with fks_build_info
    let version = env!("CARGO_PKG_VERSION");
    let metrics_text = format!(
        r#"# HELP fks_build_info Build information for the service
# TYPE fks_build_info gauge
fks_build_info{{service="fks_execution",version="{}"}} 1
"#,
        version
    );
    (StatusCode::OK, [("content-type", "text/plain; version=0.0.4; charset=utf-8")], metrics_text)
}
