-- v15 -> v16: the token-spend record of a run (task 01a1b851, REQ-USG-01/02).
--
-- ONE row per execution, pinned by the PRIMARY KEY: a re-application of the
-- same run can never double the sum (REQ-USG-01 AC4). The row is written by
-- the PLATFORM, never by a role, and its shape is provider-agnostic — the
-- driver's own report format never leaks into the store.
--
-- `measured` is the honesty bit: 1 = the numbers came from a real usage
-- report; 0 = «расход неизвестен» and prompt/completion/total stay NULL —
-- never zero (REQ-USG-02 AC1: absence is a fact, not a free run).
--
-- Rows written before this rung keep NO row at all. That is not a hole in
-- the sum: the reader counts runs from `role_executions` and treats a run
-- without a usage row as an UNKNOWN run, exactly like a measured=0 row
-- (backfill of history from logs is explicitly out of scope — ratified OQ-7).
--
-- ADDITIVE ONLY: one new table and its read indexes. No existing row is
-- touched.
CREATE TABLE IF NOT EXISTS execution_usage(
  execution_id      TEXT PRIMARY KEY REFERENCES role_executions(id) ON DELETE CASCADE,
  task_id           TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  role_slug         TEXT NOT NULL,
  -- The run number and model-bundle stamp the execution carried at start,
  -- copied here so a usage read does not have to join for the identity the
  -- plan/explanation is built on.
  run_id            TEXT NULL,
  provider          TEXT NULL,
  model             TEXT NULL,
  prompt_tokens     INTEGER NULL,
  completion_tokens INTEGER NULL,
  total_tokens      INTEGER NULL,
  duration_ms       INTEGER NULL,
  -- NULL = the price of this model is unknown (no config price and no cost
  -- in the driver report) — never 0.0 as a placeholder.
  cost_usd          REAL NULL,
  measured          INTEGER NOT NULL,
  unknown_reason    TEXT NULL,
  created_at        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS execution_usage_task ON execution_usage(task_id, created_at);
CREATE INDEX IF NOT EXISTS execution_usage_created ON execution_usage(created_at);
