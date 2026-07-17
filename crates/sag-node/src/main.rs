//! `sag-node` binary — starts this box's node, then joins the control plane:
//!
//! 1. Serve the node's own surface (`/health`, `/hello`).
//! 2. **Discover** a controller by probing the known peer list and electing a
//!    leader (`min` by `(start_epoch, latency)`).
//! 3. If none answers and this box is eligible, **self-promote**: start a
//!    controller in-process and register locally (the co-located case).
//! 4. **Heartbeat**: periodically re-discover and re-register. A self-promoted
//!    controller that later finds an older leader **steps down** and follows it.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Context;
use clap::{Parser, Subcommand, ValueEnum};
use sag_controller::http::AppState;
use sag_controller::now_epoch;
use sag_controller::registry::MemoryRegistry;
use sag_inference::{FakeEndpoint, InferenceEndpoint, OpenAiCompatEndpoint};
use sag_node::bootstrap::{detect, SystemProbe};
use sag_node::describe;
use sag_node::http::{router, NodeState};
use sag_node::locate::{discover_leader, remote_outranks_self};
use sag_node::register::ControllerClient;
use sag_proto::NodeDescriptor;
use tokio::sync::oneshot;

#[derive(Debug, Parser)]
#[command(name = "sag-node", version)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Detect this box (OS + GPU) and print how to serve; then run `sag-node run`.
    Bootstrap,
    /// Run the node agent: serve, discover/elect a controller, register.
    Run(Box<RunArgs>),
}

/// Which inference backend this node serves models through.
#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Backend {
    /// Offline echo endpoint — no GPU, for tests and dry runs.
    Fake,
    /// A real OpenAI-compatible server (Ollama, vLLM, llama.cpp).
    OpenaiCompat,
}

/// Arguments for `sag-node run`.
#[derive(Debug, Parser)]
struct RunArgs {
    /// Stable identity for this node in the registry.
    #[arg(long, env = "SAG_NODE_ID", default_value = "node-local")]
    node_id: String,

    /// A controller candidate URL. Repeatable — this is the known peer list the
    /// node probes to find (or elect) a controller. Defaults to `--controller`.
    #[arg(long = "peer")]
    peers: Vec<String>,

    /// Convenience single-controller URL, used when no `--peer` is given.
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

    /// Do not self-promote to controller when none is found (stay a plain node).
    #[arg(long)]
    disable_self_promote: bool,

    /// Address the in-process controller listens on if this node self-promotes.
    #[arg(long, env = "SAG_CONTROLLER_LISTEN", default_value = "0.0.0.0:7000")]
    controller_listen: SocketAddr,

    /// URL the self-promoted controller advertises (must be in peers' lists).
    #[arg(
        long,
        env = "SAG_CONTROLLER_ADVERTISE",
        default_value = "http://127.0.0.1:7000"
    )]
    controller_advertise: String,

    /// Seconds between heartbeat re-register + re-discovery ticks.
    #[arg(long, default_value_t = 10)]
    heartbeat_secs: u64,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    match Cli::parse().command {
        Command::Bootstrap => {
            run_bootstrap();
            Ok(())
        }
        Command::Run(args) => run_node(*args).await,
    }
}

/// Detect the box and print a plan. Side-effect-free, so it is safe to re-run.
fn run_bootstrap() {
    let detection = detect(std::env::consts::OS, &SystemProbe);
    println!("sag-node bootstrap");
    println!("  os:               {}", detection.os);
    println!("  gpu:              {}", detection.gpu.as_str());
    println!("  serving engine:   {}", detection.engine);
    println!();
    println!("Next: point a model server (the engine above) at an OpenAI /v1 port,");
    println!("then start the node — it auto-discovers the model catalog:");
    println!();
    println!("  sag-node run --backend openai-compat \\");
    println!("    --base-url http://127.0.0.1:11434 \\");
    println!("    --peer http://<controller-host>:7000");
    println!();
    println!("With no reachable controller on --peer, this box self-promotes to one.");
}

