-- v7 -> v8: the task-creation command registry (REQ-API-02, ratified OQ-1).
--
-- `task new` with an --idempotency-key records the key → task binding here
-- INSIDE the creation transaction: a retry of a create whose response was
-- lost (operator/test never saw the commit confirm) finds the original task
-- registered under the key and returns it instead of minting a duplicate.
--
-- ADDITIVE ONLY, in the style of migrations/002/004: pure CREATE statements —
-- no existing row is rewritten or deleted. Tasks created without a key have
-- no registry row, and their semantics are unchanged (every successful call
-- still creates a new task; the key is the caller-declared identity of the
-- COMMAND, not of the task content).
--
-- The PRIMARY KEY is the dedup guard itself: a second create racing to
-- register the same key violates it and its whole creation transaction
-- rolls back (REQ-API-01 atomicity — never a half-registered effect).
-- Every statement is idempotent (IF NOT EXISTS) so a hypothetical partial
-- rung re-runs to completion instead of aborting.
CREATE TABLE IF NOT EXISTS task_creation_keys(
  idempotency_key TEXT PRIMARY KEY,
  task_id         TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  created_at      TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS task_creation_keys_task ON task_creation_keys(task_id);
