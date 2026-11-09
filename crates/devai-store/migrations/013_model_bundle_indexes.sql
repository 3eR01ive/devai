-- v12 -> v13: the model-bundle surface's ADDRESSED reads (task 01a1b71f,
-- R4 quality return).
--
--   * `role_executions_status` — the snapshot's «идущие прогоны» list reads
--     ONLY the RUNNING rows. Without this index the query is a full scan of
--     the execution history on every snapshot; the list must be addressed,
--     not filtered in memory.
--   * `role_executions_stamp` — the status field's second evidence input
--     (newest finished row of one driver/llm_provider/model/reasoning stamp,
--     ordered by finish time) gets an index over exactly the columns and
--     order it is read by.
--
-- ADDITIVE ONLY: indexes, no row and no column is touched.
CREATE INDEX IF NOT EXISTS role_executions_status
  ON role_executions(status);
CREATE INDEX IF NOT EXISTS role_executions_stamp
  ON role_executions(driver, llm_provider, model, reasoning, finished_at);
