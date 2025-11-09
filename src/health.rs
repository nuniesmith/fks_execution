/// Health check endpoints for FKS services
use axum::{response::Json, routing::get, Router};
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
