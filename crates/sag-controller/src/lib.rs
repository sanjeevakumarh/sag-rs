//! `sag-controller` — "the brain".
//!
//! Swap seam: [`Controller`]. It accepts a [`Task`], schedules it onto a node,
//! drives the engine, persists state, and hands back a run id the caller polls.
//! One controller + SQLite leases is enough for several nodes (ARCHITECTURE.md
//! "No NATS initially").
//!
//! The end-to-end `sag hello` milestone adds the controller's networked surface:
//! [`http`] is the `axum` + JSON LAN transport, and [`registry`] the durable node
//! registry nodes register into. The [`Controller`] task-submission seam is
//! unchanged for now — the engine/scheduler wiring behind it lands in a later step.

pub mod http;
pub mod registry;

use std::time::{SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use sag_proto::Task;

/// Unix seconds now — stamped as a controller's `start_epoch` for leader election
/// (leader = `min` by `(start_epoch, latency)`). Saturates at the epoch on the
/// impossible pre-1970 clock rather than panicking.
pub fn now_epoch() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[derive(Debug, thiserror::Error)]
pub enum ControlError {
    #[error("task rejected: {0}")]
    Rejected(String),
}

/// The swap seam: accept a task for execution and return a run id to poll.
#[async_trait]
pub trait Controller: Send + Sync {
    async fn submit(&self, task: Task) -> Result<String, ControlError>;
}

/// Offline controller for tests: validates the task and derives a deterministic
/// run id from its id. No scheduler, no engine, no persistence — just the seam.
pub struct FakeController;

#[async_trait]
impl Controller for FakeController {
    async fn submit(&self, task: Task) -> Result<String, ControlError> {
        if task.request.trim().is_empty() {
            return Err(ControlError::Rejected("empty request".into()));
        }
        Ok(format!("run-{}", task.id))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn submit_returns_a_run_id_for_a_valid_task() {
        let task = Task {
            id: "t1".into(),
            request: "Fix intermittent cache expiry test".into(),
        };
        assert_eq!(FakeController.submit(task).await.unwrap(), "run-t1");
    }

    #[tokio::test]
    async fn empty_request_is_rejected() {
        let task = Task {
            id: "t2".into(),
            request: "   ".into(),
        };
        assert!(matches!(
            FakeController.submit(task).await,
            Err(ControlError::Rejected(_))
        ));
    }
}
