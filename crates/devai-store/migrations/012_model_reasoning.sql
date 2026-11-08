-- v11 -> v12: the REASONING LEVEL of the model bundle (task 01a1b71f,
-- REQ-MS-09 — the ratified answer to OQ-6).
--
-- The level is part of the chosen bundle: it is selected together with the
-- LLM-provider and the model, so both the agent record and the run stamp
-- carry it. Without a level the bundle is INCOMPLETE and a run on it does
-- not start — that rule lives in the loader and the engine; this rung only
-- gives the fact a place to live.
--
-- ADDITIVE ONLY: one defaulted column on the agent record and one nullable
-- column on the execution row. No existing row is rewritten. Rows written
-- before this rung keep NULL and read as «связка не записана» (empty level),
-- never as a driver default.
ALTER TABLE agent_instances ADD COLUMN reasoning TEXT NOT NULL DEFAULT '';
ALTER TABLE role_executions ADD COLUMN reasoning TEXT NULL;
