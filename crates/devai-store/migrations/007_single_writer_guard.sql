-- v6 -> v7: the single-writer guard (REQ-SW-01/03, ratified OQ-1/OQ-2).
--
-- Two additive columns and one partial unique index on role_executions:
--   owner_pid / owner_host / owner_process_start — the owner identity
--   captured platform-side at start (the process that OPENED the row;
--   pid + process start time is the pid-reuse defense ratified by OQ-3).
--   Rows written before v7 keep NULL and read as «owner unknown», never
--   as «owner dead» (REQ-SW-03 AC2).
--   role_executions_single_writer — at most one RUNNING execution per
--   task_id, pinned where the race would actually happen: inside SQLite.
--   Same storage-level discipline as artifacts_single_current (v3): the
--   second RUNNING row cannot be committed on ANY start path (CLI exec
--   start, engine stages, the response-form loop), and the store maps the
--   violation to a named error instead of queueing silently.
--
-- A CREATE UNIQUE INDEX would abort on a database that already carries
-- two RUNNING rows for one task — reachable, because nothing prevented it
-- before v7. That repair (close the duplicates: newest started_at survives,
-- the rest FAILED with the 'single-writer migration' marker plus one fact
-- event per group — the 003 supersession handler's observe-before-mutate
-- shape) is exactly what pure SQL cannot express (timestamped closure with
-- audit events), so this rung's apply tag is `single_writer_repair`: the
-- handler closes the losers FIRST, then executes this script statement by
-- statement — ADD COLUMNs skipped when already present (the 006 idempotence
-- precedent), the index DDL `IF NOT EXISTS` so a re-apply against a partial
-- state re-lands cleanly. The whole rung commits atomically with its repair
-- events and the user_version bump (SCHEMA_STEPS executor).

ALTER TABLE role_executions ADD COLUMN owner_pid INTEGER NULL;
ALTER TABLE role_executions ADD COLUMN owner_host TEXT NULL;
ALTER TABLE role_executions ADD COLUMN owner_process_start INTEGER NULL;

CREATE UNIQUE INDEX IF NOT EXISTS role_executions_single_writer
  ON role_executions(task_id)
  WHERE status = 'RUNNING';
