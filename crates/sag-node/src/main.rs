//! `sag-node` binary — starts this box's node: serves its own surface (`/health`),
//! then registers with a controller so the client can see it. Discovery across a
//! peer list and self-promotion land in a later increment; for now the controller
//! address is a single flag/env, defaulting to localhost (the co-located case).

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use clap::{Parser, ValueEnum};
use sag_inference::{FakeEndpoint, InferenceEndpoint, OpenAiCompatEndpoint};
use sag_node::describe;
use sag_node::http::{router, NodeState};
use sag_node::register::ControllerClient;

/// Which inference backend this node serves models through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Backend {
    /// Offline echo endpoint — no GPU, for tests and dry runs.
    Fake,
    /// A real OpenAI-compatible server (Ollama, vLLM, llama.cpp).
    OpenaiCompat,
}

/// Run a SAG node agent.
#[derive(Debug, Parser)]
#[command(name = "sag-node", version)]
struct Args {
    /// Stable identity for this node in the registry.
    #[arg(long, env = "SAG_NODE_ID", default_value = "node-local")]
    node_id: String,

    /// Controller base URL to register with.
    #[arg(long, env = "SAG_CONTROLLER", default_value = "http://127.0.0.1:7000")]
    controller: String,

    /// Address this node's own surface listens on.
    #[arg(long, env = "SAG_NODE_LISTEN", default_value = "0.0.0.0:8080")]
    listen: SocketAddr,

    /// URL peers should reach this node at (what gets advertised).
    #[arg(
        long,
        env = "SAG_NODE_ADVERTISE",
        default_value = "http://127.0.0.1:8080"
    )]
    advertise: String,

    /// Inference backend: `fake` (echo) or `openai-compat` (a real `/v1` server).
    #[arg(long, value_enum, env = "SAG_NODE_BACKEND", default_value = "fake")]
    backend: Backend,

    /// Base URL of the OpenAI-compatible server (used when backend = openai-compat).
    #[arg(
        long,
        env = "SAG_NODE_BASE_URL",
        default_value = "http://127.0.0.1:11434"
    )]
    base_url: String,

    /// A model id this node serves. Repeatable. Defaults: a single fake model for
    /// the fake backend; auto-discovery via `/v1/models` for openai-compat.
    #[arg(long = "model")]
    models: Vec<String>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();

    // Pick the backend and resolve the model catalog. Both paths land behind the
    // same Arc<dyn InferenceEndpoint>, so everything downstream is identical.
    let (endpoint, models): (Arc<dyn InferenceEndpoint>, Vec<String>) = match args.backend {
        Backend::Fake => {
            let models = if args.models.is_empty() {
                vec!["fake-echo".to_string()]
            } else {
                args.models.clone()
            };
            (Arc::new(FakeEndpoint), models)
        }
        Backend::OpenaiCompat => {
            let endpoint = OpenAiCompatEndpoint::new(&args.base_url);
            let models = if !args.models.is_empty() {
                args.models.clone()
            } else {
                match endpoint.list_models().await {
                    Ok(models) if !models.is_empty() => models,
                    Ok(_) => {
                        tracing::warn!(base_url = %args.base_url, "model server advertised no models");
                        Vec::new()
                    }
                    Err(err) => {
                        tracing::warn!(%err, base_url = %args.base_url, "model discovery failed; advertising none");
                        Vec::new()
                    }
                }
            };
            (Arc::new(endpoint), models)
        }
    };
    tracing::info!(backend = ?args.backend, models = ?models, "node inference backend ready");

    let descriptor = describe(&args.node_id, &args.advertise, &models);
    let node_state = NodeState {
        node_id: args.node_id.clone(),
        models,
        endpoint,
    };

    // Serve the node's own surface in the background.
    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("binding node surface on {}", args.listen))?;
    tracing::info!(listen = %args.listen, "sag-node surface serving");
    tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, router(node_state)).await {
            tracing::error!(%err, "node surface stopped");
        }
    });

    // Register, retrying briefly so node/controller startup order doesn't matter.
    let client = ControllerClient::new(&args.controller);
    register_with_retry(&client, descriptor, &args.node_id).await;

    tracing::info!("node running; Ctrl-C to stop");
    tokio::signal::ctrl_c()
        .await
        .context("waiting for shutdown signal")?;
    Ok(())
}

/// Try to register a handful of times with a fixed delay, so a node started just
/// before its controller still lands. Discovery/self-promotion (increment 4)
/// replaces this with peer-list probing + leader election.
async fn register_with_retry(
    client: &ControllerClient,
    descriptor: sag_proto::NodeDescriptor,
    node_id: &str,
) {
    const ATTEMPTS: u32 = 5;
    for attempt in 1..=ATTEMPTS {
        match client.register(descriptor.clone()).await {
            Ok(_) => {
                tracing::info!(node = %node_id, controller = %client.base_url(), "registered");
                return;
            }
            Err(err) if attempt < ATTEMPTS => {
                tracing::warn!(%err, attempt, "registration failed; retrying");
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            Err(err) => {
                tracing::error!(%err, "registration failed after {ATTEMPTS} attempts; serving anyway");
            }
        }
    }
}
