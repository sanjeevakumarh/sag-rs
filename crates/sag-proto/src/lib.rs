//! `sag-proto` — "shared vocabulary".
//!
//! The one crate with no swap seam: it carries the serde types every other crate
//! speaks (`Task`, `RepoRef`, `Citation`, `NodeSnapshot`, ...). Keeping them here
//! means the engine, scheduler, retrieval, and store agree on a shape without
//! depending on each other. No behavior lives here — only the vocabulary.

use serde::{Deserialize, Serialize};

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
}
