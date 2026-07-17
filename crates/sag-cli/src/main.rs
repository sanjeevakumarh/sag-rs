//! `sag` binary — the operator's entrypoint. Parses a subcommand with `clap` and
//! dispatches it to a controller over JSON. This increment ships `sag nodes` (list
//! the registry) and `sag doctor` (probe the controller); more subcommands and the
//! `attach` TUI arrive later.

use anyhow::Context;
use clap::{Parser, Subcommand};
use sag_cli::{discover_controller, Command, CommandDispatch, HttpDispatch};

#[derive(Debug, Parser)]
#[command(name = "sag", version, about = "SAG-RS control CLI")]
struct Cli {
    /// A controller candidate URL. Repeatable — the client probes this peer list
    /// and talks to whichever controller it elects. Defaults to `--controller`.
    #[arg(long = "peer")]
    peers: Vec<String>,

    /// Convenience single-controller URL, used when no `--peer` is given.
    #[arg(long, env = "SAG_CONTROLLER", default_value = "http://127.0.0.1:7000")]
    controller: String,

    #[command(subcommand)]
    command: Cmd,
}

#[derive(Debug, Subcommand)]
enum Cmd {
    /// List the nodes registered with the controller.
    Nodes,
    /// Greet every registered model and print each reply.
    Hello {
        /// Message to send. Defaults to a friendly greeting.
        #[arg(long, short)]
        message: Option<String>,
    },
    /// Check that the controller is reachable.
    Doctor,
}

impl From<Cmd> for Command {
    fn from(cmd: Cmd) -> Self {
        match cmd {
            Cmd::Nodes => Command::Nodes,
            Cmd::Hello { message } => Command::Hello { message },
            Cmd::Doctor => Command::Doctor,
        }
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    let peers: Vec<String> = if cli.peers.is_empty() {
        vec![cli.controller.clone()]
    } else {
        cli.peers.clone()
    };

    let http = reqwest::Client::new();
    let controller = discover_controller(&http, &peers)
        .await
        .with_context(|| format!("no controller reachable among peers: {peers:?}"))?;

    let dispatch = HttpDispatch::new(controller);
    let output = dispatch
        .dispatch(cli.command.into())
        .await
        .context("dispatching command")?;
    println!("{output}");
    Ok(())
}
