-- v10 -> v11: model bundles (task 01a1b71f, REQ-MS-02/05).
--
-- Three additive facts, no row is rewritten:
--   * `agent_instances.active` — the ONE marked agent record. The partial
--     unique index below makes a second marked row impossible at the
--     database level; the switch moves the marker in the same transaction
--     that writes the record. A record with no marker is «связка не
--     выбрана» — the built-in fallback is never substituted for it.
--   * `role_executions.driver/llm_provider/model` — the RUN STAMP. The
--     platform copies the identity off the agent record at execution start;
--     it is never accepted from a caller and never rewritten, so a later
--     switch cannot change the answer to «what did this run actually use?».
--     Rows written before this rung keep NULL and read as «связка не
--     записана» — never as an empty identity.
--
-- ADDITIVE ONLY: nullable columns, one defaulted column, one index. No
-- existing row is touched.
--
-- Legacy homes: the agent table entered `schema.sql` WITHOUT its own rung, so
-- a home that started below the agents plane may not have it when this rung
-- runs. Create it in its PRE-v11 shape first (no `active` column) and let the
-- ALTER below add the marker — on every other home this is a no-op. The
-- shape matches `schema.sql` exactly, so fresh and migrated homes agree.
CREATE TABLE IF NOT EXISTS agent_instances(
  id         TEXT PRIMARY KEY,
  name       TEXT NOT NULL UNIQUE,
  provider   TEXT NOT NULL DEFAULT '',
  model      TEXT NOT NULL DEFAULT '',
  runtime    TEXT NOT NULL DEFAULT '',
  created_at TEXT NOT NULL
);
ALTER TABLE agent_instances ADD COLUMN active INTEGER NOT NULL DEFAULT 0;
ALTER TABLE role_executions ADD COLUMN driver TEXT NULL;
ALTER TABLE role_executions ADD COLUMN llm_provider TEXT NULL;
ALTER TABLE role_executions ADD COLUMN model TEXT NULL;
CREATE UNIQUE INDEX IF NOT EXISTS agent_instances_single_active
  ON agent_instances(active)
  WHERE active = 1;
