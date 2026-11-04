-- v4 -> v5: the response-form correction loop becomes ROW DATA (OQ-FG-06,
-- ratified: "execution-строка на попытку (счётчик+lineage) — история отказов
-- есть данные"). One RoleExecution row per provider attempt; these three
-- additive columns make the loop reconstructible from rows alone:
--   attempt           1-based counter inside one role stage's loop;
--   parent_execution  lineage back to the FIRST attempt row of the stage;
--   failure_class     WHY a Failed row failed (response_form | provider |
--                     platform) so form-vs-content is answered WITHOUT
--                     parsing error text (REQ-FG-01 AC1).
--
-- ADDITIVE ONLY, in the style of migrations/002/004: no existing row is
-- rewritten. Pre-loop rows read as attempt=1 with NULL parent and NULL
-- class — exactly what they were: one honest attempt, class not recorded.
ALTER TABLE role_executions ADD COLUMN attempt INTEGER NOT NULL DEFAULT 1;
ALTER TABLE role_executions ADD COLUMN parent_execution TEXT NULL;
ALTER TABLE role_executions ADD COLUMN failure_class TEXT NULL;
