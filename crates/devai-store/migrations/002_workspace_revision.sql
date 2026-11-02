-- v1 -> v2: workspace revision binding (append-only, additive).
-- Existing role_executions rows keep workspace_revision NULL — history is
-- never rewritten; the freshness gate treats unbound reports as inert and
-- re-audits (an unbound revision can never be proven fresh).
ALTER TABLE role_executions ADD COLUMN workspace_revision TEXT NULL;

CREATE TABLE IF NOT EXISTS evaluation_profiles(
  name       TEXT NOT NULL,
  version    INTEGER NOT NULL,
  project    TEXT NOT NULL,
  body       TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY(name, version)
);
