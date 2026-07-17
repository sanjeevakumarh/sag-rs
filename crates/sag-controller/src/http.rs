//! The controller's LAN surface: `axum` + JSON, curl-debuggable (ARCHITECTURE.md
//! "Recommended stack"). Three routes for this increment:
//!
//! - `GET  /status` — `{protocol_version, start_epoch}`; the discovery +
//!   leader-election probe.
//! - `GET  /nodes` — the current registry.
//! - `POST /register` — a node announcing itself (upsert).
//! - `POST /hello` — fan the prompt out to every registered node, aggregate.
//!
//! Handlers are plain async fns over [`AppState`], so they are unit-tested by
//! calling them directly with constructed extractors — no bound port, no tower.

use std::sync::Arc;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use sag_proto::{
    HelloRequest, HelloResponse, NodeDescriptor, NodeHello, NodesResponse, RegisterRequest,
    RegisterResponse, StatusResponse, PROTOCOL_VERSION,
};
use tokio::task::JoinSet;

use crate::registry::NodeRegistry;

/// Shared state for the controller's HTTP handlers.
#[derive(Clone)]
pub struct AppState {
    pub registry: Arc<dyn NodeRegistry>,
    /// Unix seconds this controller started — reported on `/status` for election.
    pub start_epoch: u64,
    /// Client for fanning `/hello` out to nodes (connection-pooled, cheap to clone).
    pub http: reqwest::Client,
}

/// Build the controller router over the given state.
pub fn router(state: AppState) -> Router {
    Router::new()
        .route("/status", get(status))
        .route("/nodes", get(list_nodes))
        .route("/register", post(register))
        .route("/hello", post(hello))
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

async fn hello(
    State(state): State<AppState>,
    Json(req): Json<HelloRequest>,
) -> Result<Json<HelloResponse>, StatusCode> {
    let nodes = state.registry.list().await.map_err(|err| {
        tracing::error!(%err, "listing nodes for hello failed");
        StatusCode::INTERNAL_SERVER_ERROR
    })?;

    // Fan out concurrently: one node's slow model must not stall the others.
    let mut set = JoinSet::new();
    for node in nodes {
        let http = state.http.clone();
        let req = req.clone();
        set.spawn(async move { call_node(http, node, req).await });
    }

    let mut replies = Vec::new();
    while let Some(joined) = set.join_next().await {
        match joined {
            Ok(mut node_replies) => replies.append(&mut node_replies),
            Err(err) => tracing::error!(%err, "hello fan-out task panicked"),
        }
    }
    // Stable order so output and tests are deterministic despite concurrency.
    replies.sort_by(|a, b| (&a.node_id, &a.model).cmp(&(&b.node_id, &b.model)));

    Ok(Json(HelloResponse {
        protocol_version: PROTOCOL_VERSION,
        replies,
    }))
}

/// Call one node's `/hello`, returning its per-model replies — or, on any failure,
/// an error entry per model so the caller still sees the node in the aggregate.
async fn call_node(
    http: reqwest::Client,
    node: NodeDescriptor,
    req: HelloRequest,
) -> Vec<NodeHello> {
    let url = format!("{}/hello", node.addr.trim_end_matches('/'));
    let result = http
        .post(&url)
        .json(&req)
        .send()
        .await
        .and_then(|r| r.error_for_status());
    match result {
        Ok(resp) => match resp.json::<HelloResponse>().await {
            Ok(hello) => hello.replies,
            Err(err) => error_replies(&node, &err.to_string()),
        },
        Err(err) => error_replies(&node, &err.to_string()),
    }
}

/// One error entry per model the node advertised (or a single `?` if it declared
/// none), so a node that failed to answer still shows up in the table.
fn error_replies(node: &NodeDescriptor, err: &str) -> Vec<NodeHello> {
    let models: Vec<String> = if node.models.is_empty() {
        vec!["?".to_string()]
    } else {
        node.models.iter().map(|m| m.id.clone()).collect()
    };
    models
        .into_iter()
        .map(|model| NodeHello {
            node_id: node.node_id.clone(),
            model,
            reply: None,
            error: Some(err.to_string()),
            latency_ms: 0,
        })
        .collect()
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
            http: reqwest::Client::new(),
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
