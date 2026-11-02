-- v2 -> v3: artifact supersession (append-only repair + DB-level pin).
--
-- Defect: publish_artifact hard-coded status='CURRENT' for every new
-- version and never touched the previous rows, so one (task_id, kind)
-- could hold several CURRENT rows at once (incident precedent of
-- 2026-11-02: three QUALITY_REPORT rows seq 1,2,3 published CURRENT).
-- Backfill: within each (task_id, kind) exactly one CURRENT row is kept —
-- the max-seq one; ties on seq (reachable only on pre-v3 databases, seq
-- was computed outside the publish transaction) break deterministically on
-- created_at, then on id (UUIDv7, so lexicographic id order is time order).
-- Every strictly-older row becomes SUPERSEDED. Only the status column
-- changes — no row is deleted, no blob is rewritten (history is never
-- deleted, only marked).
--
-- The predicate below must stay byte-identical to MULTI_CURRENT_PREDICATE
-- in src/lib.rs: the detection SELECT (run first, inside the same rung
-- transaction) cites exactly the rows this UPDATE flips, so the repair is
-- recorded in the event log, not performed silently. The deterministic
-- total order makes the repair unconditional — a seeded multi-CURRENT
-- group always converges to exactly one CURRENT (unlike the earlier WIP
-- draft 5db7fb5, which aborted on an equal-seq tie).
--
-- Every statement is idempotent: the UPDATE predicate re-matches nothing
-- already flipped, `IF NOT EXISTS` lets the index DDL re-run against a
-- hypothetical partial state. The rung commits atomically together with
-- its repair events and the user_version bump (SCHEMA_STEPS executor).
UPDATE artifacts
SET status = 'SUPERSEDED'
WHERE status = 'CURRENT'
  AND EXISTS (
    SELECT 1 FROM artifacts AS b
    WHERE b.task_id = artifacts.task_id
      AND b.kind = artifacts.kind
      AND b.status = 'CURRENT'
      AND (
        b.seq > artifacts.seq
        OR (b.seq = artifacts.seq AND b.created_at > artifacts.created_at)
        OR (b.seq = artifacts.seq AND b.created_at = artifacts.created_at AND b.id > artifacts.id)
      )
  );

-- The single-CURRENT invariant pinned at the database level: after this
-- migration the publish path flips prior CURRENT rows inside the publish
-- transaction (UPDATE before INSERT), so a second CURRENT row for one
-- (task_id, kind) cannot be committed at all — consumers select CURRENT by
-- status alone, no seq tie-break needed. Design source:
-- wip/pre-t2-merge@5db7fb5 migration 003.
CREATE UNIQUE INDEX IF NOT EXISTS artifacts_single_current
  ON artifacts(task_id, kind)
  WHERE status = 'CURRENT';
