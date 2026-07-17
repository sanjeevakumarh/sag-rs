//! The node's own LAN surface: `GET /health` and `POST /hello`. The controller
//! fans `/hello` out to every node; a node runs the prompt against each model it
//! serves via its [`InferenceEndpoint`] and returns one entry per model.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::State;
use axum::http::StatusCode;
use axum::routing::{get, post};
use axum::{Json, Router};
use sag_inference::{GenerateRequest, InferenceEndpoint};
use sag_proto::{HelloRequest, HelloResponse, NodeHello, PROTOCOL_VERSION};

/// Default greeting when the request carries no message.
const DEFAULT_MESSAGE: &str = "hello world";

/// State for the node's HTTP handlers: its identity, the models it serves, and the
/// endpoint those models are reached through (a `Fake` echo for now; a real
/// OpenAI-compatible client in the next increment — same trait either way).
#[derive(Clone)]
pub struct NodeState {
    pub node_id: String,
    pub models: Vec<String>,
    pub endpoint: Arc<dyn InferenceEndpoint>,
}

/// Build the node's router over its state.
pub fn router(state: NodeState) -> Router {
    Router::new()
        .route("/health", get(|| async { StatusCode::OK }))
        .route("/hello", post(hello))
        .with_state(state)
}

async fn hello(
    State(state): State<NodeState>,
    Json(req): Json<HelloRequest>,
) -> Json<HelloResponse> {
    let message = req.message.unwrap_or_else(|| DEFAULT_MESSAGE.to_string());

    let mut replies = Vec::with_capacity(state.models.len());
    for model in &state.models {
        let started = Instant::now();
        let result = state
            .endpoint
            .generate(GenerateRequest {
                model: model.clone(),
                prompt: message.clone(),
            })
            .await;
        let latency_ms = started.elapsed().as_millis() as u64;
        let (reply, error) = match result {
            Ok(resp) => (Some(resp.text), None),
            Err(err) => (None, Some(err.to_string())),
        };
        replies.push(NodeHello {
            node_id: state.node_id.clone(),
            model: model.clone(),
            reply,
            error,
            latency_ms,
        });
    }

    Json(HelloResponse {
        protocol_version: PROTOCOL_VERSION,
        replies,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use sag_inference::FakeEndpoint;

    #[tokio::test]
    async fn hello_runs_each_model_through_the_endpoint() {
        let state = NodeState {
            node_id: "n1".into(),
            models: vec!["qwen".into(), "gptoss".into()],
            endpoint: Arc::new(FakeEndpoint),
        };
        let resp = hello(
            State(state),
            Json(HelloRequest {
                protocol_version: PROTOCOL_VERSION,
                message: Some("hi".into()),
            }),
        )
        .await;

        assert_eq!(resp.0.replies.len(), 2);
        assert_eq!(resp.0.replies[0].node_id, "n1");
        // FakeEndpoint echoes `[model] prompt`.
        assert_eq!(resp.0.replies[0].reply.as_deref(), Some("[qwen] hi"));
        assert!(resp.0.replies[0].error.is_none());
        assert_eq!(resp.0.replies[1].reply.as_deref(), Some("[gptoss] hi"));
    }
}
