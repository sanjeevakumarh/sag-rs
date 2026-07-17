//! `sag-engine` — "run a DAG of steps". The crown jewel; hand-built, not wrapped.
//!
//! Swap seam: [`Engine`]. It takes a [`Task`] and drives it through a DAG of
//! stages (retrieve → plan → implement → test → review → ...), retrying only the
//! failing ones with backoff.
//!
//! The real engine ([`GraphEngine`]) is **actor-style / message-passing**: a
//! coordinator owns the graph state (each stage's status + attempt count) and
//! spawns a worker per eligible stage; workers report their outcome back over a
//! `tokio::sync::mpsc` channel. Nothing shares mutable graph state — there is no
//! `Arc<Mutex<Graph>>` (ARCHITECTURE.md "One Rust-specific caution").
//!
//! What each stage *does* (retrieve via the scout, plan/implement/review via
//! inference, test in a sandbox) is deferred behind the [`StageRunner`] seam so
//! the engine's control flow — dependencies, retries, convergence — is built and
//! tested here with no model, cluster, or GPU. Real runners slot in at later
//! build-order steps.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use sag_proto::Task;
use tokio::sync::mpsc;

/// Terminal state of a run.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunStatus {
    /// Every stage reached `Done`.
    Completed,
    /// A stage exhausted its retries; the run could not converge.
    Failed,
}

/// The result of driving a task through the DAG.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunOutcome {
    pub task_id: String,
    pub status: RunStatus,
    /// Stages that completed, in the order they finished — the replay trail.
    pub stages: Vec<String>,
    /// The stage that exhausted its retries, when `status == Failed`.
    pub failed_stage: Option<String>,
}

/// Structural problems with how the engine was *configured* — as opposed to a
/// stage failing at runtime, which is an expected [`RunStatus::Failed`] outcome.
#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("invalid DAG: {reason}")]
    InvalidDag { reason: String },
}

/// Why a stage's work failed. The message flows into [`RunOutcome::failed_stage`]
/// diagnostics after retries are exhausted.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct StageError(pub String);

/// The swap seam: something that can run a task's DAG to completion.
#[async_trait]
pub trait Engine: Send + Sync {
    async fn run(&self, task: Task) -> Result<RunOutcome, EngineError>;
}

/// Executes the work of a single stage. This is where the real subsystems plug
/// in later (scout retrieval, inference for plan/implement/review, sandboxed
/// tests); the engine only cares that a stage either succeeds or fails.
#[async_trait]
pub trait StageRunner: Send + Sync {
    /// Run `stage` for `task`. `Ok(())` marks it done; `Err` triggers a retry (or
    /// fails the run once attempts are exhausted).
    async fn run_stage(&self, stage: &str, task: &Task) -> Result<(), StageError>;
}

/// One node in the DAG: a named stage, the stages it depends on, and how many
/// attempts it gets before the run is declared failed.
#[derive(Debug, Clone)]
pub struct StageSpec {
    pub name: String,
    /// Names of stages that must be `Done` before this one becomes eligible.
    pub deps: Vec<String>,
    /// Total attempts (>= 1) before the stage is considered failed.
    pub max_attempts: u32,
}

impl StageSpec {
    /// A stage with the given dependencies and a default retry budget.
    pub fn new(name: impl Into<String>, deps: impl IntoIterator<Item = &'static str>) -> Self {
        Self {
            name: name.into(),
            deps: deps.into_iter().map(String::from).collect(),
            max_attempts: 3,
        }
    }

    pub fn with_max_attempts(mut self, max_attempts: u32) -> Self {
        self.max_attempts = max_attempts.max(1);
        self
    }
}

/// The stage graph. Validated once (dangling deps, cycles) before a run starts.
#[derive(Debug, Clone)]
pub struct Dag {
    stages: Vec<StageSpec>,
}

impl Dag {
    pub fn new(stages: impl IntoIterator<Item = StageSpec>) -> Self {
        Self {
            stages: stages.into_iter().collect(),
        }
    }

