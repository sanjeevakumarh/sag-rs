//! `sag-proto` — "shared vocabulary".
//!
//! The one crate with no swap seam: it carries the serde types every other crate
//! speaks (`Task`, `RepoRef`, `Citation`, `NodeSnapshot`, ...). Keeping them here
//! means the engine, scheduler, retrieval, and store agree on a shape without
//! depending on each other. No behavior lives here — only the vocabulary.

use serde::{Deserialize, Serialize};

/// Wire-protocol version, exchanged on every controller/node/client message.
///
/// **Forward-compat rule:** bump this only for *incompatible* changes. Additive
/// fields do not need a bump — every wire struct below tolerates unknown fields
/// (serde ignores them by default; we never use `deny_unknown_fields`) and fills
/// missing ones via `#[serde(default)]`, so a newer peer and an older peer stay
/// interoperable. Receivers warn on a version mismatch rather than hard-failing.
pub const PROTOCOL_VERSION: u32 = 1;

/// One model a node can serve, as advertised to the controller.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModelDescriptor {
    /// Model id as the serving backend names it (e.g. `qwen3-coder:30b`).
    pub id: String,
    /// Capabilities this model advertises (`code.edit`, `reason.deep`, ...).
    /// Additive: unknown capabilities are kept as-is, never rejected.
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// A node advertising itself to the controller at registration time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeDescriptor {
    pub node_id: String,
    /// Base URL other peers reach this node at, e.g. `http://host:8080`.
    pub addr: String,
    #[serde(default)]
    pub models: Vec<ModelDescriptor>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// `POST /register` request body: a node announcing itself.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterRequest {
    pub protocol_version: u32,
    pub node: NodeDescriptor,
}

/// `POST /register` response.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegisterResponse {
    pub protocol_version: u32,
    pub accepted: bool,
    #[serde(default)]
    pub message: Option<String>,
}

/// `GET /nodes` response: the current registry the client renders.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodesResponse {
    pub protocol_version: u32,
    pub nodes: Vec<NodeDescriptor>,
}

/// `GET /status` response — the discovery + leader-election probe. Leadership is
/// `min` by `(start_epoch, latency)`, so this carries the controller's start time.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StatusResponse {
    pub protocol_version: u32,
    /// Unix seconds when this controller process started.
    pub start_epoch: u64,
}

/// A unit of work submitted to the runtime (e.g. `sag run fix`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    /// Natural-language request, e.g. "Fix intermittent cache expiry test".
    pub request: String,
}

/// A reference to a repository a task operates over.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepoRef {
    pub path: String,
}

/// Compact evidence the scout returns instead of whole files: a path plus an
/// inclusive line range and why it matters. See ARCHITECTURE.md "FastContext scout".
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Citation {
    pub path: String,
    /// Inclusive `(start, end)` line range, 1-based.
    pub lines: (u32, u32),
    pub reason: String,
}

/// A point-in-time view of a GPU node the scheduler selects over. Deliberately a
/// plain value type: the scheduler is tested against hand-built `Vec<NodeSnapshot>`
/// with no live cluster.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NodeSnapshot {
    pub id: String,
    pub vram_gb: u32,
    /// Requests currently queued or in flight on this node.
    pub queue_depth: u32,
    /// Models already resident (warm) on this node — feeds warm-model preference.
    pub warm_models: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn citation_round_trips_through_json() {
        let c = Citation {
            path: "src/cache.rs".into(),
            lines: (42, 58),
            reason: "expiry timer reset here".into(),
        };
        let json = serde_json::to_string(&c).expect("serialize");
        let back: Citation = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(c, back);
    }

    #[test]
    fn register_request_round_trips() {
        let req = RegisterRequest {
            protocol_version: PROTOCOL_VERSION,
            node: NodeDescriptor {
                node_id: "gmkmini".into(),
                addr: "http://gmkmini:8080".into(),
                models: vec![ModelDescriptor {
                    id: "qwen3-coder:30b".into(),
                    capabilities: vec!["code.edit".into()],
                }],
                capabilities: vec!["serve".into()],
            },
        };
        let json = serde_json::to_string(&req).expect("serialize");
        let back: RegisterRequest = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(req, back);
    }

    /// Forward-compat: a payload from a *newer* peer carrying fields we don't know
    /// yet must still deserialize (unknown fields ignored), and fields a newer peer
    /// omitted must fill from `#[serde(default)]` — old and new binaries interop.
    #[test]
    fn descriptor_tolerates_unknown_fields_and_missing_defaults() {
        let from_newer_peer = r#"{
            "node_id": "hptowerz",
            "addr": "http://hptowerz:8080",
            "gpu_vendor": "amd",
            "models": [{ "id": "gpt-oss:20b", "future_flag": true }]
        }"#;
        let node: NodeDescriptor =
            serde_json::from_str(from_newer_peer).expect("unknown fields must be ignored");
        assert_eq!(node.node_id, "hptowerz");
        // `capabilities` was absent → default empty, not an error.
        assert!(node.capabilities.is_empty());
        // The model's unknown `future_flag` was ignored; its `capabilities` defaulted.
        assert_eq!(node.models[0].id, "gpt-oss:20b");
        assert!(node.models[0].capabilities.is_empty());
    }
}
