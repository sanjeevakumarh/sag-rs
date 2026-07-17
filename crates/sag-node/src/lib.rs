//! `sag-node` — "what runs on a GPU box".
//!
//! Swap seam: [`NodeAgent`]. A node agent reports a point-in-time [`NodeSnapshot`]
//! (VRAM, queue depth, warm models) that the controller's scheduler selects over,
//! and proxies inference to the local model server. Step 1 defines the seam plus a
//! [`FakeNode`] that reports a fixed snapshot; the binary (`main.rs`) is a thin
//! entrypoint, and the real inventory/proxy wiring arrives with the bootstrap step.

use async_trait::async_trait;
use sag_proto::NodeSnapshot;

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
}
