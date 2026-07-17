-- Registered nodes. The full descriptor is kept as JSON so additive fields in
-- `NodeDescriptor` need no schema change (forward-compat); the columns we index or
-- query on are promoted out for convenience.
CREATE TABLE IF NOT EXISTS nodes (
    node_id    TEXT PRIMARY KEY,
    addr       TEXT NOT NULL,
    descriptor TEXT NOT NULL,
    updated_at INTEGER NOT NULL
);
