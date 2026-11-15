//! The health check the local stack and the load balancer probe.

use axum::{Router, routing::get};

/// Health check.
#[utoipa::path(
    get,
    path = "/health",
    responses(
        (status = 200, description = "health", body = String),
    )
)]
pub async fn health_handler() -> &'static str {
    "healthy"
}

/// The health route.
pub fn router() -> Router {
    Router::new().route("/health", get(health_handler))
}
