-- v13 -> v14: the RUN identity of an execution (REQ-AR-04, REQ-AR-07).
--
-- One launch ("прогон") drives a chain of executions (ANALYSIS -> REVIEW ...)
-- inside ONE platform-owned process. Every row of that chain carries the same
-- run id, so «all executions of run N, including the later ones» is answered
-- from stored data alone, without a live caller and without re-deriving the
-- chain from timestamps. The first execution's id IS the run number (the
-- spec leaves the shape to the implementation: ASM-4 / unknowns).
--
-- ADDITIVE ONLY: one nullable column and one addressed index. Rows written
-- before this rung keep NULL — «no run identity recorded», never a guessed
-- one; nothing is rewritten.
ALTER TABLE role_executions ADD COLUMN run_id TEXT NULL;
CREATE INDEX IF NOT EXISTS role_executions_run ON role_executions(run_id);
