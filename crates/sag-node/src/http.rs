//! The node's own LAN surface. This increment serves only `GET /health` so the
//! advertised address is real and reachable; the `/hello` inference route the
//! controller fans out to lands in the next increment.

use axum::http::StatusCode;
use axum::routing::get;
use axum::Router;

/// Build the node's router.
pub fn router() -> Router {
    Router::new().route("/health", get(|| async { StatusCode::OK }))
}
