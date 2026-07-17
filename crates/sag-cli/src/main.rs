//! `sag` binary — the operator's entrypoint. Parses a subcommand with `clap` and
//! dispatches it to a controller over JSON. This increment ships `sag nodes` (list
//! the registry) and `sag doctor` (probe the controller); more subcommands and the
//! `attach` TUI arrive later.

use anyhow::Context;
use clap::{Parser, Subcommand};
use sag_cli::{Command, CommandDispatch, HttpDispatch};

#[derive(Debug, Parser)]
#[command(name = "sag", version, about = "SAG-RS control CLI")]
struct Cli {
    /// Controller base URL. Peer-list discovery replaces this default later.
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
    let dispatch = HttpDispatch::new(&cli.controller);
    let output = dispatch
        .dispatch(cli.command.into())
        .await
        .context("dispatching command")?;
    println!("{output}");
    Ok(())
}
