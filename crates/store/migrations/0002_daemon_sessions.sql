-- Vitna Code Migration 0002: Durable Daemon Sessions and Session-to-Run Mapping
-- Keeps session identity and ordered run membership across daemon restarts without
-- requiring foreign-key rows in workspaces/agent_workspaces that direct sessions
-- do not materialize.

CREATE TABLE IF NOT EXISTS daemon_sessions (
    session_id TEXT PRIMARY KEY,
    workspace_root TEXT NOT NULL,
    created_at_ms INTEGER NOT NULL,
    status TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS daemon_session_runs (
    session_id TEXT NOT NULL REFERENCES daemon_sessions(session_id) ON DELETE CASCADE,
    run_id TEXT NOT NULL,
    ordinal INTEGER NOT NULL,
    PRIMARY KEY (session_id, run_id)
);

CREATE INDEX IF NOT EXISTS idx_daemon_session_runs_order
    ON daemon_session_runs(session_id, ordinal);
