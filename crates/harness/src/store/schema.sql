PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS items (hash TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS requests (
 id TEXT PRIMARY KEY, parent_id TEXT REFERENCES requests(id), branch TEXT NOT NULL,
 created_at INTEGER NOT NULL DEFAULT(CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)),
 input_tokens INTEGER NOT NULL DEFAULT 0, output_tokens INTEGER NOT NULL DEFAULT 0,
 cost_micros INTEGER NOT NULL DEFAULT 0, UNIQUE(parent_id, branch)
);
CREATE TABLE IF NOT EXISTS request_items (
 request_id TEXT NOT NULL REFERENCES requests(id), position INTEGER NOT NULL,
 item_hash TEXT NOT NULL REFERENCES items(hash), PRIMARY KEY(request_id, position)
);
CREATE TABLE IF NOT EXISTS events (
 id INTEGER PRIMARY KEY AUTOINCREMENT, request_id TEXT REFERENCES requests(id),
 kind TEXT NOT NULL, payload TEXT NOT NULL, created_at INTEGER NOT NULL DEFAULT(CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER))
);
CREATE TABLE IF NOT EXISTS envelopes (
 id INTEGER PRIMARY KEY AUTOINCREMENT, sender TEXT NOT NULL, recipient TEXT NOT NULL,
 class TEXT NOT NULL, item_hash TEXT NOT NULL REFERENCES items(hash),
 delivered_request TEXT REFERENCES requests(id), created_at INTEGER NOT NULL DEFAULT(CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER))
);
CREATE INDEX IF NOT EXISTS envelopes_recipient ON envelopes(recipient, id);
CREATE TABLE IF NOT EXISTS claims (
 call_id TEXT NOT NULL, request_id TEXT NOT NULL REFERENCES requests(id),
 state TEXT NOT NULL CHECK(state IN ('pending','settled','interrupted')),
 output_hash TEXT REFERENCES items(hash), PRIMARY KEY(call_id, request_id)
);
CREATE INDEX IF NOT EXISTS claims_request ON claims(request_id, state);
CREATE TABLE IF NOT EXISTS decisions (
 id INTEGER PRIMARY KEY AUTOINCREMENT, request_id TEXT REFERENCES requests(id),
 hook TEXT NOT NULL, event_refs TEXT NOT NULL, decision TEXT NOT NULL,
 evidence TEXT NOT NULL, created_at INTEGER NOT NULL DEFAULT(CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)), latency_ms INTEGER
);
CREATE TABLE IF NOT EXISTS session_state (
 session_id TEXT PRIMARY KEY, state TEXT NOT NULL,
 updated_at INTEGER NOT NULL DEFAULT(CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER))
);
