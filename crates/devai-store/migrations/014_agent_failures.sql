-- v13 -> v14: agent failures as stored facts (task 01a1b60c,
-- REQ-AF-01/05/06).
--
--   * `agent_failures` — one evidence row per (task, agent, failure class)
--     inside an anti-spam WINDOW. The WINDOW is the (agent, class) pair:
--     its first failure appends the `agent.failed` event; repeats inside
--     the window (of any task) grow their own row's `window_count` instead
--     of appending events (REQ-AF-06), while each task keeps its own
--     class/message/tail for the progress line (REQ-AF-05 AC3).
--     `tail` is bounded; the full text stays on the execution row.
--   * `agent_failure_series` — the CURRENT consecutive-failure counter of
--     one (task, agent) pair. Only a successful execution of the same
--     agent resets it (REQ-AF-05 AC2/AC6); `parked_at` records that the
--     declared threshold already parked the task.
--
-- ADDITIVE ONLY: two new tables and their read indexes; no existing row or
-- column is touched.
CREATE TABLE IF NOT EXISTS agent_failures(
  id                TEXT PRIMARY KEY,
  task_id           TEXT NOT NULL,
  execution_id      TEXT NULL,
  agent_name        TEXT NOT NULL,
  driver            TEXT NOT NULL DEFAULT '',
  llm_provider      TEXT NOT NULL DEFAULT '',
  model             TEXT NOT NULL DEFAULT '',
  class             TEXT NOT NULL,
  message           TEXT NOT NULL DEFAULT '',
  exit_code         INTEGER NULL,
  duration_ms       INTEGER NULL,
  tail              TEXT NOT NULL DEFAULT '',
  output_empty      INTEGER NOT NULL DEFAULT 0,
  driver_log_tail   TEXT NULL,
  driver_log_note   TEXT NULL,
  basis             TEXT NOT NULL DEFAULT '',
  window_count      INTEGER NOT NULL DEFAULT 1,
  window_started_at TEXT NOT NULL,
  first_at          TEXT NOT NULL,
  last_at           TEXT NOT NULL,
  threshold         INTEGER NOT NULL DEFAULT 0,
  created_at        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS agent_failures_agent
  ON agent_failures(agent_name, last_at);
CREATE INDEX IF NOT EXISTS agent_failures_task
  ON agent_failures(task_id, last_at);

CREATE TABLE IF NOT EXISTS agent_failure_series(
  task_id           TEXT NOT NULL,
  agent_name        TEXT NOT NULL,
  consecutive_count INTEGER NOT NULL,
  first_at          TEXT NOT NULL,
  last_at           TEXT NOT NULL,
  last_class        TEXT NOT NULL,
  parked_at         TEXT NULL,
  PRIMARY KEY(task_id, agent_name)
);
