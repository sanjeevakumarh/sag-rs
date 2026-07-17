//! `sag-cli` — "how I talk to it".
//!
//! Swap seam: [`CommandDispatch`]. The CLI parses a [`Command`] and dispatches it
//! (to the controller), returning a line for the terminal. Keeping dispatch behind
//! a trait lets the whole CLI be driven in tests without a TTY or a live
//! controller. [`FakeCli`] is the offline impl for tests; [`HttpDispatch`] is the
//! real one that talks to a controller over JSON. Parsing is `clap` (derive) in
//! `main.rs`; the `attach` TUI (`ratatui`) arrives later.

use async_trait::async_trait;
use sag_proto::{
    HelloRequest, HelloResponse, NodeDescriptor, NodeHello, NodesResponse, StatusResponse,
    PROTOCOL_VERSION,
};

/// A parsed command the CLI can dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Command {
    /// `sag nodes` — list the nodes registered with the controller.
    Nodes,
    /// `sag hello [--message ...]` — greet every registered model, show replies.
    Hello { message: Option<String> },
    /// `sag run fix "<request>"`
    RunFix { request: String },
    /// `sag doctor`
    Doctor,
}

#[derive(Debug, thiserror::Error)]
pub enum CliError {
    #[error("empty request")]
    EmptyRequest,
    #[error("controller error: {0}")]
    Controller(String),
    #[error("{0}")]
    NotWired(String),
}

/// The swap seam: dispatch a parsed command and produce terminal output.
#[async_trait]
pub trait CommandDispatch: Send + Sync {
    async fn dispatch(&self, cmd: Command) -> Result<String, CliError>;
}

/// Offline dispatcher for tests: renders each command to a fixed line without a
/// controller or TTY, so command parsing and routing can be tested headless.
pub struct FakeCli;

#[async_trait]
impl CommandDispatch for FakeCli {
    async fn dispatch(&self, cmd: Command) -> Result<String, CliError> {
        Ok(match cmd {
            Command::Nodes | Command::Hello { .. } => "no nodes registered".into(),
            Command::RunFix { request } if request.trim().is_empty() => {
                return Err(CliError::EmptyRequest)
            }
            Command::RunFix { request } => format!("submitted fix: {request}"),
            Command::Doctor => "all systems nominal".into(),
        })
    }
}

/// Real dispatcher: talks to a controller over JSON. The controller URL is a fixed
/// endpoint for now; peer-list discovery + leader election replace it in a later
/// increment (the client will pick whichever controller it can reach).
pub struct HttpDispatch {
    controller_url: String,
    http: reqwest::Client,
}

impl HttpDispatch {
    pub fn new(controller_url: impl Into<String>) -> Self {
        Self {
            controller_url: controller_url.into().trim_end_matches('/').to_string(),
            http: reqwest::Client::new(),
        }
    }

    async fn get_json<T: serde::de::DeserializeOwned>(&self, path: &str) -> Result<T, CliError> {
        let url = format!("{}{path}", self.controller_url);
        self.http
            .get(&url)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| CliError::Controller(format!("{url}: {e}")))?
            .json()
            .await
            .map_err(|e| CliError::Controller(format!("{url}: {e}")))
    }

    async fn post_json<B: serde::Serialize, T: serde::de::DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, CliError> {
        let url = format!("{}{path}", self.controller_url);
        self.http
            .post(&url)
            .json(body)
            .send()
            .await
            .and_then(|r| r.error_for_status())
            .map_err(|e| CliError::Controller(format!("{url}: {e}")))?
            .json()
            .await
            .map_err(|e| CliError::Controller(format!("{url}: {e}")))
    }
}

#[async_trait]
impl CommandDispatch for HttpDispatch {
    async fn dispatch(&self, cmd: Command) -> Result<String, CliError> {
        match cmd {
            Command::Nodes => {
                let resp: NodesResponse = self.get_json("/nodes").await?;
                Ok(format_nodes(&resp.nodes))
            }
            Command::Hello { message } => {
                let req = HelloRequest {
                    protocol_version: PROTOCOL_VERSION,
                    message,
                };
                let resp: HelloResponse = self.post_json("/hello", &req).await?;
                Ok(format_hello(&resp.replies))
            }
            Command::Doctor => {
                let resp: StatusResponse = self.get_json("/status").await?;
                Ok(format!(
                    "controller reachable at {} — protocol v{}, up since epoch {}",
                    self.controller_url, resp.protocol_version, resp.start_epoch
                ))
            }
            Command::RunFix { .. } => Err(CliError::NotWired(
                "`run fix` is not wired to the controller yet (later milestone)".into(),
            )),
        }
    }
}

