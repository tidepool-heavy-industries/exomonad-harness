PRAGMA foreign_keys = ON;
CREATE TABLE IF NOT EXISTS schema_version (version INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS items (hash TEXT PRIMARY KEY, json TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS requests (
 id TEXT PRIMARY KEY, parent_id TEXT REFERENCES requests(id), branch TEXT NOT NULL,
 created_at INTEGER NOT NULL DEFAULT(CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)),
 input_tokens INTEGER NOT NULL DEFAULT 0, output_tokens INTEGER NOT NULL DEFAULT 0,
 cost_micros INTEGER NOT NULL DEFAULT 0,
 embedded_run TEXT, embedded_incarnation TEXT,
 round_phase TEXT CHECK(round_phase IN ('pending','completed','cancelled','rejected')),
 CHECK((embedded_run IS NULL AND embedded_incarnation IS NULL AND round_phase IS NULL) OR
       (embedded_run IS NOT NULL AND embedded_incarnation IS NOT NULL AND round_phase IS NOT NULL)),
 UNIQUE(parent_id, branch)
);
CREATE TABLE IF NOT EXISTS request_items (
 request_id TEXT NOT NULL REFERENCES requests(id), position INTEGER NOT NULL,
 item_hash TEXT NOT NULL REFERENCES items(hash),
 source_request TEXT, source_position INTEGER, context_sources TEXT,
 context_note INTEGER NOT NULL DEFAULT 0,
 context_overlays TEXT,
 PRIMARY KEY(request_id, position)
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
 origin TEXT NOT NULL, origin_request_id TEXT NOT NULL REFERENCES requests(id),
 call_id TEXT NOT NULL, request_id TEXT NOT NULL REFERENCES requests(id),
 state TEXT NOT NULL CHECK(state IN ('pending','settled','interrupted')),
 output_hash TEXT REFERENCES items(hash),
 terminal_json TEXT,
 PRIMARY KEY(origin, origin_request_id, call_id, request_id)
);
CREATE INDEX IF NOT EXISTS claims_request ON claims(request_id, state);
CREATE INDEX IF NOT EXISTS claims_operation ON claims(origin, origin_request_id, call_id, state);
CREATE TABLE IF NOT EXISTS decisions (
 id INTEGER PRIMARY KEY AUTOINCREMENT, request_id TEXT REFERENCES requests(id),
 hook TEXT NOT NULL, event_refs TEXT NOT NULL, decision TEXT NOT NULL,
 evidence TEXT NOT NULL, created_at INTEGER NOT NULL DEFAULT(CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER)), latency_ms INTEGER
);
CREATE TABLE IF NOT EXISTS session_state (
 session_id TEXT PRIMARY KEY, state TEXT NOT NULL,
 updated_at INTEGER NOT NULL DEFAULT(CAST((julianday('now') - 2440587.5) * 86400000 AS INTEGER))
);
CREATE TABLE IF NOT EXISTS agents (
 path TEXT PRIMARY KEY, parent_path TEXT REFERENCES agents(path),
 head_request TEXT REFERENCES requests(id), contract TEXT NOT NULL,
 fork_source TEXT NOT NULL, state TEXT NOT NULL CHECK(state IN ('active','idle','completed','cancelled')),
 created_at INTEGER NOT NULL,
 CHECK((path='/root' AND parent_path IS NULL) OR
       (parent_path IS NOT NULL AND path GLOB parent_path || '/*'))
);
CREATE INDEX IF NOT EXISTS agents_parent ON agents(parent_path,path);
CREATE TABLE IF NOT EXISTS checkpoints (
 id TEXT PRIMARY KEY, origin_agent TEXT NOT NULL REFERENCES agents(path),
 source_request TEXT NOT NULL REFERENCES requests(id),
 snapshot_request TEXT NOT NULL UNIQUE REFERENCES requests(id),
 boundary_call TEXT NOT NULL, metadata TEXT NOT NULL, pending_claims TEXT NOT NULL,
 created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS embedded_bindings (
 agent_path TEXT PRIMARY KEY REFERENCES agents(path),
 run_id TEXT NOT NULL, incarnation TEXT NOT NULL
);
CREATE TABLE IF NOT EXISTS embedded_inputs (
 run_id TEXT NOT NULL, agent_path TEXT NOT NULL REFERENCES embedded_bindings(agent_path),
 incarnation TEXT NOT NULL, operation_id TEXT NOT NULL,
 envelope_id INTEGER NOT NULL REFERENCES envelopes(id),
 item_hash TEXT NOT NULL REFERENCES items(hash),
 PRIMARY KEY(run_id, agent_path, incarnation, operation_id)
);
CREATE TABLE IF NOT EXISTS embedded_context_seeds (
 run_id TEXT NOT NULL, agent_path TEXT NOT NULL REFERENCES embedded_bindings(agent_path),
 incarnation TEXT NOT NULL, operation_id TEXT NOT NULL,
 request_id TEXT NOT NULL UNIQUE REFERENCES requests(id),
 item_hash TEXT NOT NULL REFERENCES items(hash),
 PRIMARY KEY(run_id, agent_path, incarnation, operation_id)
);

CREATE TABLE IF NOT EXISTS embedded_commands (
 run_id TEXT NOT NULL, operation_id TEXT NOT NULL,
 target TEXT NOT NULL, action TEXT NOT NULL, payload TEXT NOT NULL,
 expected_round TEXT, command TEXT NOT NULL,
 state TEXT NOT NULL CHECK(state IN ('queued','dispatching','input_admitted','control_requested','refused','unconfirmed')),
 envelope_id INTEGER REFERENCES envelopes(id), outcome TEXT,
 created_at INTEGER NOT NULL,
 PRIMARY KEY(run_id, operation_id)
);
CREATE INDEX IF NOT EXISTS embedded_commands_queued ON embedded_commands(run_id,state,created_at);