    /// The default `sag run fix` pipeline: a linear chain
    /// retrieve → plan → implement → test → review (ARCHITECTURE.md "First
    /// milestone", steps 2–7).
    pub fn fix_pipeline() -> Self {
        Self::new([
            StageSpec::new("retrieve", []),
            StageSpec::new("plan", ["retrieve"]),
            StageSpec::new("implement", ["plan"]),
            StageSpec::new("test", ["implement"]),
            StageSpec::new("review", ["test"]),
        ])
    }

    /// Reject dangling dependencies, duplicate names, and cycles before we start
    /// spawning workers — a bad DAG should fail loudly, not deadlock.
    fn validate(&self) -> Result<(), EngineError> {
        let mut names: HashMap<&str, &StageSpec> = HashMap::new();
        for spec in &self.stages {
            if names.insert(spec.name.as_str(), spec).is_some() {
                return Err(EngineError::InvalidDag {
                    reason: format!("duplicate stage `{}`", spec.name),
                });
            }
        }
        for spec in &self.stages {
            for dep in &spec.deps {
                if !names.contains_key(dep.as_str()) {
                    return Err(EngineError::InvalidDag {
                        reason: format!("stage `{}` depends on unknown `{dep}`", spec.name),
                    });
                }
            }
        }
        self.check_acyclic(&names)
    }

    /// DFS with a three-colour marking (white/grey/black) to catch back-edges.
    fn check_acyclic(&self, specs: &HashMap<&str, &StageSpec>) -> Result<(), EngineError> {
        #[derive(Clone, Copy, PartialEq)]
        enum Mark {
            Grey,
            Black,
        }
        let mut marks: HashMap<&str, Mark> = HashMap::new();
        // Explicit stack carrying an "entering vs leaving" flag so we colour a
        // node black only after all its dependencies are fully explored.
        for spec in &self.stages {
            if marks.contains_key(spec.name.as_str()) {
                continue;
            }
            let mut stack: Vec<(&str, bool)> = vec![(spec.name.as_str(), false)];
            while let Some((name, leaving)) = stack.pop() {
                if leaving {
                    marks.insert(name, Mark::Black);
                    continue;
                }
                match marks.get(name) {
                    Some(Mark::Black) => continue,
                    Some(Mark::Grey) => {} // re-entering our own frame, handled below
                    None => {
                        marks.insert(name, Mark::Grey);
                    }
                }
                stack.push((name, true));
                for dep in &specs[name].deps {
                    match marks.get(dep.as_str()) {
                        Some(Mark::Grey) => {
                            return Err(EngineError::InvalidDag {
                                reason: format!("cycle through `{dep}`"),
                            });
                        }
                        Some(Mark::Black) => {}
                        None => stack.push((dep.as_str(), false)),
                    }
                }
            }
        }
        Ok(())
    }
}

/// How long a worker waits before a *retry* attempt. Exponential in the retry
/// index, capped. The first attempt (retry 0) never waits.
#[derive(Debug, Clone, Copy)]
pub struct BackoffPolicy {
    base: Duration,
    cap: Duration,
}

impl BackoffPolicy {
    pub fn exponential(base: Duration, cap: Duration) -> Self {
        Self { base, cap }
    }

    /// No delay between retries — used in tests to keep them fast.
    pub fn none() -> Self {
        Self {
            base: Duration::ZERO,
            cap: Duration::ZERO,
        }
    }

    /// Delay before the attempt at the given `retry` index (0 = first try).
    fn delay(&self, retry: u32) -> Duration {
        if retry == 0 || self.base.is_zero() {
            return Duration::ZERO;
        }
        // base * 2^(retry-1), saturating, capped.
        let factor = 1u32.checked_shl(retry - 1).unwrap_or(u32::MAX);
        self.base.saturating_mul(factor).min(self.cap)
    }
}

impl Default for BackoffPolicy {
    fn default() -> Self {
        Self::exponential(Duration::from_millis(200), Duration::from_secs(10))
    }
}

