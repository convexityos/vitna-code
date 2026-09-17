-- Vitna Code Initial Database Schema (Migration 0001)
-- Persistence model: Append-only cryptographic event store + transactional materialized state

PRAGMA journal_mode = WAL;
PRAGMA synchronous = NORMAL;
PRAGMA foreign_keys = ON;

-- 1. Schema Migrations Ledger
CREATE TABLE IF NOT EXISTS schema_migrations (
    version INTEGER PRIMARY KEY,
    description TEXT NOT NULL,
    applied_at_ms INTEGER NOT NULL
);

-- 2. Append-Only Cryptographic Event Store
CREATE TABLE IF NOT EXISTS events (
    event_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL,
    sequence INTEGER NOT NULL,
    type_url TEXT NOT NULL,
    timestamp_ms INTEGER NOT NULL,
    encrypted_payload BLOB NOT NULL,
    prev_event_hash TEXT NOT NULL,
    event_hash TEXT NOT NULL,
    UNIQUE(run_id, sequence)
);

CREATE INDEX IF NOT EXISTS idx_events_run_sequence ON events(run_id, sequence);
CREATE INDEX IF NOT EXISTS idx_events_timestamp ON events(timestamp_ms);

-- 3. Materialized Entities: Workspaces and Clones
CREATE TABLE IF NOT EXISTS workspaces (
    workspace_id TEXT PRIMARY KEY,
    canonical_root TEXT NOT NULL UNIQUE,
    fingerprint TEXT NOT NULL,
    trust_state TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS agent_workspaces (
    agent_workspace_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    path TEXT NOT NULL UNIQUE,
    base_sha TEXT NOT NULL,
    state TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS snapshots (
    snapshot_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    base_commit_sha TEXT NOT NULL,
    tracked_diff TEXT,
    untracked_files_json TEXT,
    created_at_ms INTEGER NOT NULL
);

-- 4. Sessions, Runs, and Turns
CREATE TABLE IF NOT EXISTS sessions (
    session_id TEXT PRIMARY KEY,
    workspace_id TEXT NOT NULL REFERENCES workspaces(workspace_id),
    title TEXT NOT NULL,
    profile TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    updated_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS agent_runs (
    run_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(session_id),
    parent_run_id TEXT,
    agent_workspace_id TEXT NOT NULL REFERENCES agent_workspaces(agent_workspace_id),
    model_policy_digest TEXT,
    status TEXT NOT NULL,
    budget_tokens INTEGER,
    created_at_ms INTEGER NOT NULL,
    finished_at_ms INTEGER
);

CREATE TABLE IF NOT EXISTS turns (
    turn_id TEXT PRIMARY KEY,
    session_id TEXT NOT NULL REFERENCES sessions(session_id),
    ordinal INTEGER NOT NULL,
    input_prompt TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    UNIQUE(session_id, ordinal)
);

-- 5. Executions, Calls, and Approvals
CREATE TABLE IF NOT EXISTS model_calls (
    model_call_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES agent_runs(run_id),
    provider TEXT NOT NULL,
    model_sku TEXT NOT NULL,
    request_digest TEXT NOT NULL,
    prompt_tokens INTEGER,
    completion_tokens INTEGER,
    cached_tokens INTEGER,
    cost_usd REAL,
    created_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS tool_calls (
    tool_call_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES agent_runs(run_id),
    tool_name TEXT NOT NULL,
    definition_digest TEXT NOT NULL,
    argument_digest TEXT NOT NULL,
    effect_class TEXT NOT NULL,
    status TEXT NOT NULL,
    exit_code INTEGER,
    stdout_hash TEXT,
    stderr_hash TEXT,
    created_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS approvals (
    approval_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES agent_runs(run_id),
    action_digest TEXT NOT NULL,
    scope TEXT NOT NULL,
    decision TEXT NOT NULL,
    decided_by TEXT NOT NULL,
    decided_at_ms INTEGER NOT NULL,
    expires_at_ms INTEGER
);

-- 6. Artifacts, Context Receipts, and Contracts
CREATE TABLE IF NOT EXISTS artifacts (
    artifact_id TEXT PRIMARY KEY,
    digest TEXT NOT NULL UNIQUE,
    size_bytes INTEGER NOT NULL,
    media_type TEXT NOT NULL,
    retention_class TEXT NOT NULL,
    storage_path TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS context_receipts (
    context_receipt_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES agent_runs(run_id),
    turn_ordinal INTEGER NOT NULL,
    content_hashes_json TEXT NOT NULL,
    total_tokens INTEGER NOT NULL,
    truncation_json TEXT,
    created_at_ms INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS acceptance_contracts (
    contract_id TEXT PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES agent_runs(run_id),
    checks_json TEXT NOT NULL,
    forbidden_json TEXT,
    status TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);

-- 7. Distributed Coordination: Leases and Outbox
CREATE TABLE IF NOT EXISTS leases (
    lease_id TEXT PRIMARY KEY,
    worker_id TEXT NOT NULL,
    entity_type TEXT NOT NULL,
    entity_id TEXT NOT NULL,
    heartbeat_ms INTEGER NOT NULL,
    expires_at_ms INTEGER NOT NULL,
    UNIQUE(entity_type, entity_id)
);

CREATE TABLE IF NOT EXISTS outbox (
    outbox_id TEXT PRIMARY KEY,
    destination TEXT NOT NULL,
    payload_json TEXT NOT NULL,
    status TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL
);

-- Seed Initial Migration Record
INSERT OR IGNORE INTO schema_migrations (version, description, applied_at_ms)
VALUES (1, 'Initial schema: event store and materialized projections', 1773792000000);
