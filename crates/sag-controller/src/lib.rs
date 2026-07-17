//! `sag-controller` — "the brain".
//!
//! Swap seam: [`Controller`]. It accepts a [`Task`], schedules it onto a node,
//! drives the engine, persists state, and hands back a run id the caller polls.
//! One controller + SQLite leases is enough for several nodes (ARCHITECTURE.md
//! "No NATS initially"). Step 1 defines the seam plus a [`FakeController`] that
//! assigns a deterministic run id; the binary (`main.rs`) is a thin entrypoint.

use async_trait::async_trait;
use sag_proto::Task;

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