/// The real engine: drives a [`Dag`] to completion actor-style. Generic over the
/// [`StageRunner`] so tests (and later, real subsystems) supply the stage work.
pub struct GraphEngine<R: StageRunner + 'static> {
    dag: Dag,
    runner: Arc<R>,
    backoff: BackoffPolicy,
}

impl<R: StageRunner + 'static> GraphEngine<R> {
    pub fn new(dag: Dag, runner: R) -> Self {
        Self {
            dag,
            runner: Arc::new(runner),
            backoff: BackoffPolicy::default(),
        }
    }

    pub fn with_backoff(mut self, backoff: BackoffPolicy) -> Self {
        self.backoff = backoff;
        self
    }
}

/// A stage's position in the coordinator's owned state machine.
#[derive(Clone, Copy, PartialEq)]
enum Status {
    /// Eligible once its deps are `Done`; also the state a stage returns to
    /// between a failed attempt and its retry.
    Pending,
    /// A worker is currently executing this stage.
    Running,
    Done,
}

#[async_trait]
impl<R: StageRunner + 'static> Engine for GraphEngine<R> {
    async fn run(&self, task: Task) -> Result<RunOutcome, EngineError> {
        self.dag.validate()?;

        // ── Owned coordinator state. Nothing here is shared with workers; they
        //    only send outcomes back over the channel. ──────────────────────
        let mut status: HashMap<&str, Status> = self
            .dag
            .stages
            .iter()
            .map(|s| (s.name.as_str(), Status::Pending))
            .collect();
        let mut attempts: HashMap<&str, u32> = self
            .dag
            .stages
            .iter()
            .map(|s| (s.name.as_str(), 0u32))
            .collect();
        let max_attempts: HashMap<&str, u32> = self
            .dag
            .stages
            .iter()
            .map(|s| (s.name.as_str(), s.max_attempts))
            .collect();
        let deps: HashMap<&str, &[String]> = self
            .dag
            .stages
            .iter()
            .map(|s| (s.name.as_str(), s.deps.as_slice()))
            .collect();

        let mut completed: Vec<String> = Vec::new();
        let (tx, mut rx) = mpsc::channel::<(String, Result<(), StageError>)>(self.dag.stages.len());
        let mut in_flight = 0usize;

        loop {
            // Dispatch every eligible stage (Pending with all deps Done). For a
            // linear DAG this is one at a time; for a diamond, siblings run
            // concurrently — that's the point of the actor design.
            let ready: Vec<&str> = self
                .dag
                .stages
                .iter()
                .map(|s| s.name.as_str())
                .filter(|name| status[name] == Status::Pending)
                .filter(|name| {
                    deps[name]
                        .iter()
                        .all(|d| status[d.as_str()] == Status::Done)
                })
                .collect();

            for name in ready {
                status.insert(name, Status::Running);
                let retry = attempts[name];
                let delay = self.backoff.delay(retry);
                let runner = Arc::clone(&self.runner);
                let tx = tx.clone();
                let task = task.clone();
                let stage = name.to_string();
                tokio::spawn(async move {
                    if !delay.is_zero() {
                        tokio::time::sleep(delay).await;
                    }
                    let result = runner.run_stage(&stage, &task).await;
                    // Coordinator lives until in_flight hits 0, so this never fails.
                    let _ = tx.send((stage, result)).await;
                });
                in_flight += 1;
            }

            if in_flight == 0 {
                // Nothing running and nothing eligible. For a validated DAG with
                // no failures this means every stage is Done.
                break;
            }

            let (name, result) = rx
                .recv()
                .await
                .expect("coordinator holds a sender while workers are in flight");
            in_flight -= 1;
            let name = name.as_str();
            // Re-key against our owned string slices.
            let key = *deps.get_key_value(name).expect("known stage").0;

            match result {
                Ok(()) => {
                    status.insert(key, Status::Done);
                    completed.push(key.to_string());
                }
                Err(StageError(reason)) => {
                    let a = attempts.get_mut(key).expect("known stage");
                    *a += 1;
                    if *a >= max_attempts[key] {
                        // Drain nothing further — return the failure. In-flight
                        // siblings' senders drop harmlessly.
                        return Ok(RunOutcome {
                            task_id: task.id,
                            status: RunStatus::Failed,
                            stages: completed,
                            failed_stage: Some(format!("{key}: {reason}")),
                        });
                    }
                    // Retry: back to Pending so the next dispatch pass re-runs it
                    // (only this stage — its dependents never became eligible).
                    status.insert(key, Status::Pending);
                }
            }
        }

        Ok(RunOutcome {
            task_id: task.id,
            status: RunStatus::Completed,
            stages: completed,
            failed_stage: None,
        })
    }
}

