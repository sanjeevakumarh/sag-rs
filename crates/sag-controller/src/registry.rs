//! Node registry — the durable list of nodes that have registered.
//!
//! Seam: [`NodeRegistry`]. [`MemoryRegistry`] is the no-I/O impl the HTTP layer is
//! unit-tested against; [`SqliteRegistry`] is the durable one the binary runs, so a
//! controller restart re-reads the nodes it knew (part of the re-entrancy contract).
//! Registration is an **upsert by `node_id`** — a node restarting or upgrading
//! refreshes its row instead of creating a duplicate.

use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Mutex;

use async_trait::async_trait;
use sag_proto::NodeDescriptor;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::{Row, SqlitePool};

#[derive(Debug, thiserror::Error)]
pub enum RegistryError {
    #[error("registry backend error: {0}")]
    Backend(String),
}

/// The registry seam: upsert a node, or list everything currently registered.
#[async_trait]
pub trait NodeRegistry: Send + Sync {
    /// Insert or refresh `node` keyed by its `node_id`.
    async fn upsert(&self, node: NodeDescriptor) -> Result<(), RegistryError>;
    /// All registered nodes, ordered by `node_id` for stable output.
    async fn list(&self) -> Result<Vec<NodeDescriptor>, RegistryError>;
}

/// In-memory registry for tests and ephemeral runs. No persistence, no I/O.
#[derive(Default)]
pub struct MemoryRegistry {
    nodes: Mutex<HashMap<String, NodeDescriptor>>,
}

impl MemoryRegistry {
    pub fn new() -> Self {
        Self::default()
    }
}

#[async_trait]
impl NodeRegistry for MemoryRegistry {
    async fn upsert(&self, node: NodeDescriptor) -> Result<(), RegistryError> {
        self.nodes
            .lock()
            .expect("registry mutex poisoned")
            .insert(node.node_id.clone(), node);
        Ok(())
    }

    async fn list(&self) -> Result<Vec<NodeDescriptor>, RegistryError> {
        let mut nodes: Vec<NodeDescriptor> = self
            .nodes
            .lock()
            .expect("registry mutex poisoned")
            .values()
            .cloned()
            .collect();
        nodes.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        Ok(nodes)
    }
}

/// Durable registry backed by SQLite (WAL). The descriptor is stored as JSON so the
/// schema stays stable as [`NodeDescriptor`] gains additive fields — a forward-compat
/// choice: a new field needs no migration, it just round-trips through the JSON blob.
pub struct SqliteRegistry {
    pool: SqlitePool,
}

impl SqliteRegistry {
    /// Open (creating if absent) the SQLite database at `url`, enable WAL, and run
    /// embedded migrations. Idempotent: safe to call on every startup.
    ///
    /// `url` is a SQLite connection string, e.g. `sqlite://sag-controller.db` or
    /// `sqlite::memory:` for tests.
    pub async fn connect(url: &str) -> Result<Self, RegistryError> {
        let opts = SqliteConnectOptions::from_str(url)
            .map_err(|e| RegistryError::Backend(e.to_string()))?
            .create_if_missing(true)
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal);
        let pool = SqlitePoolOptions::new()
            .connect_with(opts)
            .await
            .map_err(|e| RegistryError::Backend(e.to_string()))?;
        sqlx::migrate!("./migrations")
            .run(&pool)
            .await
            .map_err(|e| RegistryError::Backend(e.to_string()))?;
        Ok(Self { pool })
    }
}

#[async_trait]
impl NodeRegistry for SqliteRegistry {
    async fn upsert(&self, node: NodeDescriptor) -> Result<(), RegistryError> {
        let descriptor =
            serde_json::to_string(&node).map_err(|e| RegistryError::Backend(e.to_string()))?;
        sqlx::query(
            "INSERT INTO nodes (node_id, addr, descriptor, updated_at) \
             VALUES (?1, ?2, ?3, strftime('%s', 'now')) \
             ON CONFLICT(node_id) DO UPDATE SET \
             addr = excluded.addr, descriptor = excluded.descriptor, updated_at = excluded.updated_at",
        )
        .bind(&node.node_id)
        .bind(&node.addr)
        .bind(&descriptor)
        .execute(&self.pool)
        .await
        .map_err(|e| RegistryError::Backend(e.to_string()))?;
        Ok(())
    }

    async fn list(&self) -> Result<Vec<NodeDescriptor>, RegistryError> {
        let rows = sqlx::query("SELECT descriptor FROM nodes ORDER BY node_id")
            .fetch_all(&self.pool)
            .await
            .map_err(|e| RegistryError::Backend(e.to_string()))?;
        rows.into_iter()
            .map(|row| {
                let json: String = row.get("descriptor");
                serde_json::from_str(&json).map_err(|e| RegistryError::Backend(e.to_string()))
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use sag_proto::ModelDescriptor;

    fn node(id: &str, addr: &str) -> NodeDescriptor {
        NodeDescriptor {
            node_id: id.into(),
            addr: addr.into(),
            models: vec![ModelDescriptor {
                id: "qwen3-coder:30b".into(),
                capabilities: vec![],
            }],
            capabilities: vec![],
        }
    }

    #[tokio::test]
    async fn memory_registry_upserts_and_lists_sorted() {
        let reg = MemoryRegistry::new();
        reg.upsert(node("zeta", "http://z:8080")).await.unwrap();
        reg.upsert(node("alpha", "http://a:8080")).await.unwrap();
        let nodes = reg.list().await.unwrap();
        assert_eq!(
            nodes.iter().map(|n| n.node_id.as_str()).collect::<Vec<_>>(),
            ["alpha", "zeta"]
        );
    }

    #[tokio::test]
    async fn memory_registry_upsert_is_idempotent_by_node_id() {
        let reg = MemoryRegistry::new();
        reg.upsert(node("alpha", "http://old:8080")).await.unwrap();
        reg.upsert(node("alpha", "http://new:8080")).await.unwrap();
        let nodes = reg.list().await.unwrap();
        assert_eq!(nodes.len(), 1, "re-register must not duplicate");
        assert_eq!(nodes[0].addr, "http://new:8080", "address refreshed");
    }

    #[tokio::test]
    async fn sqlite_registry_persists_and_upserts() {
        // A shared in-memory DB (no file) exercises the real SQL + migrations path
        // with no cluster, no disk.
        let reg = SqliteRegistry::connect("sqlite::memory:").await.unwrap();
        reg.upsert(node("alpha", "http://old:8080")).await.unwrap();
        reg.upsert(node("alpha", "http://new:8080")).await.unwrap();
        reg.upsert(node("beta", "http://b:8080")).await.unwrap();
        let nodes = reg.list().await.unwrap();
        assert_eq!(nodes.len(), 2);
        assert_eq!(nodes[0].node_id, "alpha");
        assert_eq!(nodes[0].addr, "http://new:8080");
    }
}
