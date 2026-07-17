//! `sag-controller` binary — the brain's LAN entrypoint. Opens the durable node
//! registry (SQLite, auto-migrated) and serves the `axum` + JSON surface so nodes
//! can register and clients can list them.

use std::net::SocketAddr;
use std::sync::Arc;

use anyhow::Context;
use clap::Parser;
use sag_controller::http::{router, AppState};
use sag_controller::{now_epoch, registry::SqliteRegistry};

/// Run the SAG controller.
#[derive(Debug, Parser)]
#[command(name = "sag-controller", version)]
struct Args {
    /// Address to listen on (host:port).
    #[arg(long, env = "SAG_CONTROLLER_LISTEN", default_value = "0.0.0.0:7000")]
    listen: SocketAddr,

    /// SQLite connection string for the durable registry.
    #[arg(
        long,
        env = "SAG_CONTROLLER_DB",
        default_value = "sqlite://sag-controller.db"
    )]
    db: String,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let args = Args::parse();

    let registry = SqliteRegistry::connect(&args.db)
        .await
        .with_context(|| format!("opening registry at {}", args.db))?;
    let state = AppState {
        registry: Arc::new(registry),
        start_epoch: now_epoch(),
        http: reqwest::Client::new(),
    };

    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .with_context(|| format!("binding {}", args.listen))?;
    tracing::info!(listen = %args.listen, db = %args.db, "sag-controller serving");

    axum::serve(listener, router(state))
        .await
        .context("controller server error")?;
    Ok(())
}