/// Deterministic in-memory engine for tests and early wiring: reports the fixed
/// happy-path DAG as completed without spawning workers. No mpsc, no nodes, no
/// network — the trivial [`Engine`] fake other crates can depend on.
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
            failed_stage: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn task() -> Task {
        Task {
            id: "t1".into(),
            request: "Fix intermittent cache expiry test".into(),
        }
    }

    #[tokio::test]
    async fn fake_engine_completes_the_happy_path_dag() {
        let outcome = FakeEngine.run(task()).await.expect("run");
        assert_eq!(outcome.task_id, "t1");
        assert_eq!(outcome.status, RunStatus::Completed);
        assert_eq!(outcome.stages.first().map(String::as_str), Some("retrieve"));
        assert_eq!(outcome.stages.last().map(String::as_str), Some("review"));
    }

    /// A runner that always succeeds and records the order stages were invoked.
    struct HappyRunner {
        seen: Mutex<Vec<String>>,
    }

    impl HappyRunner {
        fn new() -> Self {
            Self {
                seen: Mutex::new(Vec::new()),
            }
        }
    }

    #[async_trait]
    impl StageRunner for HappyRunner {
        async fn run_stage(&self, stage: &str, _task: &Task) -> Result<(), StageError> {
            self.seen.lock().unwrap().push(stage.to_string());
            Ok(())
        }
    }

    /// A runner scripted to fail specific stages a fixed number of times before
    /// succeeding — exercises the retry loop. Counts attempts per stage.
    struct ScriptedRunner {
        fail_times: HashMap<String, u32>,
        attempts: Mutex<HashMap<String, u32>>,
    }

    impl ScriptedRunner {
        fn new(fail_times: impl IntoIterator<Item = (&'static str, u32)>) -> Self {
            Self {
                fail_times: fail_times
                    .into_iter()
                    .map(|(k, v)| (k.to_string(), v))
                    .collect(),
                attempts: Mutex::new(HashMap::new()),
            }
        }

        fn attempts_for(&self, stage: &str) -> u32 {
            self.attempts
                .lock()
                .unwrap()
                .get(stage)
                .copied()
                .unwrap_or(0)
        }
    }

    #[async_trait]
    impl StageRunner for ScriptedRunner {
        async fn run_stage(&self, stage: &str, _task: &Task) -> Result<(), StageError> {
            let n = {
                let mut a = self.attempts.lock().unwrap();
                let e = a.entry(stage.to_string()).or_insert(0);
                *e += 1;
                *e
            };
            let budget = self.fail_times.get(stage).copied().unwrap_or(0);
            if n <= budget {
                Err(StageError(format!("scripted failure #{n}")))
            } else {
                Ok(())
            }
        }
    }

    #[tokio::test]
    async fn graph_engine_runs_linear_pipeline_in_dependency_order() {
        let runner = HappyRunner::new();
        let engine = GraphEngine::new(Dag::fix_pipeline(), runner);
        let outcome = engine.run(task()).await.expect("run");

        assert_eq!(outcome.status, RunStatus::Completed);
        assert_eq!(outcome.failed_stage, None);
        assert_eq!(
            outcome.stages,
            ["retrieve", "plan", "implement", "test", "review"]
        );
    }

    #[tokio::test]
    async fn graph_engine_retries_only_the_failing_stage() {
        // "test" fails twice then passes; with default max_attempts (3) the run
        // still converges. Backoff::none keeps it instant.
        let runner = Arc::new(ScriptedRunner::new([("test", 2)]));
        let engine = GraphEngine::new(Dag::fix_pipeline(), TestRunnerHandle(Arc::clone(&runner)))
            .with_backoff(BackoffPolicy::none());
        let outcome = engine.run(task()).await.expect("run");

        assert_eq!(outcome.status, RunStatus::Completed);
        // Only completed stages appear once each, in order.
        assert_eq!(
            outcome.stages,
            ["retrieve", "plan", "implement", "test", "review"]
        );
        // "test" was attempted 3 times; unaffected stages exactly once.
        assert_eq!(runner.attempts_for("test"), 3);
        assert_eq!(runner.attempts_for("plan"), 1);
        assert_eq!(runner.attempts_for("review"), 1);
    }

    #[tokio::test]
    async fn graph_engine_fails_run_when_a_stage_exhausts_retries() {
        // "implement" always fails; run stops there, never reaching test/review.
        let runner = Arc::new(ScriptedRunner::new([("implement", u32::MAX)]));
        let engine = GraphEngine::new(Dag::fix_pipeline(), TestRunnerHandle(Arc::clone(&runner)))
            .with_backoff(BackoffPolicy::none());
        let outcome = engine.run(task()).await.expect("run");

        assert_eq!(outcome.status, RunStatus::Failed);
        assert_eq!(outcome.stages, ["retrieve", "plan"]);
        assert!(outcome
            .failed_stage
            .as_deref()
            .unwrap()
            .starts_with("implement:"));
        // Exhausted the default budget of 3 attempts, no more.
        assert_eq!(runner.attempts_for("implement"), 3);
        assert_eq!(runner.attempts_for("test"), 0);
    }

    #[tokio::test]
    async fn graph_engine_respects_diamond_dependencies() {
        // a → {b, c} → d. b and c may run concurrently, but a is always first
        // and d always last.
        let dag = Dag::new([
            StageSpec::new("a", []),
            StageSpec::new("b", ["a"]),
            StageSpec::new("c", ["a"]),
            StageSpec::new("d", ["b", "c"]),
        ]);
        let engine = GraphEngine::new(dag, HappyRunner::new());
        let outcome = engine.run(task()).await.expect("run");

        assert_eq!(outcome.status, RunStatus::Completed);
        assert_eq!(outcome.stages.first().map(String::as_str), Some("a"));
        assert_eq!(outcome.stages.last().map(String::as_str), Some("d"));
        assert_eq!(outcome.stages.len(), 4);
    }

    #[tokio::test]
    async fn invalid_dag_is_rejected_before_running() {
        let cyclic = Dag::new([StageSpec::new("x", ["y"]), StageSpec::new("y", ["x"])]);
        let err = GraphEngine::new(cyclic, HappyRunner::new())
            .run(task())
            .await
            .expect_err("cycle must be rejected");
        assert!(matches!(err, EngineError::InvalidDag { .. }));

        let dangling = Dag::new([StageSpec::new("only", ["missing"])]);
        let err = GraphEngine::new(dangling, HappyRunner::new())
            .run(task())
            .await
            .expect_err("dangling dep must be rejected");
        assert!(matches!(err, EngineError::InvalidDag { .. }));
    }

    /// Lets a test hold its own `Arc` to inspect attempt counts while the engine
    /// also owns the runner through the `StageRunner` seam.
    struct TestRunnerHandle(Arc<ScriptedRunner>);

    #[async_trait]
    impl StageRunner for TestRunnerHandle {
        async fn run_stage(&self, stage: &str, task: &Task) -> Result<(), StageError> {
            self.0.run_stage(stage, task).await
        }
    }
}
