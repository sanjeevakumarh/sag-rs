//! `sag-node` — "what runs on a GPU box".
//!
//! Swap seam: [`NodeAgent`]. A node agent reports a point-in-time [`NodeSnapshot`]
//! (VRAM, queue depth, warm models) that the controller's scheduler selects over,
//! and proxies inference to the local model server. The [`FakeNode`] reports a
//! fixed snapshot for tests.
//!
//! The end-to-end `sag hello` milestone adds the networked node: [`register`] is
//! the client side of the registration wire (announce to a controller, probe
//! `/status`), [`http`] the node's own surface, and [`describe`] builds the
//! [`NodeDescriptor`] this node advertises.

pub mod http;
pub mod locate;
pub mod register;

use async_trait::async_trait;
use sag_proto::{ModelDescriptor, NodeDescriptor, NodeSnapshot};

/// Build the descriptor this node advertises to a controller: its id, the address
/// peers reach it at, and the models it serves.
pub fn describe(node_id: &str, addr: &str, model_ids: &[String]) -> NodeDescriptor {
    NodeDescriptor {
        node_id: node_id.to_string(),
        addr: addr.to_string(),
        models: model_ids
            .iter()
            .map(|id| ModelDescriptor {
                id: id.clone(),
                capabilities: Vec::new(),
            })
            .collect(),
        capabilities: vec!["serve".to_string()],
    }
}

#[derive(Debug, thiserror::Error)]
pub enum NodeError {
    #[error("node unavailable")]
    Unavailable,
}

/// The swap seam: a handle to one GPU box that reports its current state.
#[async_trait]
pub trait NodeAgent: Send + Sync {
    async fn snapshot(&self) -> Result<NodeSnapshot, NodeError>;
}

/// Offline node for tests: reports a fixed snapshot with no GPU probe and no
/// network, so the scheduler and controller can be driven against known inventory.
pub struct FakeNode {
    snapshot: NodeSnapshot,
}

impl FakeNode {
    pub fn new(snapshot: NodeSnapshot) -> Self {
        Self { snapshot }
    }
}

#[async_trait]
impl NodeAgent for FakeNode {
    async fn snapshot(&self) -> Result<NodeSnapshot, NodeError> {
        Ok(self.snapshot.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_node_reports_its_fixed_snapshot() {
        let node = FakeNode::new(NodeSnapshot {
            id: "gmkmini".into(),
            vram_gb: 108,
            queue_depth: 0,
            warm_models: vec!["qwen3-coder:30b".into()],
        });
        let snap = node.snapshot().await.unwrap();
        assert_eq!(snap.id, "gmkmini");
        assert_eq!(snap.vram_gb, 108);
    }

    #[test]
    fn describe_builds_the_advertised_descriptor() {
        let d = describe(
            "gmkmini",
            "http://gmkmini:8080",
            &["qwen3-coder:30b".to_string(), "gpt-oss:20b".to_string()],
        );
        assert_eq!(d.node_id, "gmkmini");
        assert_eq!(d.addr, "http://gmkmini:8080");
        assert_eq!(
            d.models.iter().map(|m| m.id.as_str()).collect::<Vec<_>>(),
            ["qwen3-coder:30b", "gpt-oss:20b"]
        );
    }
}