/// Render the registry as an aligned text table for the terminal.
pub fn format_nodes(nodes: &[NodeDescriptor]) -> String {
    if nodes.is_empty() {
        return "no nodes registered".to_string();
    }
    let mut out = format!("{:<16} {:<30} {}\n", "NODE", "ADDR", "MODELS");
    for n in nodes {
        let models = if n.models.is_empty() {
            "-".to_string()
        } else {
            n.models
                .iter()
                .map(|m| m.id.clone())
                .collect::<Vec<_>>()
                .join(", ")
        };
        out.push_str(&format!("{:<16} {:<30} {}\n", n.node_id, n.addr, models));
    }
    out.trim_end().to_string()
}

/// Render an aggregated hello as a table: one row per `(node, model)`.
pub fn format_hello(replies: &[NodeHello]) -> String {
    if replies.is_empty() {
        return "no nodes registered".to_string();
    }
    let mut out = format!("{:<12} {:<18} {:>6}  {}\n", "NODE", "MODEL", "MS", "REPLY");
    for r in replies {
        let text = match (&r.reply, &r.error) {
            (Some(reply), _) => truncate(&reply.replace('\n', " "), 120),
            (None, Some(err)) => format!("ERROR: {}", truncate(err, 110)),
            (None, None) => "(no reply)".to_string(),
        };
        out.push_str(&format!(
            "{:<12} {:<18} {:>6}  {}\n",
            r.node_id, r.model, r.latency_ms, text
        ));
    }
    out.trim_end().to_string()
}

/// Clip `s` to at most `max` characters, appending `…` when truncated.
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        return s.to_string();
    }
    let clipped: String = s.chars().take(max.saturating_sub(1)).collect();
    format!("{clipped}…")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn dispatches_run_fix_and_doctor() {
        assert_eq!(
            FakeCli
                .dispatch(Command::RunFix {
                    request: "fix cache expiry".into()
                })
                .await
                .unwrap(),
            "submitted fix: fix cache expiry"
        );
        assert_eq!(
            FakeCli.dispatch(Command::Doctor).await.unwrap(),
            "all systems nominal"
        );
    }

    #[tokio::test]
    async fn empty_run_fix_request_is_an_error() {
        assert!(matches!(
            FakeCli
                .dispatch(Command::RunFix { request: "".into() })
                .await,
            Err(CliError::EmptyRequest)
        ));
    }

    #[test]
    fn format_nodes_renders_table_and_empty() {
        assert_eq!(format_nodes(&[]), "no nodes registered");

        let nodes = vec![NodeDescriptor {
            node_id: "gmkmini".into(),
            addr: "http://gmkmini:8080".into(),
            models: vec![
                sag_proto::ModelDescriptor {
                    id: "qwen3-coder:30b".into(),
                    capabilities: vec![],
                },
                sag_proto::ModelDescriptor {
                    id: "gpt-oss:20b".into(),
                    capabilities: vec![],
                },
            ],
            capabilities: vec![],
        }];
        let table = format_nodes(&nodes);
        assert!(table.contains("gmkmini"));
        assert!(table.contains("http://gmkmini:8080"));
        assert!(table.contains("qwen3-coder:30b, gpt-oss:20b"));
    }

    #[test]
    fn format_hello_renders_replies_and_errors() {
        assert_eq!(format_hello(&[]), "no nodes registered");

        let replies = vec![
            NodeHello {
                node_id: "gmkmini".into(),
                model: "qwen3-coder:30b".into(),
                reply: Some("Hello!".into()),
                error: None,
                latency_ms: 42,
            },
            NodeHello {
                node_id: "hptowerz".into(),
                model: "gpt-oss:20b".into(),
                reply: None,
                error: Some("connection refused".into()),
                latency_ms: 0,
            },
        ];
        let table = format_hello(&replies);
        assert!(table.contains("gmkmini"));
        assert!(table.contains("Hello!"));
        assert!(table.contains("ERROR: connection refused"));
    }
}
