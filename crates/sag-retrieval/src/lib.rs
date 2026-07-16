//! `sag-retrieval` — "find relevant context".
//!
//! Swap seam: [`CodeExplorer`]. The scout stage: given a repo and a question it
//! returns compact [`Citation`]s (path + inclusive line range + reason) instead of
//! whole files, so the context bundle stays small. Step 5 builds `NativeExplorer`
//! (ripgrep + tree-sitter), and later the FastContext model, behind this same seam
//! — a config flip, not a rewrite. Step 1 defines it plus a deterministic
//! [`FakeExplorer`] that needs no ripgrep and no model.

use async_trait::async_trait;
use sag_proto::{Citation, RepoRef};

#[derive(Debug, thiserror::Error)]
pub enum ExploreError {
    #[error("repo not found: {0}")]
    RepoNotFound(String),
}

/// The swap seam: turn a natural-language question about a repo into citations.
#[async_trait]
pub trait CodeExplorer: Send + Sync {
    async fn explore(&self, repo: &RepoRef, query: &str) -> Result<Vec<Citation>, ExploreError>;
}

/// Deterministic offline scout for tests: returns a single citation pointing at a
/// conventional entrypoint and echoes the query as the reason. No ripgrep, no
/// tree-sitter, no model — just enough to exercise the seam and the retrieval stage.
pub struct FakeExplorer;

#[async_trait]
impl CodeExplorer for FakeExplorer {
    async fn explore(&self, repo: &RepoRef, query: &str) -> Result<Vec<Citation>, ExploreError> {
        if repo.path.trim().is_empty() {
            return Err(ExploreError::RepoNotFound(repo.path.clone()));
        }
        Ok(vec![Citation {
            path: "src/lib.rs".into(),
            lines: (1, 1),
            reason: format!("entrypoint relevant to: {query}"),
        }])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn fake_explorer_cites_for_a_valid_repo() {
        let repo = RepoRef {
            path: "/home/op/proj".into(),
        };
        let cites = FakeExplorer.explore(&repo, "cache expiry").await.unwrap();
        assert_eq!(cites.len(), 1);
        assert_eq!(cites[0].path, "src/lib.rs");
        assert!(cites[0].reason.contains("cache expiry"));
    }

    #[tokio::test]
    async fn empty_repo_path_is_not_found() {
        let repo = RepoRef { path: "".into() };
        assert!(matches!(
            FakeExplorer.explore(&repo, "anything").await,
            Err(ExploreError::RepoNotFound(_))
        ));
    }
}
