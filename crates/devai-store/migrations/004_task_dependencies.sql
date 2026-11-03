-- v3 -> v4: the task dependency graph as a stored platform fact (REQ-01/02/08).
--
-- Today "this task exists because of that one" and "this one cannot continue
-- without that one" live only in the operator's chat. This rung adds the
-- edge table so the relation becomes readable domain data: many-to-many in
-- both directions, typed ('parent' genealogy / 'blocks' continuation gate),
-- attributed (actor + created_at, the decisions.actor precedent).
--
-- ADDITIVE ONLY, in the style of migrations/002: pure CREATE statements —
-- no existing row is rewritten or deleted. Tasks written before v4 have no
-- edge rows and read as tasks with an EMPTY dependency set, never as
-- corrupt (REQ-08 AC2): there is nothing to backfill, the absence of a row
-- IS the empty set.
--
-- Integrity is enforced AT WRITE, not silently on read (REQ-07 AC2):
-- the CHECK rejects self-dependency, the two FKs (foreign_keys=ON) reject
-- dangling endpoint references, the UNIQUE triple rejects the same edge
-- twice — while one (task, depends-on) pair may still hold BOTH kinds at
-- once (REQ-02 AC1). Every statement is idempotent (IF NOT EXISTS) so a
-- hypothetical partial rung re-runs to completion instead of aborting.
CREATE TABLE IF NOT EXISTS task_dependencies(
  id             TEXT PRIMARY KEY,
  task_id        TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  depends_on_id  TEXT NOT NULL REFERENCES tasks(id) ON DELETE CASCADE,
  kind           TEXT NOT NULL,
  actor          TEXT NOT NULL,
  created_at     TEXT NOT NULL,
  CHECK (task_id <> depends_on_id),
  UNIQUE (task_id, depends_on_id, kind)
);
CREATE INDEX IF NOT EXISTS task_deps_task ON task_dependencies(task_id);
CREATE INDEX IF NOT EXISTS task_deps_depends_on ON task_dependencies(depends_on_id);
