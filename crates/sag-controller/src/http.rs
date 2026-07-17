//! The controller's LAN surface: `axum` + JSON, curl-debuggable (ARCHITECTURE.md
//! "Recommended stack"). Three routes for this increment:
//!
//! - `GET  /status` — `{protocol_version, start_epoch}`; the discovery +
//!   leader-election probe.
//! - `GET  /nodes` — the current registry.
//! - `POST /register` — a node announcing itself (upsert).
//!
//! Handlers are plain async fns over [`AppState`], so they are unit-tested by
//! calling them directly with constructed extractors — no bound port, no tower.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use sag_proto::{
    NodesResponse, RegisterRequest, RegisterResponse, StatusResponse, PROTOCOL_VERSION,
};

use crate::registry::NodeRegistry;

/// Shared state for the controller's HTTP handlers.
#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<dyn NodeRegistry>,
    /// Unix seconds this controller started — reported on `/status` for election.
    pub start_epoch: u64,
}

/// Build the controller router over the given state.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/nodes", get(list_nodes))
        .route("/register", post(register))
        .with_state(state)
}

async fn status(State(state): State<AppState>) -> Json<StatusResponse> {
    Json(StatusResponse {
        protocol_version: PROTOCOL_VERSION,
        start_epoch: state.start_epoch,
    })
}

async fn list_nodes(State(state): State<AppState>) -> Result<Json<NodesResponse>, StatusCode> {
    let nodes = state.registry.list().await.map_err(|err| {
        tracing::error!(%err, "listing nodes failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    Ok(Json(NodesResponse {
        protocol_version: PROTOCOL_VERSION,
        nodes,
    }))
}

async fn register(
    State(state): State<AppState>,
    Json(req): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, StatusCode> {
    // Forward-compat: a version mismatch is a warning, not a rejection — the wire
    // types tolerate unknown/missing fields, so peers a version apart still interop.
    if req.protocol_version != PROTOCOL_VERSION {
        tracing::warn!(
            theirs = req.protocol_version,
            ours = PROTOCOL_VERSION,
            node = %req.node.node_id,
            "protocol version drift on register; accepting"
        );
    }
    let node_id = req.node.node_id.clone();
    state.registry.upsert(req.node).await.map_err(|err| {
        tracing::error!(%err, node = %node_id, "registration upsert failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;
    tracing::info!(node = %node_id, "node registered");
    Ok(Json(RegisterResponse {
        protocol_version: PROTOCOL_VERSION,
        accepted: true,
        message: None,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::registry::MemoryRegistry;
    use sag_proto::NodeDescriptor;

    fn state() -> AppState {
        AppState {
            registry: Arc::new(MemoryRegistry::new()),
            start_epoch: 1_700_000_000,
        }
    }

    fn register_req(node_id: &str) -> RegisterRequest {
        RegisterRequest {
            protocol_version: PROTOCOL_VERSION,
            node: NodeDescriptor {
                node_id: node_id.into(),
                addr: format!("http://{node_id}:8080"),
                models: vec![],
                capabilities: vec![],
            },
        }
    }

    #[tokio::test]
    async fn register_then_list_round_trips_through_handlers() {
        let state = state();
        let resp = register(State(state.clone()), Json(register_req("gmkmini")))
            .await
            .expect("register ok");
        assert!(resp.0.accepted);

        let listed = list_nodes(State(state)).await.expect("list ok");
        assert_eq!(listed.0.nodes.len(), 1);
        assert_eq!(listed.0.nodes[0].node_id, "gmkmini");
        assert_eq!(listed.0.protocol_version, PROTOCOL_VERSION);
    }

    #[tokio::test]
    async fn status_reports_start_epoch() {
        let resp = status(State(state())).await;
        assert_eq!(resp.0.start_epoch, 1_700_000_000);
        assert_eq!(resp.0.protocol_version, PROTOCOL_VERSION);
    }

    #[tokio::test]
    async fn register_accepts_across_protocol_drift() {
        let state = state();
        let mut req = register_req("hptowerz");
        req.protocol_version = PROTOCOL_VERSION + 1; // a newer node
        let resp = register(State(state.clone()), Json(req))
            .await
            .expect("drift still accepted");
        assert!(resp.0.accepted);
        assert_eq!(list_nodes(State(state)).await.unwrap().0.nodes.len(), 1);
    }
}
