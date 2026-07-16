//! `sag-engine` — "run a DAG of steps". The crown jewel; hand-built, not wrapped.
//!
//! Swap seam: [`Engine`]. It takes a [`Task`] and drives it through a DAG of
//! stages (retrieve → plan → implement → test → review → ...), looping only the
//! failing ones. Step 1 defines the seam and a [`FakeEngine`]; step 2 builds the
//! real actor-style engine (`tokio::sync::mpsc` + owned per-task state, never
//! `Arc<Mutex<Graph>>` — see ARCHITECTURE.md "One Rust-specific caution").

use async_trait::async_trait;
use sag_proto::Task;

/// Terminal state of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    Completed,
    Failed,
}

/// The result of driving a task through the DAG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub task_id: String,
    pub status: RunStatus,
    /// Stages executed, in order — the replay trail.
    pub stages: Vec<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("stage {stage} failed: {reason}")]
    StageFailed { stage: String, reason: String },
}

/// The swap seam: something that can run a task's DAG to completion.
#[async_trait]
pub trait Engine: Send + Sync {
    async fn run(&self, task: Task) -> Result<RunOutcome, EngineError>;
}

/// Deterministic in-memory engine for tests: walks a fixed happy-path DAG and
/// reports every stage as completed. No mpsc, no nodes, no network.
pub struct FakeEngine;

#[async_trait]
impl Engine for FakeEngine {
    async fn run(&self, task: Task) -> Result<RunOutcome, EngineError> {
        let stages = ["retrieve", "plan", "implement", "test", "review"]
            .into_iter()
            .map(String::from)
            .collect();
        Ok(RunOutcome {
            task_id: task.id,
            status: RunStatus::Completed,
            stages,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_engine_completes_the_happy_path_dag() {
        let task = Task {
            id: "t1".into(),
            request: "Fix intermittent cache expiry test".into(),
        };
        let outcome = FakeEngine.run(task).await.expect("run");
        assert_eq!(outcome.task_id, "t1");
        assert_eq!(outcome.status, RunStatus::Completed);
        assert_eq!(outcome.stages.first().map(String::as_str), Some("retrieve"));
        assert_eq!(outcome.stages.last().map(String::as_str), Some("review"));
    }
}
