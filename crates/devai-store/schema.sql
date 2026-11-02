-- Devai local store schema (v2).
-- SQLite is the local system of record; everything is domain data so a task
-- can be restored after a restart and exported for portability.
-- v2 adds workspace provenance: role_executions.workspace_revision and the
-- insert-only evaluation_profiles history. Existing v1 databases are
-- migrated forward by migrations/002_workspace_revision.sql — additive only,
-- no row is rewritten or deleted.

CREATE TABLE projects(
  id           TEXT PRIMARY KEY,
  name         TEXT NOT NULL UNIQUE,
  description  TEXT NOT NULL DEFAULT '',
  repos        TEXT NOT NULL DEFAULT '[]',
  created_at   TEXT NOT NULL,
  updated_at   TEXT NOT NULL
);

CREATE TABLE tasks(
  id                   TEXT PRIMARY KEY,
  project_id           TEXT NULL REFERENCES projects(id),
  title                TEXT NOT NULL,
  business_description TEXT NOT NULL DEFAULT '',
  state                TEXT NOT NULL,
  return_state         TEXT NULL,
  requirements_seq     INTEGER NOT NULL DEFAULT 0,
  lock_version         INTEGER NOT NULL DEFAULT 1,
  created_at           TEXT NOT NULL,
  updated_at           TEXT NOT NULL
);

CREATE TABLE artifacts(
  id               TEXT PRIMARY KEY,
  task_id          TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  kind             TEXT NOT NULL,
  name             TEXT NOT NULL,
  seq              INTEGER NOT NULL,
  category         TEXT NOT NULL,
  status           TEXT NOT NULL,
  sha256           TEXT NOT NULL,
  size             INTEGER NOT NULL,
  media_type       TEXT NOT NULL DEFAULT 'application/octet-stream',
  contract_name    TEXT NULL,
  contract_version INTEGER NULL,
  created_by       TEXT NOT NULL,
  execution_id     TEXT NULL,
  depends_on       TEXT NOT NULL DEFAULT '[]',
  created_at       TEXT NOT NULL
);
CREATE INDEX artifacts_task ON artifacts(task_id);

CREATE TABLE contracts(
  name       TEXT NOT NULL,
  version    INTEGER NOT NULL,
  schema     TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY(name, version)
);

CREATE TABLE role_definitions(
  id              TEXT PRIMARY KEY,
  slug            TEXT NOT NULL,
  version         INTEGER NOT NULL,
  name            TEXT NOT NULL,
  purpose         TEXT NOT NULL DEFAULT '',
  instructions    TEXT NOT NULL DEFAULT '',
  tools           TEXT NOT NULL DEFAULT '[]',
  input_contract  TEXT NULL,
  output_contract TEXT NULL,
  done_criteria   TEXT NOT NULL DEFAULT '',
  created_at      TEXT NOT NULL,
  UNIQUE(slug, version)
);

CREATE TABLE agent_instances(
  id         TEXT PRIMARY KEY,
  name       TEXT NOT NULL UNIQUE,
  provider   TEXT NOT NULL DEFAULT '',
  model      TEXT NOT NULL DEFAULT '',
  runtime    TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL
);

CREATE TABLE role_executions(
  id                TEXT PRIMARY KEY,
  task_id           TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  role_slug         TEXT NOT NULL,
  role_version      INTEGER NOT NULL,
  agent_name        TEXT NULL,
  status            TEXT NOT NULL,
  inputs            TEXT NOT NULL DEFAULT '[]',
  summary           TEXT NULL,
  error             TEXT NULL,
  started_at        TEXT NOT NULL,
  finished_at       TEXT NULL,
  -- Workspace provenance captured platform-side at execution start (JSON:
  -- git_head, dirty_digest, profile name@version, roles slug@version).
  -- Append-only: rows written before v2 keep NULL; nothing is rewritten.
  workspace_revision TEXT NULL
);
CREATE INDEX exec_task ON role_executions(task_id);

-- Evaluation profile history: every (name, version) ever seeded, with the
-- raw YAML body, so a recorded profile identity in an old execution stays
-- resolvable after in-place config edits bump the version. Insert-only
-- (INSERT OR IGNORE): a stored definition is never overwritten.
CREATE TABLE evaluation_profiles(
  name       TEXT NOT NULL,
  version    INTEGER NOT NULL,
  project    TEXT NOT NULL,
  body       TEXT NOT NULL,
  created_at TEXT NOT NULL,
  PRIMARY KEY(name, version)
);

CREATE TABLE decisions(
  id         TEXT PRIMARY KEY,
  task_id    TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  kind       TEXT NOT NULL,
  verdict    TEXT NOT NULL,
  actor      TEXT NOT NULL,
  rationale  TEXT NULL,
  refs       TEXT NOT NULL DEFAULT '[]',
  created_at TEXT NOT NULL
);

CREATE TABLE questions(
  id           TEXT PRIMARY KEY,
  task_id      TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  execution_id TEXT NULL,
  text         TEXT NOT NULL,
  context      TEXT NULL,
  options      TEXT NOT NULL DEFAULT '[]',
  blocking     INTEGER NOT NULL DEFAULT 0,
  status       TEXT NOT NULL,
  answer       TEXT NULL,
  answered_by  TEXT NULL,
  answered_at  TEXT NULL,
  created_at   TEXT NOT NULL
);

-- Immutable process history. Never UPDATE/DELETE: enforced below.
CREATE TABLE workflow_events(
  seq        INTEGER PRIMARY KEY AUTOINCREMENT,
  id         TEXT NOT NULL,
  task_id    TEXT NULL,
  kind       TEXT NOT NULL,
  actor      TEXT NOT NULL,
  payload    TEXT NOT NULL,
  created_at TEXT NOT NULL
);
CREATE INDEX events_task ON workflow_events(task_id, seq);

CREATE TRIGGER wf_events_no_update BEFORE UPDATE ON workflow_events
BEGIN SELECT RAISE(ABORT, 'workflow_events is append-only'); END;
CREATE TRIGGER wf_events_no_delete BEFORE DELETE ON workflow_events
BEGIN SELECT RAISE(ABORT, 'workflow_events is append-only'); END;
