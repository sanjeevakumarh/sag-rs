//! `sag-scheduler` — "pick which node".
//!
//! Swap seam: [`Scheduler`]. Two stages (ARCHITECTURE.md "The scheduler"): hard
//! eligibility rejects nodes that can't satisfy the request, then weighted
//! selection ranks the survivors. Selection is pure and synchronous — it runs over
//! a `&[NodeSnapshot]`, so it tests in milliseconds against hand-built snapshots
//! with no live cluster. Step 4 builds the real weighted scorer; step 1 gives the
//! seam plus a first-eligible [`FakeScheduler`].

use sag_proto::NodeSnapshot;

/// What a task needs from a node — the input to hard eligibility.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ScheduleRequest {
    /// Model the task requires resident/servable, if any.
    pub required_model: Option<String>,
    /// Minimum free VRAM, in GB.
    pub min_vram_gb: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum ScheduleError {
    #[error("no eligible node for request")]
    NoEligibleNode,
}

/// The swap seam: choose a node id for a request, or reject.
pub trait Scheduler: Send + Sync {
    fn select(
        &self,
        nodes: &[NodeSnapshot],
        req: &ScheduleRequest,
    ) -> Result<String, ScheduleError>;
}

/// Trivial baseline: apply hard eligibility, then take the first survivor (no
/// weighting). Enough to exercise the seam and to debug routing deterministically.
pub struct FakeScheduler;

impl FakeScheduler {
    fn eligible(node: &NodeSnapshot, req: &ScheduleRequest) -> bool {
        if node.vram_gb < req.min_vram_gb {
            return false;
        }
        match &req.required_model {
            Some(m) => node.warm_models.iter().any(|w| w == m),
            None => true,
        }
    }
}

impl Scheduler for FakeScheduler {
    fn select(
        &self,
        nodes: &[NodeSnapshot],
        req: &ScheduleRequest,
    ) -> Result<String, ScheduleError> {
        nodes
            .iter()
            .find(|n| Self::eligible(n, req))
            .map(|n| n.id.clone())
            .ok_or(ScheduleError::NoEligibleNode)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, vram: u32, warm: &[&str]) -> NodeSnapshot {
        NodeSnapshot {
            id: id.into(),
            vram_gb: vram,
            queue_depth: 0,
            warm_models: warm.iter().map(|s| s.to_string()).collect(),
        }
    }

    #[test]
    fn rejects_nodes_below_the_vram_floor_and_picks_the_first_that_fits() {
        let nodes = vec![node("hptowerz", 20, &[]), node("gmkmini", 108, &[])];
        let req = ScheduleRequest {
            required_model: None,
            min_vram_gb: 32,
        };
        assert_eq!(FakeScheduler.select(&nodes, &req).unwrap(), "gmkmini");
    }

    #[test]
    fn no_eligible_node_is_an_error() {
        let nodes = vec![node("hptowerz", 20, &[])];
        let req = ScheduleRequest {
            required_model: Some("qwen3-coder:30b".into()),
            min_vram_gb: 0,
        };
        assert!(matches!(
            FakeScheduler.select(&nodes, &req),
            Err(ScheduleError::NoEligibleNode)
        ));
    }
}
