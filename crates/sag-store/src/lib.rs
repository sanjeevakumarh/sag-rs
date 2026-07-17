//! `sag-store` — "remember + artifacts".
//!
//! Swap seam: [`Store`]. Agents produce immutable artifacts (plan.json,
//! changes.patch, review-findings.json, ...); the store keeps them
//! content-addressed, so identical bytes always map to the same id and nothing is
//! mutated in place (ARCHITECTURE.md "Artifacts, not mutation"). Step 1 defines the
//! seam plus an in-memory [`FakeStore`]; the real one is `sqlx` + a `blake3`
//! content-addressed dir.

use async_trait::async_trait;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("artifact not found: {0}")]
    NotFound(String),
}

/// The swap seam: store immutable artifacts and fetch them back by content id.
#[async_trait]
pub trait Store: Send + Sync {
    /// Store `bytes` and return their content id. Storing the same bytes twice
    /// yields the same id (dedupe), never a second copy.
    async fn put(&self, bytes: Vec<u8>) -> Result<String, StoreError>;
    async fn get(&self, id: &str) -> Result<Vec<u8>, StoreError>;
}

/// In-memory content-addressed store for tests: the id is a stable digest of the
/// bytes, so identical content collapses to one entry. No filesystem, no `sqlx`,
/// no `blake3` — the real store swaps those in behind this same seam.
#[derive(Default)]
pub struct FakeStore {
    artifacts: Mutex<HashMap<String, Vec<u8>>>,
}

fn digest(bytes: &[u8]) -> String {
    // `DefaultHasher` has fixed keys, so the same bytes hash identically — enough
    // to model content addressing for tests without pulling in a crypto hash.
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    format!("{:016x}", h.finish())
}

#[async_trait]
impl Store for FakeStore {
    async fn put(&self, bytes: Vec<u8>) -> Result<String, StoreError> {
        let id = digest(&bytes);
        self.artifacts.lock().unwrap().insert(id.clone(), bytes);
        Ok(id)
    }

    async fn get(&self, id: &str) -> Result<Vec<u8>, StoreError> {
        self.artifacts
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .ok_or_else(|| StoreError::NotFound(id.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn round_trips_and_dedupes_identical_bytes() {
        let store = FakeStore::default();
        let id1 = store.put(b"plan: fix cache".to_vec()).await.unwrap();
        let id2 = store.put(b"plan: fix cache".to_vec()).await.unwrap();
        assert_eq!(id1, id2, "identical content must share an id");
        assert_eq!(store.get(&id1).await.unwrap(), b"plan: fix cache");
    }

    #[tokio::test]
    async fn missing_id_is_not_found() {
        let store = FakeStore::default();
        assert!(matches!(
            store.get("deadbeef").await,
            Err(StoreError::NotFound(_))
        ));
    }
}