async fn run_node(args: RunArgs) -> anyhow::Result<()> {
    let eligible = !args.disable_self_promote;
    let peers = resolve_peers(&args);
    let advertise_self = args.controller_advertise.trim_end_matches('/').to_string();

    // Resolve the inference backend + model catalog; both land behind the same
    // Arc<dyn InferenceEndpoint>, so everything downstream is identical.
    let (endpoint, models) = build_backend(&args).await;
    tracing::info!(backend = ?args.backend, models = ?models, "node inference backend ready");
    let descriptor = describe(&args.node_id, &args.advertise, &models);

    // Serve the node's own surface in the background.
    let node_state = NodeState {
        node_id: args.node_id.clone(),
        models,
        endpoint,
    };
    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("binding node surface on {}", args.listen))?;
    tracing::info!(listen = %args.listen, "sag-node surface serving");
    tokio::spawn(async move {
        if let Err(err) = axum::serve(listener, router(node_state)).await {
            tracing::error!(%err, "node surface stopped");
        }
    });

    let http = reqwest::Client::new();

    // Locate a controller, or self-promote.
    let mut controller_url;
    let mut promoted: Option<oneshot::Sender<()>> = None;
    let mut my_epoch = 0u64;
    match discover_leader(&http, &peers).await {
        Some(leader) => {
            tracing::info!(controller = %leader.url, start_epoch = leader.status.start_epoch, "controller discovered");
            controller_url = leader.url;
        }
        None if eligible => {
            let (url, shutdown, epoch) = promote(&args, http.clone()).await?;
            tracing::info!(controller = %url, "no controller found; self-promoted");
            controller_url = url;
            promoted = Some(shutdown);
            my_epoch = epoch;
        }
        None => {
            tracing::warn!(?peers, "no controller found and self-promotion disabled");
            controller_url = peers[0].clone();
        }
    }
    register_with_retry(&controller_url, &descriptor, &args.node_id).await;

    // Heartbeat: re-discover + re-register, stepping down if an older leader appears.
    let beat = Duration::from_secs(args.heartbeat_secs.max(1));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {
                tracing::info!("shutting down");
                break;
            }
            _ = tokio::time::sleep(beat) => {}
        }

        match discover_leader(&http, &peers).await {
            Some(leader) => {
                if promoted.is_some() {
                    if remote_outranks_self(&leader, my_epoch, &advertise_self) {
                        tracing::info!(leader = %leader.url, "older controller found; stepping down");
                        if let Some(shutdown) = promoted.take() {
                            let _ = shutdown.send(());
                        }
                        controller_url = leader.url;
                    }
                } else if leader.url != controller_url {
                    tracing::info!(from = %controller_url, to = %leader.url, "controller changed");
                    controller_url = leader.url;
                }
            }
            None if promoted.is_none() && eligible => match promote(&args, http.clone()).await {
                Ok((url, shutdown, epoch)) => {
                    tracing::info!(controller = %url, "controller lost; self-promoted");
                    controller_url = url;
                    promoted = Some(shutdown);
                    my_epoch = epoch;
                }
                Err(err) => tracing::error!(%err, "self-promotion failed"),
            },
            None => {}
        }

        if let Err(err) = ControllerClient::new(&controller_url)
            .register(descriptor.clone())
            .await
        {
            tracing::warn!(%err, controller = %controller_url, "heartbeat re-register failed");
        }
    }

    Ok(())
}

/// The known peer list: explicit `--peer`s, or the single `--controller` fallback.
fn resolve_peers(args: &RunArgs) -> Vec<String> {
    let raw = if args.peers.is_empty() {
        std::slice::from_ref(&args.controller)
    } else {
        args.peers.as_slice()
    };
    raw.iter()
        .map(|p| p.trim_end_matches('/').to_string())
        .collect()
}

/// Build the inference endpoint and resolve the model catalog for the chosen backend.
async fn build_backend(args: &RunArgs) -> (Arc<dyn InferenceEndpoint>, Vec<String>) {
    match args.backend {
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
    }
}

/// Start a controller in-process (the self-promotion path). Returns the URL it
/// advertises, a shutdown handle (to step down later), and its start epoch. Uses an
/// in-memory registry — nodes heartbeat, so it refills; the standalone
/// `sag-controller` binary is the durable one.
async fn promote(
    args: &RunArgs,
    http: reqwest::Client,
) -> anyhow::Result<(String, oneshot::Sender<()>, u64)> {
    let start_epoch = now_epoch();
    let state = AppState {
        registry: Arc::new(MemoryRegistry::new()),
        start_epoch,
        http,
    };
    let listener = tokio::net::TcpListener::bind(args.controller_listen)
        .await
        .with_context(|| format!("binding embedded controller on {}", args.controller_listen))?;
    let (shutdown, rx) = oneshot::channel::<()>();
    tokio::spawn(async move {
        let server = axum::serve(listener, sag_controller::http::router(state))
            .with_graceful_shutdown(async move {
                let _ = rx.await;
            });
        if let Err(err) = server.await {
            tracing::error!(%err, "embedded controller stopped");
        }
    });
    let advertise = args.controller_advertise.trim_end_matches('/').to_string();
    Ok((advertise, shutdown, start_epoch))
}

/// Register a few times with a fixed delay so node/controller start order doesn't
/// matter for the first registration.
async fn register_with_retry(controller_url: &str, descriptor: &NodeDescriptor, node_id: &str) {
    const ATTEMPTS: u32 = 5;
    let client = ControllerClient::new(controller_url);
    for attempt in 1..=ATTEMPTS {
        match client.register(descriptor.clone()).await {
            Ok(_) => {
                tracing::info!(node = %node_id, controller = %controller_url, "registered");
                return;
            }
            Err(err) if attempt < ATTEMPTS => {
                tracing::warn!(%err, attempt, "registration failed; retrying");
                tokio::time::sleep(Duration::from_secs(1)).await;
            }
            Err(err) => {
                tracing::error!(%err, "registration failed after {ATTEMPTS} attempts; serving anyway");
            }
        }
    }
}
