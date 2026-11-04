//! Local persistence for Devai: SQLite database plus a content-addressed
//! artifact filestore, and portable task bundles (export/import).
//!
//! Business rules are NOT implemented here: this crate stores the domain
//! data produced by `devai-core` and enforces a few storage-level invariants
//! (append-only event log, optimistic task locking, content hashes).

pub mod bundle;
pub mod config;
mod error;
mod rows;
pub mod seed;

pub use bundle::{BundleManifest, ImportOutcome};
pub use error::{StoreError, StoreResult};

use std::path::{Path, PathBuf};

use chrono::Utc;
use devai_core::types::*;
use devai_core::workflow::Mutation;
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use sha2::{Digest, Sha256};
use uuid::Uuid;

// The schema ladder is DATA (REQ-004 / arch/registry-in-data v5): the
// catalog — order, `from`/`to`, stable id, SQL script path and apply-handler
// tag per rung — lives in `migrations/manifest.json`; `build.rs` mirrors it
// (plus the script contents, single-binary discipline) into the generated
// file included here, failing the BUILD when the ladder is not continuous
// 1→N or a cited script is missing. `SCHEMA_VERSION` and `SCHEMA_RUNGS_RAW`
// below are that generated data, not hand-written code: adding a schema
// version appends a manifest record + a .sql file and never edits `open()`.
include!(concat!(env!("OUT_DIR"), "/schema_manifest.rs"));

/// Actor prefix on migration events: `migration:<rung id>` — derived from
/// the registry record, so renaming a rung relabels its events consistently.
const MIGRATION_ACTOR_PREFIX: &str = "migration:";

/// Why the supersession backfill flipped rows — cited verbatim in its
/// events so the one-time repair is auditable, not silent (REQ-003).
const SUPERSESSION_BACKFILL_REASON: &str = "publication did not honor supersession before v3: within each (task_id, kind) the max-seq CURRENT row is kept CURRENT, the rest marked SUPERSEDED (status transition only — nothing deleted)";

/// Why the decisions backfill copied rationale out of event payloads —
/// cited verbatim in its fact events (REQ-BF-004): the one-time extraction
/// is auditable, not silent.
const DECISIONS_BACKFILL_REASON: &str = "approve/reject/return rationales lived only in event payloads before the decisions write-path: each undisclosed rationale is copied VERBATIM into a NEW decision row (actor/created_at from the event columns, refs cite the source event id) — existing decisions and events are never touched";

/// The loser predicate of the v3 repair: a CURRENT row that has a strictly
/// better CURRENT sibling in its (task_id, kind) group under the total
/// order (seq, created_at, id). Kept byte-identical to the UPDATE in
/// migrations/003_artifact_supersession.sql: this detection SELECT runs
/// FIRST inside the rung transaction so the repair events can cite exactly
/// the rows the script flips. The tie-break is deterministic (equal seq is
/// resolved by created_at, then by the unique id — a UUIDv7, so text order
/// is time order), which makes the backfill unconditional: every seeded
/// multi-CURRENT group converges to exactly one CURRENT instead of
/// aborting the open (divergence from the WIP draft 5db7fb5, open question
/// "equal-seq rungs" resolved toward repair, not operator intervention).
const MULTI_CURRENT_LOSER_PREDICATE: &str = "status = 'CURRENT' AND EXISTS (\
    SELECT 1 FROM artifacts AS b \
    WHERE b.task_id = artifacts.task_id AND b.kind = artifacts.kind AND b.status = 'CURRENT' \
      AND (b.seq > artifacts.seq \
        OR (b.seq = artifacts.seq AND b.created_at > artifacts.created_at) \
        OR (b.seq = artifacts.seq AND b.created_at = artifacts.created_at AND b.id > artifacts.id)))";

/// One rung of the ladder, materialised from the generated manifest data.
pub struct SchemaRung {
    /// The version this rung upgrades FROM; execution order is manifest order.
    pub from: i64,
    pub to: i64,
    /// Stable id: cited in migration events (`migration:<id>`) and errors.
    pub id: &'static str,
    /// Repository-relative path of the shipped SQL, manifest data (cited
    /// in repair-event payloads for auditability).
    pub script_path: &'static str,
    /// Apply-handler tag from the manifest, resolved through `RUNG_APPLIERS`.
    pub apply: &'static str,
    /// The script content itself, baked from `script_path` at build time
    /// (the single-binary discipline of `include_str!`, via `build.rs`).
    pub script: &'static str,
}

/// Load the ladder as data: one `SchemaRung` per manifest record, in the
/// manifest's order. The generic executor walks this; it knows no rung name.
pub(crate) fn schema_rungs() -> Vec<SchemaRung> {
    SCHEMA_RUNGS_RAW.iter().map(|(id, from, to, script_path, apply, script)| SchemaRung { id, from: *from, to: *to, script_path, apply, script }).collect()
}

/// The apply-handler table — the single place where the ladder stays CODE,
/// and why that is the right boundary (REQ-004 allows `apply` to remain a
/// function pointer only with this justification): pure SQL expresses every
/// rung's DDL/DML verbatim (the `"script"` tag), but SQL CANNOT observe the
/// pre-mutation state BEFORE the UPDATE so the backfill events can cite
/// exactly the rows the script flips (`supersession_backfill`), nor parse a
/// JSON payload / mint UUID ids while appending repair rows
/// (`decisions_backfill`), nor — for future rungs — detect table shape
/// before a rebuild. A new pure-SQL rung
/// appends a manifest record and NO code; only genuinely un-expressible
/// behavior appends a handler here. The tag NAME is manifest data; this
/// table maps name → behavior, failing loud on a tag it cannot run.
type RungApply = fn(&Transaction<'_>, &SchemaRung) -> StoreResult<()>;
const RUNG_APPLIERS: &[(&str, RungApply)] = &[("script", apply_script), ("supersession_backfill", supersession_backfill), ("decisions_backfill", decisions_backfill), ("execution_form_columns", execution_form_columns)];

/// Handle to the local store. `root` contains `devai.db` and `blobs/`.
pub struct Store {
    pub(crate) conn: Connection,
    root: PathBuf,
    pub(crate) blobs: PathBuf,
}

impl Store {
    pub fn open(root: &Path) -> StoreResult<Store> {
        std::fs::create_dir_all(root)?;
        let blobs = root.join("blobs");
        std::fs::create_dir_all(&blobs)?;
        let mut conn = Connection::open(root.join("devai.db"))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == 0 {
            // Fresh home: the current schema is created whole; no rungs run.
            conn.execute_batch(include_str!("../schema.sql"))?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        } else if version > SCHEMA_VERSION {
            return Err(StoreError::SchemaVersion { found: version, expected: SCHEMA_VERSION });
        } else {
            // Generic ladder executor (REQ-004, registry-in-data): walk the
            // manifest rungs from the stored version up to SCHEMA_VERSION.
            // Which rung runs next, what SQL it ships and which handler
            // applies it are READ FROM THE MANIFEST DATA — adding a schema
            // version appends a `migrations/manifest.json` record and never
            // grows a branch here. ONE `BEGIN IMMEDIATE … COMMIT` per rung:
            // the DDL, the backfill UPDATEs, the repair events and the
            // `user_version` bump land atomically together — a crash can
            // never leave a half-repaired group (REQ-003 acceptance "сбой
            // посреди ранга"). Rungs are additionally idempotent, so a
            // database found in a hypothetical partial form (index present,
            // version behind) is carried to completion instead of aborted.
            let rungs = schema_rungs();
            let mut from = version;
            while from < SCHEMA_VERSION {
                let step = rungs.iter().find(|s| s.from == from).ok_or(StoreError::SchemaVersion { found: from, expected: SCHEMA_VERSION })?;
                // Fail loud on a manifest tag with no behavior behind it —
                // never silently skip a rung the catalog promised to run.
                let apply = RUNG_APPLIERS.iter().find(|(name, _)| *name == step.apply).map(|(_, f)| *f).ok_or_else(|| StoreError::Corrupt(format!("manifest rung `{}` names unknown apply handler `{}` — refusing to run a migration it cannot describe", step.id, step.apply)))?;
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                apply(&tx, step)?;
                tx.pragma_update(None, "user_version", step.to)?;
                tx.commit()?;
                from = step.to;
            }
        }
        Ok(Store { conn, root: root.to_path_buf(), blobs })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    // ------------------------------------------------------------------
    // projects
    // ------------------------------------------------------------------

    pub fn create_project(&self, p: &Project) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO projects(id,name,description,repos,created_at,updated_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![p.id.to_string(), p.name, p.description, json_string(&p.repos), rows::ts(&p.created_at), rows::ts(&p.updated_at)],
        )?;
        Ok(())
    }

    pub fn list_projects(&self) -> StoreResult<Vec<Project>> {
        let mut stmt = self.conn.prepare("SELECT id,name,description,repos,created_at,updated_at FROM projects ORDER BY name")?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::project_from_row(r)?);
        }
        Ok(out)
    }

    pub fn get_project(&self, id_or_name: &str) -> StoreResult<Option<Project>> {
        rows::unwrap_row(
            self.conn
                .query_row(
                    "SELECT id,name,description,repos,created_at,updated_at FROM projects WHERE id = ?1 OR name = ?1",
                    params![id_or_name],
                    |r| rows::r2sql(rows::project_from_row(r)),
                )
                .optional(),
        )
    }

    // ------------------------------------------------------------------
    // tasks
    // ------------------------------------------------------------------

    pub fn create_task(&self, t: &Task) -> StoreResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO tasks(id,project_id,title,business_description,state,return_state,requirements_seq,lock_version,created_at,updated_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",
            params![
                t.id.to_string(),
                t.project_id.map(|i| i.to_string()),
                t.title,
                t.business_description,
                t.state.as_str(),
                t.return_state.map(|s| s.as_str()),
                t.requirements_seq,
                t.lock_version as i64,
                rows::ts(&t.created_at),
                rows::ts(&t.updated_at),
            ],
        )?;
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                t.id.to_string(),
                event_kind::TASK_CREATED,
                "user",
                json_string(&serde_json::json!({"title": t.title, "state": t.state.as_str()})),
                rows::ts(&t.created_at),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_task(&self, id: &str) -> StoreResult<Task> {
        rows::unwrap_row(
            self.conn
                .query_row(
                    "SELECT id,project_id,title,business_description,state,return_state,requirements_seq,lock_version,created_at,updated_at FROM tasks WHERE id = ?1",
                    params![id],
                    |r| rows::r2sql(rows::task_from_row(r)),
                )
                .optional(),
        )?
        .ok_or_else(|| StoreError::NotFound("task".into(), id.to_string()))
    }

    /// Resolves a task by a unique id prefix (human-friendly CLI usage).
    pub fn resolve_task(&self, prefix: &str) -> StoreResult<Task> {
        let mut stmt = self.conn.prepare(
            "SELECT id,project_id,title,business_description,state,return_state,requirements_seq,lock_version,created_at,updated_at
             FROM tasks WHERE id = ?1 OR id LIKE ?1 || '%' LIMIT 2",
        )?;
        let mut rows = stmt.query(params![prefix.to_lowercase()])?;
        let mut found: Vec<Task> = Vec::new();
        while let Some(r) = rows.next()? {
            found.push(rows::task_from_row(r)?);
        }
        match found.len() {
            0 => Err(StoreError::NotFound("task".into(), prefix.to_string())),
            1 => Ok(found.pop().unwrap()),
            _ => Err(StoreError::Ambiguous(format!("task id prefix `{prefix}` matches multiple tasks"))),
        }
    }

    pub fn list_tasks(&self) -> StoreResult<Vec<Task>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,project_id,title,business_description,state,return_state,requirements_seq,lock_version,created_at,updated_at
             FROM tasks ORDER BY updated_at DESC",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::task_from_row(r)?);
        }
        Ok(out)
    }

    /// Applies a validated workflow mutation transactionally with an
    /// optimistic concurrency check against `base_lock`.
    pub fn commit(&self, base_lock: u64, m: &Mutation) -> StoreResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        let affected = tx.execute(
            "UPDATE tasks SET state=?1, return_state=?2, requirements_seq=?3, lock_version=?4, updated_at=?5
             WHERE id=?6 AND lock_version=?7",
            params![
                m.task.state.as_str(),
                m.task.return_state.map(|s| s.as_str()),
                m.task.requirements_seq,
                m.task.lock_version as i64,
                rows::ts(&m.task.updated_at),
                m.task.id.to_string(),
                base_lock as i64,
            ],
        )?;
        if affected == 0 {
            drop(tx);
            return Err(StoreError::Conflict {
                what: format!("task {}", m.task.id),
                expected: base_lock,
                found: self.get_task(&m.task.id.to_string())?.lock_version,
            });
        }
        for (id, status) in &m.artifact_status {
            tx.execute("UPDATE artifacts SET status=?1 WHERE id=?2", params![status.as_str(), id.to_string()])?;
        }
        for ev in &m.events {
            tx.execute(
                "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    new_id().to_string(),
                    m.task.id.to_string(),
                    ev.kind,
                    ev.actor,
                    ev.payload.to_string(),
                    rows::ts(&Utc::now()),
                ],
            )?;
        }
        // A mutation that PARKS the task on a blocking question carries the
        // question row itself: it lands in this SAME transaction, so a held
        // task is never left without an answerable question (REQ-RG-02: the
        // empty-rework gate is loud only if the operator can act on it).
        for q in &m.questions {
            tx.execute(
                "INSERT INTO questions(id,task_id,execution_id,text,context,options,blocking,status,answer,answered_by,answered_at,created_at)
                 VALUES(?1,?2,NULL,?3,?4,?5,?6,'OPEN',NULL,NULL,NULL,?7)",
                params![
                    q.id.to_string(),
                    m.task.id.to_string(),
                    q.text,
                    q.context,
                    json_string(&q.options),
                    q.blocking as i64,
                    rows::ts(&Utc::now()),
                ],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    // ------------------------------------------------------------------
    // task dependency graph (REQ-01/02/07)
    // ------------------------------------------------------------------

    /// Shared projection of one ACTIVE stored edge with the JOINed
    /// states/titles of both endpoint tasks. The states are a read-time
    /// projection, never denormalised onto the edge — one query yields
    /// dependencies AND task states (REQ-04 AC1). Callers add their own
    /// `WHERE d.status='ACTIVE' AND ...`: SUPERSEDED rows are history, they
    /// leave every live read (ratified OQ7) but stay in the table forever.
    const DEPENDENCY_EDGE_SELECT: &'static str = "SELECT d.id, d.task_id, t.state, t.title, d.depends_on_id, p.state, p.title, d.kind, d.actor, d.created_at \
        FROM task_dependencies d JOIN tasks t ON t.id = d.task_id JOIN tasks p ON p.id = d.depends_on_id";

    /// Is there already an ACTIVE path `from →*→ to` over edges of `kind`?
    /// Used by `link_task_dependency` to reject cycles WITHIN one kind
    /// (ratified OQ4: the parent graph and the blocks graph are each a DAG)
    /// while a MIXED cycle — the standard backfill/gate case, child `blocks`
    /// its parent while the parent `parent`-links the child — stays legal.
    /// A one-row SQL constraint cannot express transitive reachability, so
    /// the walk is code (the documented data/code boundary of the ladder).
    fn kind_path_exists(&self, from: Id, to: Id, kind: DependencyKind) -> StoreResult<bool> {
        let mut frontier = vec![from.to_string()];
        let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
        seen.insert(from.to_string());
        let target = to.to_string();
        while let Some(node) = frontier.pop() {
            let mut stmt = self
                .conn
                .prepare("SELECT depends_on_id FROM task_dependencies WHERE task_id=?1 AND kind=?2 AND status='ACTIVE'")?;
            let mut rows = stmt.query(params![node, kind.as_str()])?;
            while let Some(r) = rows.next()? {
                let next: String = r.get(0)?;
                if next == target {
                    return Ok(true);
                }
                if seen.insert(next.clone()) {
                    frontier.push(next);
                }
            }
        }
        Ok(false)
    }

    /// Registers a typed dependency edge `task_id → depends_on_id` (REQ-07).
    /// Integrity is LOUD AT WRITE, never silently ignored on read:
    /// self-dependency, a duplicate ACTIVE edge and — ratified OQ4 — an edge
    /// closing a cycle inside one kind are rejected here (the self-check and
    /// the single-ACTIVE partial index pin the shapes again in v4 DDL); a
    /// dangling endpoint surfaces as `NotFound`. The edge stores its author
    /// and creation time (the `decisions.actor` precedent) and lands in
    /// `workflow_events` as `task.dependency.linked` inside ONE transaction
    /// — the relation and its provenance are never half-written.
    pub fn link_task_dependency(&self, task_id: Id, depends_on_id: Id, kind: DependencyKind, actor: &str, now: Timestamp) -> StoreResult<TaskDependency> {
        if task_id == depends_on_id {
            return Err(StoreError::InvalidDependency(format!(
                "task `{task_id}` cannot depend on itself"
            )));
        }
        // Loud reference check before the INSERT (REQ-07 AC2): the dependent
        // and the depended-on task must both exist as platform data.
        let dependent = self.get_task(&task_id.to_string())?;
        let depends_on = self.get_task(&depends_on_id.to_string())?;
        let duplicate: Option<String> = self
            .conn
            .query_row(
                "SELECT id FROM task_dependencies WHERE task_id=?1 AND depends_on_id=?2 AND kind=?3 AND status='ACTIVE'",
                params![task_id.to_string(), depends_on_id.to_string(), kind.as_str()],
                |r| r.get(0),
            )
            .optional()?;
        if duplicate.is_some() {
            return Err(StoreError::InvalidDependency(format!(
                "dependency `{task_id}` —{}→ `{depends_on_id}` already exists",
                kind.as_str()
            )));
        }
        // Ratified OQ4: adding `task_id → depends_on_id` would close a cycle
        // in this kind iff an ACTIVE same-kind path already runs the OTHER
        // way, `depends_on_id →*→ task_id`. Refused loudly, never stored.
        if self.kind_path_exists(depends_on_id, task_id, kind)? {
            return Err(StoreError::InvalidDependency(format!(
                "dependency `{task_id}` —{}→ `{depends_on_id}` would close a {}-cycle; each kind must stay a DAG (mixed parent/blocks cycles are legal)",
                kind.as_str(),
                kind.as_str()
            )));
        }
        let edge = TaskDependency {
            id: new_id(),
            task_id,
            task_state: dependent.state,
            task_title: dependent.title,
            depends_on_id,
            depends_on_state: depends_on.state,
            depends_on_title: depends_on.title,
            kind,
            actor: actor.to_string(),
            created_at: now,
        };
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "INSERT INTO task_dependencies(id,task_id,depends_on_id,kind,status,actor,created_at) VALUES(?1,?2,?3,?4,'ACTIVE',?5,?6)",
            params![edge.id.to_string(), edge.task_id.to_string(), edge.depends_on_id.to_string(), kind.as_str(), edge.actor, rows::ts(&edge.created_at)],
        )?;
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                edge.task_id.to_string(),
                event_kind::TASK_DEPENDENCY_LINKED,
                edge.actor,
                json_string(&serde_json::json!({
                    (json_field::DEPENDENCY_ID): edge.id.to_string(),
                    (json_field::DEPENDS_ON_ID): edge.depends_on_id.to_string(),
                    (json_field::DEPENDENCY_KIND): edge.kind.as_str(),
                })),
                rows::ts(&edge.created_at),
            ],
        )?;
        tx.commit()?;
        Ok(edge)
    }

    /// Outgoing ACTIVE edges of one task — what it depends on (the
    /// resume/accept gate input; REQ-03). A task with no edge rows (every
    /// task created before v4, or one with nothing registered) returns an
    /// EMPTY list, never an error (REQ-08 AC2). SUPERSEDED edges are
    /// history and never appear here (ratified OQ7).
    pub fn task_dependencies(&self, task_id: Id) -> StoreResult<Vec<TaskDependency>> {
        let mut stmt = self
            .conn
            .prepare(&format!("{} WHERE d.status='ACTIVE' AND d.task_id=?1 ORDER BY d.created_at, d.id", Self::DEPENDENCY_EDGE_SELECT))?;
        let mut rows = stmt.query(params![task_id.to_string()])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::dependency_from_row(r)?);
        }
        Ok(out)
    }

    /// Incoming ACTIVE edges of one task — which tasks depend on it. This
    /// is the SAME query shape as the outgoing view: an "epic" needs no
    /// separate entity at all — ratified OQ8 names it by shape over this
    /// ordinary dependency relation (`epic_heads` below, REQ-06 AC2).
    pub fn incoming_dependencies(&self, task_id: Id) -> StoreResult<Vec<TaskDependency>> {
        let mut stmt = self
            .conn
            .prepare(&format!("{} WHERE d.status='ACTIVE' AND d.depends_on_id=?1 ORDER BY d.created_at, d.id", Self::DEPENDENCY_EDGE_SELECT))?;
        let mut rows = stmt.query(params![task_id.to_string()])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::dependency_from_row(r)?);
        }
        Ok(out)
    }

    /// The approved removal semantics of an edge (RATIFIED DECISION OQ7):
    /// "delete" = mark the row SUPERSEDED, never erase. The same-status
    /// pattern the artifacts table set in v3 — history is COMPLETE: the row
    /// keeps its author and created_at and gains who and when superseded
    /// it. Afterwards the live graph no longer sees the edge (a gate it
    /// held opens), a duplicate re-registration may create a fresh ACTIVE
    /// row (the partial unique index covers ACTIVE only), and both rows
    /// stay readable in the table forever. One transaction writes the flip
    /// and the `task.dependency.unlinked` event.
    pub fn supersede_task_dependency(&self, task_id: Id, depends_on_id: Id, kind: DependencyKind, actor: &str, now: Timestamp) -> StoreResult<TaskDependency> {
        let found: Option<TaskDependency> = self
            .conn
            .query_row(
                &format!(
                    "{} WHERE d.status='ACTIVE' AND d.task_id=?1 AND d.depends_on_id=?2 AND d.kind=?3",
                    Self::DEPENDENCY_EDGE_SELECT
                ),
                params![task_id.to_string(), depends_on_id.to_string(), kind.as_str()],
                |r| rows::r2sql(rows::dependency_from_row(r)),
            )
            .optional()?;
        let Some(edge) = found else {
            return Err(StoreError::InvalidDependency(format!(
                "no active `{task_id}` —{}→ `{depends_on_id}` dependency to supersede",
                kind.as_str()
            )));
        };
        let edge_id = edge.id.to_string();
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE task_dependencies SET status='SUPERSEDED', superseded_actor=?2, superseded_at=?3 WHERE id=?1 AND status='ACTIVE'",
            params![edge_id, actor, rows::ts(&now)],
        )?;
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                task_id.to_string(),
                event_kind::TASK_DEPENDENCY_UNLINKED,
                actor,
                json_string(&serde_json::json!({
                    (json_field::DEPENDENCY_ID): edge_id,
                    (json_field::DEPENDS_ON_ID): depends_on_id.to_string(),
                    (json_field::DEPENDENCY_KIND): kind.as_str(),
                })),
                rows::ts(&now),
            ],
        )?;
        tx.commit()?;
        // The caller gets back the edge exactly as it was while ACTIVE.
        Ok(edge)
    }

    /// The epic registry as a VIEW over the stored graph (REQ-06, RATIFIED
    /// DECISION OQ8): epic = vertex with NO outgoing ACTIVE `parent` edge
    /// and AT LEAST ONE incoming one. No epic table, no epic column — the
    /// SAME `task_dependencies` relation and its ACTIVE rows answer, in one
    /// query (REQ-06 AC1/AC2). Blocking edges do not make a vertex an epic:
    /// only the genealogy kind defines the derivation tree.
    pub fn epic_heads(&self) -> StoreResult<Vec<EpicVertex>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.project_id, t.title, t.business_description, t.state, t.return_state,
                    t.requirements_seq, t.lock_version, t.created_at, t.updated_at,
                    (SELECT count(*) FROM task_dependencies c
                      WHERE c.depends_on_id = t.id AND c.kind = 'parent' AND c.status = 'ACTIVE')
             FROM tasks t
             WHERE NOT EXISTS (SELECT 1 FROM task_dependencies p
                                 WHERE p.task_id = t.id AND p.kind = 'parent' AND p.status = 'ACTIVE')
               AND EXISTS (SELECT 1 FROM task_dependencies c
                            WHERE c.depends_on_id = t.id AND c.kind = 'parent' AND c.status = 'ACTIVE')
             ORDER BY t.updated_at DESC, t.id",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(EpicVertex {
                task: rows::task_from_row(r)?,
                derived_children: r.get::<_, i64>(10)? as usize,
            });
        }
        Ok(out)
    }

    /// The WHOLE graph in ONE query: every task node with its state and its
    /// outgoing ACTIVE typed edges (both endpoint states included). This is
    /// what makes the plan computable from platform data alone — a roadmap
    /// consumer takes this set, the ready set is `open_blockers` per node
    /// (REQ-04 AC1/AC2) — and the planning-level "what is happening"
    /// view (REQ-05 AC1). Tasks without edges appear with an empty set;
    /// SUPERSEDED edges join in only as nothing (live graph = ACTIVE).
    pub fn dependency_graph(&self) -> StoreResult<Vec<DependencyNode>> {
        let mut stmt = self.conn.prepare(
            "SELECT t.id, t.project_id, t.title, t.business_description, t.state, t.return_state,
                    t.requirements_seq, t.lock_version, t.created_at, t.updated_at,
                    d.id, d.kind, d.depends_on_id, d.actor, d.created_at, p.state, p.title
             FROM tasks t
             LEFT JOIN task_dependencies d ON d.task_id = t.id AND d.status = 'ACTIVE'
             LEFT JOIN tasks p ON p.id = d.depends_on_id
             ORDER BY t.updated_at DESC, t.id, d.created_at, d.id",
        )?;
        let mut rows = stmt.query([])?;
        let mut out: Vec<DependencyNode> = Vec::new();
        while let Some(r) = rows.next()? {
            let task = rows::task_from_row(r)?;
            let edge = rows::dependency_tail_from_row(r, &task)?;
            match out.last_mut() {
                // Rows of one task are consecutive (the ORDER BY): append
                // the edge to the node instead of emitting the task twice.
                Some(node) if node.task.id == task.id => {
                    if let Some(e) = edge {
                        node.dependencies.push(e);
                    }
                }
                _ => out.push(DependencyNode { task, dependencies: edge.into_iter().collect() }),
            }
        }
        Ok(out)
    }

    // ------------------------------------------------------------------
    // artifacts & content-addressed store
    // ------------------------------------------------------------------

    /// Publishes a new artifact version. The supersession invariant
    /// (REQ-001): every previous CURRENT row of the same (task_id, kind)
    /// group flips to SUPERSEDED inside the SAME transaction as the INSERT
    /// and the events — a crash can never leave zero or two CURRENT rows.
    /// Append-only discipline: only the status column is UPDATEd and event
    /// rows are INSERTed; nothing is deleted, no blob is rewritten.
    pub fn publish_artifact(&self, draft: ArtifactDraft) -> StoreResult<Artifact> {
        let sha = hex_encode(&Sha256::digest(&draft.content));
        let path = self.blob_path(&sha);
        // invariant: why unreachable — `path` is `blob_path(sha)` =
        // `<root>/blobs/<xx>/<yy>/<sha>`: at least three components are
        // appended to a rooted dir, so `parent()` is always `Some` by the
        // shape of the blob layout itself.
        std::fs::create_dir_all(path.parent().unwrap())?;
        if !path.exists() {
            // write-then-rename keeps the store crash-safe
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, &draft.content)?;
            std::fs::rename(&tmp, &path)?;
        }
        let artifact = Artifact {
            id: new_id(),
            task_id: draft.task_id,
            kind: draft.kind,
            name: draft.name,
            // seq: MAX(seq)+1 inside the group, computed inside the publish
            // transaction (was: outside it — the TOCTOU that made equal-seq
            // CURRENT siblings reachable on v2 databases).
            seq: 0,
            category: draft.category,
            status: ArtifactStatus::Current,
            sha256: sha.clone(),
            size: draft.content.len() as u64,
            media_type: draft.media_type,
            contract: draft.contract,
            created_by: draft.created_by,
            execution_id: draft.execution_id,
            depends_on: draft.depends_on,
            created_at: Utc::now(),
        };
        let tx = self.conn.unchecked_transaction()?;
        let seq: u32 = tx
            .query_row(
                "SELECT COALESCE(MAX(seq),0)+1 FROM artifacts WHERE task_id=?1 AND kind=?2",
                params![artifact.task_id.to_string(), artifact.kind.as_str()],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(1);
        let artifact = Artifact { seq, ..artifact };
        // REQ-001: supersede first, then INSERT — the partial UNIQUE index
        // (one CURRENT per (task_id, kind), schema v3) is never transiently
        // violated, and a failed INSERT rolls the flip back with it.
        let superseded: Vec<String> = {
            let mut stmt = tx.prepare(
                "SELECT id FROM artifacts WHERE task_id=?1 AND kind=?2 AND status='CURRENT' ORDER BY seq",
            )?;
            let mut rows = stmt.query(params![artifact.task_id.to_string(), artifact.kind.as_str()])?;
            let mut ids = Vec::new();
            while let Some(r) = rows.next()? {
                ids.push(r.get(0)?);
            }
            ids
        };
        if !superseded.is_empty() {
            tx.execute(
                "UPDATE artifacts SET status='SUPERSEDED' WHERE task_id=?1 AND kind=?2 AND status='CURRENT'",
                params![artifact.task_id.to_string(), artifact.kind.as_str()],
            )?;
        }
        tx.execute(
            "INSERT INTO artifacts(id,task_id,kind,name,seq,category,status,sha256,size,media_type,contract_name,contract_version,created_by,execution_id,depends_on,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16)",
            params![
                artifact.id.to_string(),
                artifact.task_id.to_string(),
                artifact.kind.as_str(),
                artifact.name,
                artifact.seq,
                artifact.category.as_str(),
                artifact.status.as_str(),
                artifact.sha256,
                artifact.size as i64,
                artifact.media_type,
                artifact.contract.as_ref().map(|c| c.name.clone()),
                artifact.contract.as_ref().map(|c| c.version),
                artifact.created_by,
                artifact.execution_id.map(|i| i.to_string()),
                json_string(&artifact.depends_on),
                rows::ts(&artifact.created_at),
            ],
        )?;
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                artifact.task_id.to_string(),
                event_kind::ARTIFACT_PUBLISHED,
                artifact.created_by,
                json_string(&serde_json::json!({
                    (json_field::ARTIFACT_ID): artifact.id.to_string(),
                    (json_field::KIND): artifact.kind.as_str(),
                    (json_field::NAME): artifact.name,
                    (json_field::SEQ): artifact.seq,
                    (json_field::SHA256): artifact.sha256,
                    (json_field::SIZE): artifact.size,
                    (json_field::CONTRACT): artifact.contract.as_ref().map(|c| c.to_string()),
                })),
                rows::ts(&artifact.created_at),
            ],
        )?;
        // REQ-002: exactly one batched supersession event per publication
        // that superseded something — and nothing at all for the first
        // version of a kind (zero SUPERSEDED rows, zero events).
        if !superseded.is_empty() {
            tx.execute(
                "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    new_id().to_string(),
                    artifact.task_id.to_string(),
                    event_kind::ARTIFACT_SUPERSEDED,
                    artifact.created_by,
                    json_string(&serde_json::json!({
                        (json_field::ARTIFACT_ID): artifact.id.to_string(),
                        (json_field::SUPERSEDED): superseded,
                        (json_field::KIND): artifact.kind.as_str(),
                        (json_field::TASK_ID): artifact.task_id.to_string(),
                    })),
                    rows::ts(&artifact.created_at),
                ],
            )?;
        }
        tx.commit()?;
        Ok(artifact)
    }

    pub fn blob_path(&self, sha: &str) -> PathBuf {
        self.blobs.join(&sha[0..2]).join(&sha[2..4]).join(sha)
    }

    pub fn read_blob(&self, sha: &str) -> StoreResult<Vec<u8>> {
        Ok(std::fs::read(self.blob_path(sha))?)
    }

    pub fn artifacts(&self, task_id: Id) -> StoreResult<Vec<Artifact>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,task_id,kind,name,seq,category,status,sha256,size,media_type,contract_name,contract_version,created_by,execution_id,depends_on,created_at
             FROM artifacts WHERE task_id=?1 ORDER BY created_at",
        )?;
        let mut rows = stmt.query(params![task_id.to_string()])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::artifact_from_row(r)?);
        }
        Ok(out)
    }

    pub fn get_artifact(&self, id: &str) -> StoreResult<Artifact> {
        rows::unwrap_row(
            self.conn
                .query_row(
                    "SELECT id,task_id,kind,name,seq,category,status,sha256,size,media_type,contract_name,contract_version,created_by,execution_id,depends_on,created_at
                 FROM artifacts WHERE id = ?1 OR id LIKE ?1 || '%' LIMIT 1",
                    params![id.to_lowercase()],
                    |r| rows::r2sql(rows::artifact_from_row(r)),
                )
                .optional(),
        )?
        .ok_or_else(|| StoreError::NotFound("artifact".into(), id.to_string()))
    }

    // ------------------------------------------------------------------
    // events
    // ------------------------------------------------------------------

    pub fn events(&self, task_id: Id, limit: Option<usize>) -> StoreResult<Vec<WorkflowEvent>> {
        let sql = "SELECT id,task_id,kind,actor,payload,created_at FROM workflow_events WHERE task_id=?1 ORDER BY seq";
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query(params![task_id.to_string()])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            if let Some(l) = limit {
                if out.len() >= l {
                    break;
                }
            }
            out.push(rows::event_from_row(r)?);
        }
        Ok(out)
    }

    pub fn all_events_for_task(&self, task_id: Id) -> StoreResult<Vec<WorkflowEvent>> {
        self.events(task_id, None)
    }

    // ------------------------------------------------------------------
    // roles / agents / contracts
    // ------------------------------------------------------------------

    pub fn put_role(&self, r: &RoleDefinition) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO role_definitions(id,slug,version,name,purpose,instructions,tools,input_contract,output_contract,done_criteria,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11)",
            params![
                r.id.to_string(),
                r.slug,
                r.version,
                r.name,
                r.purpose,
                r.instructions,
                json_string(&r.tools),
                r.input_contract.as_ref().map(|c| c.to_string()),
                r.output_contract.as_ref().map(|c| c.to_string()),
                r.done_criteria,
                rows::ts(&r.created_at),
            ],
        )?;
        Ok(())
    }

    pub fn list_roles(&self) -> StoreResult<Vec<RoleDefinition>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,slug,version,name,purpose,instructions,tools,input_contract,output_contract,done_criteria,created_at
             FROM role_definitions ORDER BY slug, version",
        )?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::role_from_row(r)?);
        }
        Ok(out)
    }

    pub fn get_role(&self, slug: &str) -> StoreResult<Option<RoleDefinition>> {
        // `slug` may be `name@version` or plain name (latest wins)
        let (name, version) = match slug.split_once('@') {
            Some((n, v)) => (n.to_string(), Some(v.parse::<u32>().map_err(|e| StoreError::Corrupt(format!("bad role version: {e}")))?)),
            None => (slug.to_string(), None),
        };
        // Both branches bind two params; the plain-name case passes NULL so the
        // `?2 IS NULL` clause matches every version and ORDER BY picks the latest.
        let sql = match version {
            Some(_) => "SELECT id,slug,version,name,purpose,instructions,tools,input_contract,output_contract,done_criteria,created_at FROM role_definitions WHERE slug=?1 AND version=?2",
            None => "SELECT id,slug,version,name,purpose,instructions,tools,input_contract,output_contract,done_criteria,created_at FROM role_definitions WHERE slug=?1 AND (?2 IS NULL OR version=?2) ORDER BY version DESC LIMIT 1",
        };
        let v = version.map(|x| x.to_string());
        rows::unwrap_row(
            self.conn
                .query_row(sql, rusqlite::params_from_iter([Some(name), v].iter()), |r| rows::r2sql(rows::role_from_row(r)))
                .optional(),
        )
    }

    pub fn put_agent(&self, a: &AgentInstance) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO agent_instances(id,name,provider,model,runtime,created_at) VALUES(?1,?2,?3,?4,?5,?6)
             ON CONFLICT(name) DO UPDATE SET provider=?3, model=?4, runtime=?5",
            params![a.id.to_string(), a.name, a.provider, a.model, a.runtime, rows::ts(&a.created_at)],
        )?;
        Ok(())
    }

    pub fn list_agents(&self) -> StoreResult<Vec<AgentInstance>> {
        let mut stmt = self.conn.prepare("SELECT id,name,provider,model,runtime,created_at FROM agent_instances ORDER BY name")?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::agent_from_row(r)?);
        }
        Ok(out)
    }

    pub fn put_contract(&self, name: &str, version: u32, schema: &str) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO contracts(name,version,schema,created_at) VALUES(?1,?2,?3,?4)
             ON CONFLICT(name,version) DO UPDATE SET schema=?3",
            params![name, version, schema, rows::ts(&Utc::now())],
        )?;
        Ok(())
    }

    pub fn get_contract(&self, name: &str, version: u32) -> StoreResult<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT schema FROM contracts WHERE name=?1 AND version=?2", params![name, version], |r| r.get(0))
            .optional()?)
    }

    pub fn all_contracts(&self) -> StoreResult<Vec<(String, u32, String)>> {
        let mut stmt = self.conn.prepare("SELECT name,version,schema FROM contracts ORDER BY name, version")?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push((r.get(0)?, r.get(1)?, r.get(2)?));
        }
        Ok(out)
    }

    // ------------------------------------------------------------------
    // evaluation profile history (insert-only)
    // ------------------------------------------------------------------

    /// Snapshot one evaluation profile definition. INSERT-only: the first
    /// stored text for `(name, version)` IS the definition of that version —
    /// later in-place edits bump the version and land as a new row, so every
    /// profile identity recorded in an old execution stays resolvable.
    pub fn put_evaluation_profile(&self, name: &str, version: u32, project: &str, body: &str, created_at: &Timestamp) -> StoreResult<bool> {
        let inserted = self.conn.execute(
            "INSERT OR IGNORE INTO evaluation_profiles(name,version,project,body,created_at) VALUES(?1,?2,?3,?4,?5)",
            params![name, version, project, body, rows::ts(created_at)],
        )?;
        Ok(inserted > 0)
    }

    /// Resolve a recorded `name@version` profile identity to its stored body.
    pub fn get_evaluation_profile(&self, name: &str, version: u32) -> StoreResult<Option<(String, String)>> {
        Ok(self
            .conn
            .query_row("SELECT project, body FROM evaluation_profiles WHERE name=?1 AND version=?2", params![name, version], |r| Ok((r.get(0)?, r.get(1)?)))
            .optional()?)
    }

    pub fn list_evaluation_profiles(&self) -> StoreResult<Vec<(String, u32, String)>> {
        let mut stmt = self.conn.prepare("SELECT name, version, project FROM evaluation_profiles ORDER BY name, version")?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push((r.get(0)?, r.get(1)?, r.get(2)?));
        }
        Ok(out)
    }

    // ------------------------------------------------------------------
    // executions
    // ------------------------------------------------------------------

    pub fn start_execution(&self, e: &RoleExecution) -> StoreResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        // The workspace revision is stored as the JSON object the engine
        // computed at execution start. NULL is reserved for rows written
        // before provenance existed and for manual rows without one.
        let revision = e.workspace_revision.as_ref().map(|r| json_string(r));
        tx.execute(
            "INSERT INTO role_executions(id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at,workspace_revision,attempt,parent_execution,failure_class)
             VALUES(?1,?2,?3,?4,?5,?6,?7,NULL,NULL,?8,NULL,?9,?10,?11,NULL)",
            params![
                e.id.to_string(),
                e.task_id.to_string(),
                e.role_slug,
                e.role_version,
                e.agent_name,
                e.status.as_str(),
                json_string(&e.inputs),
                rows::ts(&e.started_at),
                revision,
                e.attempt as i64,
                e.parent_execution.map(|p| p.to_string()),
            ],
        )?;
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                e.task_id.to_string(),
                event_kind::EXECUTION_STARTED,
                format!("role:{}", e.role_slug),
                json_string(&serde_json::json!({
                    "execution_id": e.id.to_string(),
                    "role": e.role_slug,
                    "role_version": e.role_version,
                    "agent": e.agent_name,
                    "workspace_revision": e.workspace_revision,
                })),
                rows::ts(&e.started_at),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn finish_execution(&self, e: &RoleExecution, status: ExecutionStatus, summary: Option<&str>, error: Option<&str>, now: Timestamp) -> StoreResult<()> {
        self.finish_execution_classified(e, status, summary, error, None, now)
    }

    /// `finish_execution` plus the `failure_class` write (REQ-FG-01): WHY a
    /// Failed row failed — `response_form` / `provider` / `platform` (the
    /// `failure_class` vocabulary in core) — so form-vs-content stays
    /// answerable as data, never by parsing the error text.
    pub fn finish_execution_classified(&self, e: &RoleExecution, status: ExecutionStatus, summary: Option<&str>, error: Option<&str>, failure_class: Option<&str>, now: Timestamp) -> StoreResult<()> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE role_executions SET status=?1, summary=?2, error=?3, finished_at=?4, failure_class=COALESCE(?6, failure_class) WHERE id=?5",
            params![status.as_str(), summary, error, rows::ts(&now), e.id.to_string(), failure_class],
        )?;
        let kind = match status {
            ExecutionStatus::Completed => event_kind::EXECUTION_FINISHED,
            ExecutionStatus::Failed => event_kind::EXECUTION_FAILED,
            _ => event_kind::EXECUTION_CANCELLED,
        };
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                e.task_id.to_string(),
                kind,
                format!("role:{}", e.role_slug),
                json_string(&serde_json::json!({
                    "execution_id": e.id.to_string(),
                    "role": e.role_slug,
                    "status": status.as_str(),
                    "summary": summary,
                    "error": error,
                })),
                rows::ts(&now),
            ],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn get_execution(&self, id: &str) -> StoreResult<RoleExecution> {
        rows::unwrap_row(
            self.conn
                .query_row(
                    "SELECT id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at,workspace_revision,attempt,parent_execution,failure_class
                 FROM role_executions WHERE id=?1 OR id LIKE ?1 || '%' LIMIT 1",
                    params![id.to_lowercase()],
                    |r| rows::r2sql(rows::execution_from_row(r)),
                )
                .optional(),
        )?
        .ok_or_else(|| StoreError::NotFound("execution".into(), id.to_string()))
    }

    pub fn list_executions(&self, task_id: Option<Id>) -> StoreResult<Vec<RoleExecution>> {
        let (sql, args): (&str, Vec<Option<String>>) = match task_id {
            Some(t) => (
                "SELECT id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at,workspace_revision,attempt,parent_execution,failure_class FROM role_executions WHERE task_id=?1 ORDER BY started_at",
                vec![Some(t.to_string())],
            ),
            None => (
                "SELECT id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at,workspace_revision,attempt,parent_execution,failure_class FROM role_executions ORDER BY started_at",
                vec![],
            ),
        };
        let mut stmt = self.conn.prepare(sql)?;
        let mut rows = stmt.query(rusqlite::params_from_iter(args.iter()))?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::execution_from_row(r)?);
        }
        Ok(out)
    }

    // ------------------------------------------------------------------
    // decisions & questions
    // ------------------------------------------------------------------

    pub fn record_decision(&self, d: &Decision) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO decisions(id,task_id,kind,verdict,actor,rationale,refs,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
            params![d.id.to_string(), d.task_id.to_string(), d.kind, d.verdict.as_str(), d.actor, d.rationale, json_string(&d.refs), rows::ts(&d.created_at)],
        )?;
        Ok(())
    }

    pub fn list_decisions(&self, task_id: Id) -> StoreResult<Vec<Decision>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,task_id,kind,verdict,actor,rationale,refs,created_at FROM decisions WHERE task_id=?1 ORDER BY created_at",
        )?;
        let mut rows = stmt.query(params![task_id.to_string()])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::decision_from_row(r)?);
        }
        Ok(out)
    }

    pub fn create_question(&self, q: &Question) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO questions(id,task_id,execution_id,text,context,options,blocking,status,answer,answered_by,answered_at,created_at)
             VALUES(?1,?2,?3,?4,?5,?6,?7,?8,NULL,NULL,NULL,?9)",
            params![
                q.id.to_string(),
                q.task_id.to_string(),
                q.execution_id.map(|i| i.to_string()),
                q.text,
                q.context,
                json_string(&q.options),
                q.blocking as i64,
                q.status.as_str(),
                rows::ts(&q.created_at),
            ],
        )?;
        Ok(())
    }

    pub fn get_question(&self, id: &str) -> StoreResult<Question> {
        rows::unwrap_row(
            self.conn
                .query_row(
                    "SELECT id,task_id,execution_id,text,context,options,blocking,status,answer,answered_by,answered_at,created_at FROM questions WHERE id=?1 OR id LIKE ?1 || '%' LIMIT 1",
                    params![id.to_lowercase()],
                    |r| rows::r2sql(rows::question_from_row(r)),
                )
                .optional(),
        )?
        .ok_or_else(|| StoreError::NotFound("question".into(), id.to_string()))
    }

    pub fn answer_question(&self, id: Id, answer: &str, answered_by: &str, now: Timestamp) -> StoreResult<()> {
        self.conn.execute(
            "UPDATE questions SET status='ANSWERED', answer=?1, answered_by=?2, answered_at=?3 WHERE id=?4",
            params![answer, answered_by, rows::ts(&now), id.to_string()],
        )?;
        Ok(())
    }

    pub fn list_questions(&self, task_id: Id) -> StoreResult<Vec<Question>> {
        let mut stmt = self.conn.prepare(
            "SELECT id,task_id,execution_id,text,context,options,blocking,status,answer,answered_by,answered_at,created_at FROM questions WHERE task_id=?1 ORDER BY created_at",
        )?;
        let mut rows = stmt.query(params![task_id.to_string()])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push(rows::question_from_row(r)?);
        }
        Ok(out)
    }

    // ------------------------------------------------------------------
    // helpers
    // ------------------------------------------------------------------

    pub fn insert_event(&self, task_id: Option<Id>, kind: &str, actor: &str, payload: serde_json::Value) -> StoreResult<()> {
        self.conn.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![new_id().to_string(), task_id.map(|i| i.to_string()), kind, actor, payload.to_string(), rows::ts(&Utc::now())],
        )?;
        Ok(())
    }

    pub fn store_blob(&self, sha: &str, content: &[u8]) -> StoreResult<()> {
        let computed = hex_encode(&Sha256::digest(content));
        if computed != sha {
            return Err(StoreError::Corrupt(format!("blob checksum mismatch: expected {sha}, content hashes to {computed}")));
        }
        let path = self.blob_path(sha);
        std::fs::create_dir_all(path.parent().unwrap())?;
        if !path.exists() {
            std::fs::write(&path, content)?;
        }
        Ok(())
    }

    pub fn has_task(&self, id: &str) -> StoreResult<bool> {
        Ok(self
            .conn
            .query_row("SELECT 1 FROM tasks WHERE id=?1", params![id], |_| Ok(()))
            .optional()?
            .is_some())
    }
}

/// The body of a pure-SQL rung: apply the shipped script verbatim. The
/// executor already wraps it in the rung's transaction.
fn apply_script(tx: &Transaction<'_>, step: &SchemaRung) -> StoreResult<()> {
    tx.execute_batch(step.script)?;
    Ok(())
}

/// Rung v4→v5: the three additive `role_executions` columns of the
/// response-form loop. SQLite has no `ADD COLUMN IF NOT EXISTS`, so the
/// shipped script is executed STATEMENT BY STATEMENT, each `ADD COLUMN`
/// skipped when the live schema already carries that column. This is the
/// manifest's documented code/data boundary — idempotence pure SQL cannot
/// express. It matters in both directions: an interrupted earlier run that
/// applied part of the script, and a fully-shaped database whose version
/// marker was pushed back (the supersession tests rewrite history inside a
/// current-shape file) must both re-land at v5 without a duplicate-column
/// abort. An `ADD COLUMN` statement is recognized textually from the script
/// itself — the .sql file stays the single source of the target shape.
fn execution_form_columns(tx: &Transaction<'_>, step: &SchemaRung) -> StoreResult<()> {
    let existing: Vec<String> = {
        let mut stmt = tx.prepare("SELECT name FROM pragma_table_info('role_executions')")?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    // `--` commentary (Russian included, and its punctuation may contain
    // `;`) is cut from EVERY line BEFORE splitting on statements — comment
    // text must never reach the executor or the token parser.
    let code: String = step
        .script
        .lines()
        .map(|l| match l.find("--") {
            Some(i) => &l[..i],
            None => l,
        })
        .collect::<Vec<_>>()
        .join("\n");
    for stmt in code.split(';') {
        let stmt = stmt.trim();
        if stmt.is_empty() {
            continue;
        }
        if let Some(col) = added_column_name(stmt) {
            if existing.iter().any(|c| c == col) {
                continue;
            }
        }
        tx.execute(stmt, [])?;
    }
    Ok(())
}

/// The column name an `ALTER TABLE … ADD COLUMN <name> …` statement adds,
/// by token scan; `None` for any other statement (those run verbatim —
/// only column-adding statements get the presence check).
fn added_column_name(code: &str) -> Option<&str> {
    let toks: Vec<&str> = code.split_whitespace().collect();
    for w in toks.windows(3) {
        if w[0].eq_ignore_ascii_case("ADD") && w[1].eq_ignore_ascii_case("COLUMN") {
            let name = w[2].trim_end_matches(|c: char| !(c.is_alphanumeric() || c == '_'));
            if !name.is_empty() {
                return Some(name);
            }
        }
    }
    None
}

/// Rung v2→v3 (REQ-003): the one-time repair of pre-existing multi-CURRENT
/// sets (the 2026-11-02 precedent: three CURRENT QUALITY_REPORT rows at
/// once). Runs INSIDE the rung transaction opened by `open()`, so
/// detection, the script's UPDATE + index DDL and the repair events commit
/// atomically with the `user_version` bump: a crash never leaves a
/// half-repaired group. The rows to flip are identified BEFORE the UPDATE
/// so each repaired (task_id, kind) group gets exactly one
/// `task.artifact.superseded` event citing its winner and every flipped id
/// — the repair lands in the event log, not silently. A group that was
/// already honest produces zero UPDATEs and zero events.
fn supersession_backfill(tx: &Transaction<'_>, step: &SchemaRung) -> StoreResult<()> {
    let losers: Vec<(String, String, String)> = {
        let mut stmt = tx.prepare(&format!(
            "SELECT task_id, kind, id FROM artifacts WHERE {MULTI_CURRENT_LOSER_PREDICATE} ORDER BY task_id, kind, seq"
        ))?;
        let mut rows = stmt.query([])?;
        let mut out = Vec::new();
        while let Some(r) = rows.next()? {
            out.push((r.get(0)?, r.get(1)?, r.get(2)?));
        }
        out
    };
    tx.execute_batch(step.script)?;
    let mut by_group: Vec<((String, String), Vec<String>)> = Vec::new();
    for (task_id, kind, id) in losers {
        match by_group.iter_mut().find(|(g, _)| *g == (task_id.clone(), kind.clone())) {
            Some((_, ids)) => ids.push(id),
            None => by_group.push(((task_id, kind), vec![id])),
        }
    }
    for ((task_id, kind), ids) in by_group {
        // The surviving CURRENT of the group after the UPDATE: exactly one
        // (the deterministic max of the (seq, created_at, id) order).
        let winner: String = tx
            .query_row(
                "SELECT id FROM artifacts WHERE task_id=?1 AND kind=?2 AND status='CURRENT'",
                params![task_id, kind],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| StoreError::Corrupt(format!(
                "supersession backfill left group ({task_id}, {kind}) with no CURRENT row — aborted before commit"
            )))?;
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                task_id,
                event_kind::ARTIFACT_SUPERSEDED,
                format!("{MIGRATION_ACTOR_PREFIX}{}", step.id),
                json_string(&serde_json::json!({
                    (json_field::BACKFILL): step.id,
                    (json_field::SCRIPT_PATH): step.script_path,
                    (json_field::TASK_ID): task_id,
                    (json_field::KIND): kind,
                    (json_field::CURRENT_ARTIFACT_ID): winner,
                    (json_field::SUPERSEDED): ids,
                    (json_field::REASON): SUPERSESSION_BACKFILL_REASON,
                })),
                rows::ts(&Utc::now()),
            ],
        )?;
    }
    Ok(())
}

/// Fixed namespace for the deterministic ids of rung 004 (ratified spec
/// OQ2): every created decision row gets `UUIDv5(namespace, source event
/// id)` — a name-based id computed FROM the event id, so a repeated
/// extraction recomputes the same primary key and the same refs, and the
/// partial unique index `decisions_backfill_source_event` (predicated on
/// the v5 id space, which live rows — UUIDv7 — never enter) makes a second
/// insertion of the same event impossible at the database level. The value
/// is the backfill task id and is STABLE FOREVER: changing it would mint a
/// fresh id space that no longer dedups against the rows the first
/// application wrote.
pub const DECISIONS_BACKFILL_NAMESPACE: Uuid = Uuid::from_u128(0x01a1a14e2c0775a292ba905fe9886adc);

/// The deterministic decision id of the backfill (spec OQ2: "детерминированный
/// id decision из id исходного события"). Public so acceptance tests can
/// recompute the expected id instead of trusting the handler's bookkeeping.
pub fn backfill_decision_id(event_id: &str) -> Id {
    Uuid::new_v5(&DECISIONS_BACKFILL_NAMESPACE, event_id.as_bytes())
}

/// Rung v3→v4 (REQ-BF-*): the one-time extraction of historical
/// approve/reject/return acts into `decisions`. Runs INSIDE the rung
/// transaction opened by `open()`, so detection, the NEW decision rows, the
/// fact events and the `user_version` bump commit atomically — a failure
/// writing an event rolls the whole rung back (the 003 precedent, REQ-BF-003
/// AC3). This is CODE because SQL cannot parse the payload JSON, derive the
/// deterministic ids and emit one fact event per (task_id, source event
/// kind) group citing exactly the rows it created (the manifest's
/// documented code/data boundary); the shipped script's DDL still applies
/// verbatim inside the handler. Detection runs BEFORE any mutation in the
/// ratified "already disclosed" form (OQ2/OQ6, predicate style of 003,
/// REQ-BF-005): an event is extracted only when NO decision cites its id in
/// `refs` (re-application matches nothing its own rows already store) and —
/// when the payload carries a string rationale — no decision of the same
/// task already stores that exact text (today's `task approve` records
/// event AND decision beside each other, so its event is skipped). A
/// payload WITHOUT a rationale is still extracted as a NULL-rationale row
/// (ratified OQ4: the fact of the decision must not be lost — the reader
/// renders "(none stored)"). Beyond detection, the hard idempotence is the
/// database's: each insert carries the deterministic id and
/// `ON CONFLICT DO NOTHING` (never INSERT OR IGNORE — the silent-loss
/// precedent), and the affected-row count says whether THIS run created the
/// row, so fact events cite only rows truly created. One extraction NEVER
/// rewrites history: rows are only appended, events are only appended.
/// Verdict and decision kind are derived from the event kind — the payload
/// does not carry a verdict; the kind IS the act (REQ-BF-002); the reject/
/// return kinds are the producer-dictionary constants of ratified OQ3.
/// `actor` and `created_at` are copied from the event COLUMNS verbatim, and
/// every created row cites its source event id in `refs` (OQ6:
/// traceability and the dedup material).
fn decisions_backfill(tx: &Transaction<'_>, step: &SchemaRung) -> StoreResult<()> {
    // Shape detection before any mutation (the code-rung boundary the
    // RUNG_APPLIERS comment anticipates): a database predating the very
    // tables this extraction reads has nothing to backfill — inventing
    // domain rows from absent history is not repair, and a shipped v1
    // fixture shape stays migratable forward. Both tables must be present
    // or the rung is inert.
    let shapes: i64 = tx.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name IN ('workflow_events', 'decisions')", [], |r| r.get(0))?;
    if shapes < 2 {
        return Ok(());
    }
    let candidates: Vec<(String, String, String, String, String, String)> = {
        // Only events of a LIVE task are extractable: `decisions.task_id`
        // carries an FK, and an orphan row (task removed via CASCADE while
        // its events stay, e.g. bundle handling) must not brick `open()`.
        let mut stmt = tx.prepare(
            "SELECT id, task_id, kind, actor, payload, created_at FROM workflow_events \
             WHERE task_id IS NOT NULL \
               AND EXISTS (SELECT 1 FROM tasks tk WHERE tk.id = workflow_events.task_id) \
               AND kind IN (?1, ?2, ?3) ORDER BY seq",
        )?;
        let rows = stmt.query_map(
            params![event_kind::REQUIREMENTS_APPROVED, event_kind::REQUIREMENTS_REJECTED, event_kind::QUALITY_RETURNED],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?, r.get(5)?)),
        )?;
        rows.collect::<Result<Vec<_>, _>>()?
    };
    // The shipped script (additive DDL) lands inside the same transaction.
    tx.execute_batch(step.script)?;

    // Grouped exactly like 003 (ratified OQ5): one fact event per
    // (task_id, source event kind) group that produced rows; a clean
    // database produces zero events.
    let mut groups: Vec<((String, String), Vec<String>, Vec<String>)> = Vec::new();
    for (event_id, task_id, kind, actor, payload, created_at) in candidates {
        let (dec_kind, verdict) = match kind.as_str() {
            k if k == event_kind::REQUIREMENTS_APPROVED => (decision_kind::REQUIREMENTS_APPROVAL, Verdict::Approved),
            k if k == event_kind::REQUIREMENTS_REJECTED => (decision_kind::REQUIREMENTS_REJECTION, Verdict::Rejected),
            k if k == event_kind::QUALITY_RETURNED => (decision_kind::QUALITY_RETURN, Verdict::Returned),
            _ => continue,
        };
        let parsed: serde_json::Value = serde_json::from_str(&payload).map_err(|e| {
            StoreError::Corrupt(format!("workflow_events id={event_id}: payload is not valid JSON ({e}) — refusing to build decisions on unreadable history"))
        })?;
        // Ratified OQ4: a payload WITHOUT a rationale (missing key or JSON
        // null) is still extracted, as a NULL-rationale row — skipping it
        // would lose the fact that the human acted; the reader renders
        // "(none stored)". Nothing is ever invented: the rationale column
        // stores the payload text verbatim, byte-identical, or NULL.
        let rationale: Option<String> = parsed.get(json_field::RATIONALE).and_then(|v| v.as_str()).map(|s| s.to_string());
        // "Already disclosed" detection BEFORE any mutation (REQ-BF-005,
        // predicate style of 003), in the ratified OQ2/OQ6 order:
        // (a) a decision already citing THIS event id in refs — a
        //     re-applied rung matches nothing its own rows stored;
        let refs_json = json_string(&vec![event_id.clone()]);
        let cited: i64 = tx.query_row("SELECT count(*) FROM decisions WHERE refs=?1", params![refs_json], |r| r.get(0))?;
        if cited > 0 {
            continue;
        }
        // (b) for string rationales: a decision of the same task already
        //     storing that exact text — the modern write-path's event is
        //     never backfilled secondarily. NULL rationales have no text to
        //     match; (a) plus the deterministic id dedups them.
        if let Some(text) = &rationale {
            let disclosed: i64 = tx.query_row("SELECT count(*) FROM decisions WHERE task_id=?1 AND rationale=?2", params![task_id, text], |r| r.get(0))?;
            if disclosed > 0 {
                continue;
            }
        }
        // Deterministic id from the source event id (OQ2/OQ6) inserted with
        // ON CONFLICT DO NOTHING (never INSERT OR IGNORE — the silent-loss
        // precedent): the database pins the no-op, the affected-row count
        // tells whether THIS run created the row, so a re-applied rung
        // cites zero rows and writes zero fact events.
        let decision_id = backfill_decision_id(&event_id).to_string();
        let inserted = tx.execute(
            "INSERT INTO decisions(id,task_id,kind,verdict,actor,rationale,refs,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8) ON CONFLICT DO NOTHING",
            params![decision_id, task_id, dec_kind, verdict.as_str(), actor, rationale, refs_json, created_at],
        )?;
        if inserted == 0 {
            continue;
        }
        match groups.iter_mut().find(|((g_task, g_kind), _, _)| *g_task == task_id && *g_kind == kind) {
            Some((_, src, decs)) => {
                src.push(event_id);
                decs.push(decision_id);
            }
            None => groups.push(((task_id, kind), vec![event_id], vec![decision_id])),
        }
    }
    for ((task_id, kind), source_events, decision_ids) in groups {
        tx.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![
                new_id().to_string(),
                task_id,
                event_kind::DECISIONS_BACKFILLED,
                format!("{MIGRATION_ACTOR_PREFIX}{}", step.id),
                json_string(&serde_json::json!({
                    (json_field::BACKFILL): step.id,
                    (json_field::SCRIPT_PATH): step.script_path,
                    (json_field::TASK_ID): task_id,
                    (json_field::KIND): kind,
                    (json_field::SOURCE_EVENTS): source_events,
                    (json_field::DECISION_IDS): decision_ids,
                    (json_field::REASON): DECISIONS_BACKFILL_REASON,
                })),
                rows::ts(&Utc::now()),
            ],
        )?;
    }
    Ok(())
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Input for publishing a new artifact version.
pub struct ArtifactDraft {
    pub task_id: Id,
    pub kind: ArtifactKind,
    pub name: String,
    pub category: DataCategory,
    pub content: Vec<u8>,
    pub media_type: String,
    pub contract: Option<ContractRef>,
    pub created_by: String,
    pub execution_id: Option<Id>,
    pub depends_on: Vec<Id>,
}

/// Best-effort media type from a file name.
pub fn guess_media_type(path: &Path) -> String {
    match path.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("md" | "markdown") => "text/markdown".into(),
        Some("json") => "application/json".into(),
        Some("txt") => "text/plain".into(),
        Some("patch" | "diff") => "text/x-diff".into(),
        Some("html") => "text/html".into(),
        Some("rs" | "py" | "js" | "ts") => "text/x-source".into(),
        _ => "application/octet-stream".into(),
    }
}

#[cfg(test)]
mod trigger_tests {
    use super::*;

    #[test]
    fn event_log_rejects_update_and_delete() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let t = Task::new("t", "d", None, Utc::now());
        store.create_task(&t).unwrap();

        let upd = store.conn.execute("UPDATE workflow_events SET kind='forged' WHERE task_id=?1", params![t.id.to_string()]);
        assert!(upd.is_err(), "events must not be updatable");
        let del = store.conn.execute("DELETE FROM workflow_events WHERE task_id=?1", params![t.id.to_string()]);
        assert!(del.is_err(), "events must not be deletable");
        assert_eq!(store.events(t.id, None).unwrap().len(), 1);
    }
}

#[cfg(test)]
mod migration_tests {
    use super::*;

    /// REQ-002 (of T1): a v1 database migrates forward ADDITIVELY — the old
    /// execution row survives untouched with a NULL revision, and nothing
    /// pre-existing is rewritten or deleted. The v2→v3 supersession rung
    /// walks the same ladder: the fixture carries a v1-shaped `artifacts`
    /// table (every real v1 schema had it), so the backfill detects a
    /// clean group and performs no flips at all.
    #[test]
    fn v1_database_migrates_to_v2_without_touching_history() {
        let dir = tempfile::TempDir::new().unwrap();
        let old_id = new_id().to_string();
        let new_id_s = new_id().to_string();
        let task_uuid = new_id().to_string();
        {
            // Hand-build a v1-shaped database: role_executions WITHOUT the
            // workspace_revision column, one historical row, plus the v1
            // artifacts table (empty — the v3 rung must find nothing).
            let conn = Connection::open(dir.path().join("devai.db")).unwrap();
            conn.execute_batch(
                "PRAGMA user_version=1;
                 CREATE TABLE role_executions(
                   id TEXT PRIMARY KEY, task_id TEXT NOT NULL, role_slug TEXT NOT NULL,
                   role_version INTEGER NOT NULL, agent_name TEXT NULL, status TEXT NOT NULL,
                   inputs TEXT NOT NULL DEFAULT '[]', summary TEXT NULL, error TEXT NULL,
                   started_at TEXT NOT NULL, finished_at TEXT NULL);
                 CREATE TABLE artifacts(
                   id TEXT PRIMARY KEY, task_id TEXT NOT NULL, kind TEXT NOT NULL,
                   name TEXT NOT NULL, seq INTEGER NOT NULL, category TEXT NOT NULL,
                   status TEXT NOT NULL, sha256 TEXT NOT NULL, size INTEGER NOT NULL,
                   media_type TEXT NOT NULL DEFAULT 'application/octet-stream',
                   contract_name TEXT NULL, contract_version INTEGER NULL,
                   created_by TEXT NOT NULL, execution_id TEXT NULL,
                   depends_on TEXT NOT NULL DEFAULT '[]', created_at TEXT NOT NULL);",
            )
            .unwrap();
            conn.execute(
                "INSERT INTO role_executions(id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at)
                 VALUES(?1,?2,'requirements-analyst',1,'mock','COMPLETED','[]','was old',NULL,'2026-01-01T00:00:00.000000Z',NULL)",
                params![old_id, task_uuid],
            )
            .unwrap();
        }

        let store = Store::open(dir.path()).unwrap();
        let version: i64 = store.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(version, SCHEMA_VERSION, "open must migrate v1 forward");

        let e = store.get_execution(&old_id).unwrap();
        assert!(e.workspace_revision.is_none(), "history rows keep a NULL revision — never guessed");
        assert_eq!(e.summary.as_deref(), Some("was old"), "the historical row is byte-intact");

        let count: i64 = store.conn.query_row("SELECT count(*) FROM evaluation_profiles", [], |r| r.get(0)).unwrap();
        assert_eq!(count, 0, "the new table exists and starts empty");

        // The new column accepts v2 writes (a NEW row — history is never
        // updated): append-only in the literal SQL sense.
        store
            .conn
            .execute(
                "INSERT INTO role_executions(id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at,workspace_revision)
                 VALUES(?1,?2,'quality-auditor',2,'mock','COMPLETED','[]',NULL,NULL,'2026-01-02T00:00:00.000000Z',NULL,?3)",
                params![new_id_s, task_uuid, json_string(&serde_json::json!({"git_head":null,"dirty_digest":null,"profile":null,"roles":[]}))],
            )
            .unwrap();
        let e = store.get_execution(&new_id_s).unwrap();
        assert!(e.workspace_revision.is_some(), "v2 rows can carry a revision");
        let old = store.get_execution(&old_id).unwrap();
        assert!(old.workspace_revision.is_none(), "the historical row was NOT rewritten by the v2 write");
    }
}

#[cfg(test)]
mod supersession_tests {
    use super::*;

    fn task(store: &Store) -> Task {
        let t = Task::new("supersession", "fixture", None, Utc::now());
        store.create_task(&t).unwrap();
        t
    }

    fn publish(store: &Store, t: &Task, kind: ArtifactKind, bytes: &[u8]) -> Artifact {
        store
            .publish_artifact(ArtifactDraft {
                task_id: t.id,
                kind,
                name: format!("{kind:?}"),
                category: DataCategory::Artifact,
                content: bytes.to_vec(),
                media_type: "text/plain".into(),
                contract: None,
                created_by: "user".into(),
                execution_id: None,
                depends_on: vec![],
            })
            .unwrap()
    }

    fn status_of(store: &Store, id: Id) -> ArtifactStatus {
        store.get_artifact(&id.to_string()).unwrap().status
    }

    fn superseded_events(store: &Store, task_id: Id) -> Vec<WorkflowEvent> {
        store.events(task_id, None).unwrap().into_iter().filter(|e| e.kind == event_kind::ARTIFACT_SUPERSEDED).collect()
    }

    /// Only the events written by the v2→v3 rung (publish-path events share
    /// the same kind, so repair accounting must filter by actor).
    fn migration_events(store: &Store, task_id: Id) -> Vec<WorkflowEvent> {
        superseded_events(store, task_id)
            .into_iter()
            .filter(|e| e.actor == format!("{MIGRATION_ACTOR_PREFIX}003_artifact_supersession"))
            .collect()
    }

    /// REQ-001 atomicity: if the INSERT (or anything after the supersede
    /// UPDATE) fails, the flip is rolled back with it — never a
    /// zero-CURRENT intermediate, never an orphaned event.
    #[test]
    fn publish_rolls_back_the_supersede_flip_when_insert_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let t = task(&store);
        let v1 = publish(&store, &t, ArtifactKind::Patch, b"v1");
        // Simulate a mid-transaction failure: every artifacts INSERT aborts.
        store
            .conn
            .execute_batch("CREATE TRIGGER test_fail_artifact_insert BEFORE INSERT ON artifacts BEGIN SELECT RAISE(ABORT,'boom'); END;")
            .unwrap();
        let draft = ArtifactDraft {
            task_id: t.id,
            kind: ArtifactKind::Patch,
            name: "Patch".into(),
            category: DataCategory::Artifact,
            content: b"v2".to_vec(),
            media_type: "text/plain".into(),
            contract: None,
            created_by: "user".into(),
            execution_id: None,
            depends_on: vec![],
        };
        assert!(store.publish_artifact(draft).is_err(), "the aborted INSERT must surface");
        store.conn.execute_batch("DROP TRIGGER test_fail_artifact_insert").unwrap();

        assert_eq!(status_of(&store, v1.id), ArtifactStatus::Current, "the rolled-back flip must leave exactly one CURRENT");
        assert_eq!(store.artifacts(t.id).unwrap().len(), 1, "no half-published row survived");
        assert!(superseded_events(&store, t.id).is_empty(), "a rolled-back transaction writes no event");
    }

    /// REQ-001 acceptance (AC2), verbatim: the invariant query itself —
    /// `SELECT task_id, kind, COUNT(*) FROM artifacts WHERE status='CURRENT'
    /// GROUP BY 1,2 HAVING COUNT(*)>1` — must be empty after an arbitrary
    /// (interleaved, multi-kind, multi-task) publication series. The DB-level
    /// pin (partial unique index, schema v3) makes a violation uncommittable;
    /// this test runs the literal detection the task text names.
    #[test]
    fn invariant_query_finds_no_multi_current_group_after_publish_series() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let t1 = task(&store);
        let t2 = {
            let t = Task::new("supersession-2", "fixture", None, Utc::now());
            store.create_task(&t).unwrap();
            t
        };
        // Interleaved series: first versions AND supersessions of several
        // kinds across two tasks, in one mixed sequence.
        for bytes in [b"a", b"b", b"c"] {
            publish(&store, &t1, ArtifactKind::RequirementsSpec, bytes);
            publish(&store, &t1, ArtifactKind::ReviewReport, bytes);
            publish(&store, &t1, ArtifactKind::ImplementationReport, bytes);
            publish(&store, &t1, ArtifactKind::QualityReport, bytes);
            publish(&store, &t2, ArtifactKind::QualityReport, bytes);
            publish(&store, &t2, ArtifactKind::Patch, bytes);
        }
        let mut stmt = store
            .conn
            .prepare(
                "SELECT task_id, kind, COUNT(*) FROM artifacts WHERE status='CURRENT' GROUP BY 1,2 HAVING COUNT(*)>1",
            )
            .unwrap();
        let offenders: Vec<(String, String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap();
        assert!(offenders.is_empty(), "the single-CURRENT invariant query must be empty, found {offenders:?}");
        // History is never deleted: all 18 rows remain readable.
        assert_eq!(store.artifacts(t1.id).unwrap().len(), 12);
        assert_eq!(store.artifacts(t2.id).unwrap().len(), 6);
    }

    /// REQ-003: the v2→v3 rung repairs the 2026-11-02 precedent shape (three
    /// CURRENT QUALITY_REPORT rows) to exactly one CURRENT per group, keeps
    /// history, and records one event per repaired group — "not silently".
    #[test]
    fn v2_multi_current_database_is_repaired_with_events() {
        let dir = tempfile::TempDir::new().unwrap();
        let (task_id, ids, patch_id): (Id, Vec<Id>, Id);
        {
            let store = Store::open(dir.path()).unwrap();
            let t = task(&store);
            let a1 = publish(&store, &t, ArtifactKind::QualityReport, b"q1");
            let a2 = publish(&store, &t, ArtifactKind::QualityReport, b"q2");
            let a3 = publish(&store, &t, ArtifactKind::QualityReport, b"q3");
            let p = publish(&store, &t, ArtifactKind::Patch, b"p1");
            // Recreate the pre-v3 defect: all report rows CURRENT at once,
            // the pin removed, version pushed back to v2.
            store
                .conn
                .execute_batch("DROP INDEX artifacts_single_current; UPDATE artifacts SET status='CURRENT'; PRAGMA user_version=2;")
                .unwrap();
            task_id = t.id;
            ids = vec![a1.id, a2.id, a3.id];
            patch_id = p.id;
        }
        let store = Store::open(dir.path()).unwrap();

        let arts = store.artifacts(task_id).unwrap();
        // Exactly one CURRENT per group; max-seq survives; nothing deleted.
        let current: Vec<Id> = arts.iter().filter(|a| a.status == ArtifactStatus::Current && a.kind == ArtifactKind::QualityReport).map(|a| a.id).collect();
        assert_eq!(current, vec![ids[2]], "the max-seq row is kept CURRENT");
        assert_eq!(arts.iter().filter(|a| a.status == ArtifactStatus::Superseded && a.kind == ArtifactKind::QualityReport).count(), 2);
        assert_eq!(status_of(&store, patch_id), ArtifactStatus::Current, "an honest group is untouched");
        assert_eq!(arts.len(), 4, "backfill never deletes history");

        // One repair event citing winner + flipped ids, actor `migration:<id>`
        // (the two publish-path events of the seeding remain untouched).
        let evs = migration_events(&store, task_id);
        assert_eq!(evs.len(), 1, "one batched event per repaired group");
        assert_eq!(superseded_events(&store, task_id).len(), 3, "history keeps the two publish-path events too");
        let ev = &evs[0];
        assert_eq!(ev.actor, format!("{MIGRATION_ACTOR_PREFIX}003_artifact_supersession"));
        assert_eq!(ev.payload[json_field::BACKFILL], "003_artifact_supersession");
        assert_eq!(ev.payload[json_field::SCRIPT_PATH], "migrations/003_artifact_supersession.sql");
        assert_eq!(ev.payload[json_field::KIND], "QUALITY_REPORT");
        assert_eq!(ev.payload[json_field::CURRENT_ARTIFACT_ID], ids[2].to_string());
        let flipped: Vec<String> = ev.payload[json_field::SUPERSEDED].as_array().unwrap().iter().map(|v| v.as_str().unwrap().to_string()).collect();
        assert_eq!(flipped, vec![ids[0].to_string(), ids[1].to_string()], "both older CURRENT rows are cited, in seq order");
        assert!(ev.payload[json_field::REASON].as_str().unwrap().contains("nothing deleted"), "the why is recorded, not silent");

        // Idempotence: a second open runs no rung (version already 3) and
        // re-opening on a forced-2 version repairs nothing / emits nothing.
        let v: i64 = store.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION);
        drop(store);
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(migration_events(&store, task_id).len(), 1, "re-open repeats no repair");
    }

    /// REQ-003 edge (open question "equal-seq rungs"): the repair is
    /// deterministic — seq, then created_at, then id (UUIDv7 text order is
    /// time order). No silent coin flip, no operator abort.
    #[test]
    fn backfill_ties_on_seq_and_created_at_break_on_id() {
        let dir = tempfile::TempDir::new().unwrap();
        let (a1, a2);
        {
            let store = Store::open(dir.path()).unwrap();
            let t = task(&store);
            a1 = publish(&store, &t, ArtifactKind::TestReport, b"one");
            a2 = publish(&store, &t, ArtifactKind::TestReport, b"two");
            store
                .conn
                .execute_batch(
                    "DROP INDEX artifacts_single_current;
                     UPDATE artifacts SET status='CURRENT', seq=1, created_at='2026-01-01T00:00:00.000000Z';
                     PRAGMA user_version=2;",
                )
                .unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        // The winner is whichever id is lexicographically greater — the
        // same total order the SQL predicate uses, so the outcome is
        // deterministic per fixture and asserted as such, not guessed.
        let (winner, loser) = if a1.id.to_string() > a2.id.to_string() { (a1.id, a2.id) } else { (a2.id, a1.id) };
        assert_eq!(status_of(&store, winner), ArtifactStatus::Current, "equal (seq, created_at): the greater id wins");
        assert_eq!(status_of(&store, loser), ArtifactStatus::Superseded);
    }

    /// REQ-003 acceptance "чистая база — ноль UPDATE и ноль событий": a
    /// pre-v3 database with honest statuses gets pushed through the rung
    /// with no flips and no events.
    #[test]
    fn backfill_on_clean_database_writes_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        {
            let store = Store::open(dir.path()).unwrap();
            let t = task(&store);
            publish(&store, &t, ArtifactKind::DecisionRecord, b"one");
            publish(&store, &t, ArtifactKind::DecisionRecord, b"two");
            // Honest state (single CURRENT), but version pushed back to v2:
            // the rung must re-run and find nothing to repair.
            store.conn.execute_batch("PRAGMA user_version=2;").unwrap();
            assert_eq!(store.events(t.id, None).unwrap().iter().filter(|e| e.kind == event_kind::ARTIFACT_SUPERSEDED).count(), 1, "the publish-path event of the second version is the only one");
        }
        let store = Store::open(dir.path()).unwrap();
        let t = store.list_tasks().unwrap().pop().unwrap();
        assert_eq!(superseded_events(&store, t.id).len(), 1, "the backfill added no event");
    }

    /// REQ-003 acceptance "сбой посреди ранга": a failure inside the rung
    /// (the repair events cannot be written) rolls back the flips, the
    /// index and the version bump together — no half-repaired group.
    #[test]
    fn backfill_is_atomic_when_the_event_write_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        {
            let store = Store::open(dir.path()).unwrap();
            let t = task(&store);
            publish(&store, &t, ArtifactKind::QualityReport, b"q1");
            publish(&store, &t, ArtifactKind::QualityReport, b"q2");
            store
                .conn
                .execute_batch("DROP INDEX artifacts_single_current; UPDATE artifacts SET status='CURRENT'; PRAGMA user_version=2;")
                .unwrap();
            // Hide the event table so the rung fails AFTER its UPDATE.
            store.conn.execute_batch("ALTER TABLE workflow_events RENAME TO workflow_events_hidden;").unwrap();
        }
        let err = Store::open(dir.path());
        assert!(err.is_err(), "a mid-rung failure must surface");
        drop(err);
        {
            let conn = Connection::open(dir.path().join("devai.db")).unwrap();
            let cur: i64 = conn
                .query_row("SELECT count(*) FROM artifacts WHERE status='CURRENT' AND kind='QUALITY_REPORT'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(cur, 2, "the rolled-back rung left the group exactly as found — half-repair is impossible");
            let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(v, 2, "the version bump rolled back with the repair");
            let idx: i64 = conn
                .query_row("SELECT count(*) FROM sqlite_master WHERE type='index' AND name='artifacts_single_current'", [], |r| r.get(0))
                .unwrap();
            assert_eq!(idx, 0, "the rung's DDL rolled back too");
            conn.execute_batch("ALTER TABLE workflow_events_hidden RENAME TO workflow_events;").unwrap();
        }
        // With the table restored, the next open completes the repair.
        let store = Store::open(dir.path()).unwrap();
        let t = store.list_tasks().unwrap().pop().unwrap();
        assert_eq!(migration_events(&store, t.id).len(), 1, "the retry performed exactly one recorded repair");
        let arts = store.artifacts(t.id).unwrap();
        assert_eq!(arts.iter().filter(|a| a.status == ArtifactStatus::Current).count(), 1, "exactly one CURRENT after the retry");
        let v: i64 = store.conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
        assert_eq!(v, SCHEMA_VERSION, "the version bump landed with the completed rung");
    }

    /// QA return7 п.(г): a corrupt JSON array column is a read error, not
    /// an empty list (depends_on drives the stale closure).
    #[test]
    fn corrupt_array_column_fails_loud_on_read() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let t = task(&store);
        let a = publish(&store, &t, ArtifactKind::Patch, b"p");
        store.conn.execute("UPDATE artifacts SET depends_on='{broken' WHERE id=?1", params![a.id.to_string()]).unwrap();
        assert!(store.get_artifact(&a.id.to_string()).is_err(), "corrupt depends_on must not read as `[]`");
        assert!(store.artifacts(t.id).is_err());
    }

    /// REQ-004: the ladder is manifest DATA — the generated table mirrors
    /// `migrations/manifest.json` field-for-field (ids, ranges, script
    /// paths, apply tags), is continuous 1→SCHEMA_VERSION, and every tag
    /// resolves to a behavior. `SCHEMA_VERSION` itself is the manifest's
    /// top rung, not a hand-maintained const.
    #[test]
    fn schema_registry_is_continuous_data() {
        let rungs = schema_rungs();
        let doc: serde_json::Value = serde_json::from_str(include_str!("../migrations/manifest.json")).unwrap();
        let records = doc["rungs"].as_array().unwrap();
        assert_eq!(rungs.len(), records.len(), "one generated rung per manifest record, in manifest order");
        let mut prev_to = 1; // the ladder starts at the fresh-install version (schema.sql)
        for (rung, r) in rungs.iter().zip(records) {
            assert_eq!(rung.from, prev_to, "ladder continuous 1→N");
            assert_eq!(rung.to, rung.from + 1, "one rung advances exactly one version");
            assert_eq!(rung.id, r["id"].as_str().unwrap(), "id is read from the manifest data");
            assert_eq!(rung.script_path, r["script"].as_str().unwrap(), "script path is read from the manifest data");
            assert_eq!(rung.apply, r["apply"].as_str().unwrap(), "apply tag is read from the manifest data");
            assert!(!rung.script.is_empty() && !rung.id.is_empty());
            // Fail-loud contract: a tag the executor cannot run must never be reachable.
            assert!(RUNG_APPLIERS.iter().any(|(name, _)| *name == rung.apply), "apply tag `{}` of rung `{}` has no handler", rung.apply, rung.id);
            prev_to = rung.to;
        }
        assert_eq!(prev_to, SCHEMA_VERSION, "SCHEMA_VERSION is derived from the manifest top rung");
    }
}

#[cfg(test)]
mod decisions_backfill_tests {
    use super::*;

    fn seed_event(store: &Store, id: &str, task_id: &str, kind: &str, actor: &str, payload: &str, created_at: &str) {
        store.conn.execute(
            "INSERT INTO workflow_events(id,task_id,kind,actor,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",
            params![id, task_id, kind, actor, payload, created_at],
        )
        .unwrap();
    }

    fn payload_of(store: &Store, event_id: &str) -> String {
        store.conn.query_row("SELECT payload FROM workflow_events WHERE id=?1", params![event_id], |r| r.get(0)).unwrap()
    }

    /// REQ-BF-002/004/005 + REQ-BF-007 (store half): historical approve/
    /// reject/return acts are copied VERBATIM into NEW decision rows
    /// (actor/created_at from the event columns, verdict derived from the
    /// kind, deterministic ids from the event ids per ratified OQ2), one
    /// fact event per (task_id, kind) group (OQ5); a NULL-rationale event is
    /// extracted as a NULL row (ratified OQ4) while an already-disclosed
    /// modern approve is skipped; events stay byte-intact; a re-applied rung
    /// is a no-op.
    #[test]
    fn historical_events_backfill_decisions_once_and_idempotently() {
        let dir = tempfile::TempDir::new().unwrap();
        let task_id: Id;
        // Real UUID strings: the event reader parses ids as UUIDs.
        let (e_rej, e_appr, e_ret, e_null, e_modern) = (new_id().to_string(), new_id().to_string(), new_id().to_string(), new_id().to_string(), new_id().to_string());
        {
            let store = Store::open(dir.path()).unwrap();
            let t = Task::new("backfill", "fixture", None, Utc::now());
            store.create_task(&t).unwrap();
            task_id = t.id;
            let tid = t.id.to_string();
            // Pre-REQ-QAD-001 shape: the rationale lives only in the payload.
            seed_event(&store, &e_rej, &tid, event_kind::REQUIREMENTS_REJECTED, "human:legacy", r#"{"rationale":"split OQ1 before approving"}"#, "2026-09-01T00:00:00.000000Z");
            seed_event(&store, &e_appr, &tid, event_kind::REQUIREMENTS_APPROVED, "human:legacy", r#"{"requirements_seq":2,"rationale":"OQ1: keep `task active`, do NOT ship `exec list --active`"}"#, "2026-10-01T00:00:00.000000Z");
            seed_event(&store, &e_ret, &tid, event_kind::QUALITY_RETURNED, "human:legacy", r#"{"rationale":"ships the rejected alternative"}"#, "2026-10-05T00:00:00.000000Z");
            // The NULL-rationale approve IS extracted (ratified OQ4: the
            // fact of the act must not be lost — NULL lands as a NULL row).
            seed_event(&store, &e_null, &tid, event_kind::REQUIREMENTS_APPROVED, "human:legacy", r#"{"requirements_seq":1,"rationale":null}"#, "2026-08-01T00:00:00.000000Z");
            // The MODERN approve is the skip case: its decision row already
            // stands beside the event (the commands.rs write-path shape).
            seed_event(&store, &e_modern, &tid, event_kind::REQUIREMENTS_APPROVED, "human:now", r#"{"rationale":"already a stored decision"}"#, "2026-10-06T00:00:00.000000Z");
            store
                .record_decision(&Decision {
                    id: new_id(),
                    task_id: t.id,
                    kind: decision_kind::REQUIREMENTS_APPROVAL.into(),
                    verdict: Verdict::Approved,
                    actor: "human:now".into(),
                    rationale: Some("already a stored decision".into()),
                    refs: vec!["spec-id".into()],
                    created_at: Utc::now(),
                })
                .unwrap();
            // Pretend this database never saw v4: open() must run rung 004.
            store.conn.execute_batch("PRAGMA user_version=3;").unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), SCHEMA_VERSION, "open migrates v3 forward");
        let idx: i64 = store
            .conn
            .query_row("SELECT count(*) FROM sqlite_master WHERE type='index' AND name IN ('decisions_task','decisions_backfill_source_event')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(idx, 2, "the rung landed both indexes — the partial unique one pins the ratified OQ2 dedup at the database level");

        let decisions = store.list_decisions(task_id).unwrap();
        // The one pre-existing row + all four undisclosed events, the
        // NULL-rationale one included (ratified OQ4); only the modern
        // approve is skipped — its rationale is already disclosed.
        assert_eq!(decisions.len(), 5, "1 existing + 4 backfilled (NULL included), e-modern skipped: {decisions:?}");
        assert!(decisions.iter().all(|d| d.refs.iter().all(|r| *r != e_modern)), "the modern approve event is not extracted secondarily (REQ-BF-005 AC)");
        // Stored chronology (ORDER BY created_at): Aug NULL-approve, Sep
        // reject, Oct approve, Oct return, then the pre-existing live row.
        for (event, row) in [(&e_null, &decisions[0]), (&e_rej, &decisions[1]), (&e_appr, &decisions[2]), (&e_ret, &decisions[3])] {
            assert_eq!(row.id, backfill_decision_id(event), "decision id is DETERMINISTICALLY derived from the source event id (ratified OQ2)");
            assert_eq!(row.refs, vec![event.clone()], "the row cites its source event id in refs (ratified OQ6)");
        }
        assert_eq!(decisions[0].kind, decision_kind::REQUIREMENTS_APPROVAL);
        assert_eq!(decisions[0].verdict, Verdict::Approved);
        assert!(decisions[0].rationale.is_none(), "OQ4: the NULL payload rationale lands as a NULL row, not a skip");
        assert_eq!(decisions[1].kind, decision_kind::REQUIREMENTS_REJECTION);
        assert_eq!(decisions[1].verdict, Verdict::Rejected);
        let appr = &decisions[2];
        assert_eq!(appr.verdict, Verdict::Approved);
        assert_eq!(appr.rationale.as_deref().unwrap(), "OQ1: keep `task active`, do NOT ship `exec list --active`", "rationale copied byte-identical — never paraphrased");
        assert!(appr.rationale.as_deref().unwrap().contains("task active"), "the acceptance string is visible to readers");
        assert_eq!(appr.actor, "human:legacy", "actor comes from the event column");
        let ret = &decisions[3];
        assert_eq!(ret.kind, decision_kind::QUALITY_RETURN);
        assert_eq!(ret.verdict, Verdict::Returned);
        assert_eq!(ret.created_at.to_rfc3339_opts(chrono::SecondsFormat::Micros, true), "2026-10-05T00:00:00.000000Z", "created_at preserved from the event");

        // created_at byte-identity at the column level too (the reader
        // reproduces the stored text, not a re-format).
        let raw: String = store
            .conn
            .query_row("SELECT created_at FROM decisions WHERE id=?1", params![backfill_decision_id(&e_appr).to_string()], |r| r.get(0))
            .unwrap();
        assert_eq!(raw, "2026-10-01T00:00:00.000000Z");

        // One fact event per (task, source kind) group (ratified OQ5) —
        // actor migration:004, payload naming rung/script/reason and the
        // ids of both sides.
        let facts: Vec<WorkflowEvent> = store.events(task_id, None).unwrap().into_iter().filter(|e| e.kind == event_kind::DECISIONS_BACKFILLED).collect();
        assert_eq!(facts.len(), 3, "one event per group: {facts:?}");
        for f in &facts {
            assert_eq!(f.actor, format!("{MIGRATION_ACTOR_PREFIX}005_decisions_backfill"));
            assert_eq!(f.payload[json_field::BACKFILL], "005_decisions_backfill");
            assert_eq!(f.payload[json_field::SCRIPT_PATH], "migrations/005_decisions_backfill.sql");
            assert!(f.payload[json_field::REASON].as_str().unwrap().contains("VERBATIM"), "the why is recorded, not silent");
        }
        let appr_fact = facts.iter().find(|f| f.payload[json_field::KIND] == serde_json::json!(event_kind::REQUIREMENTS_APPROVED)).unwrap();
        assert_eq!(appr_fact.payload[json_field::SOURCE_EVENTS].as_array().unwrap().len(), 2, "e-appr AND e-null were extracted for the approve group");
        assert_eq!(appr_fact.payload[json_field::DECISION_IDS].as_array().unwrap().len(), 2);

        // REQ-BF-003: the source events are untouched history.
        assert_eq!(payload_of(&store, &e_appr), r#"{"requirements_seq":2,"rationale":"OQ1: keep `task active`, do NOT ship `exec list --active`"}"#);
        assert_eq!(payload_of(&store, &e_modern), r#"{"rationale":"already a stored decision"}"#);

        // Idempotence (REQ-BF-005, ratified OQ2): force the version back,
        // re-open — the deterministic ids recompute identically, the ON
        // CONFLICT DO NOTHING inserts affect no row, no group produces a
        // fact event.
        drop(store);
        {
            let conn = Connection::open(dir.path().join("devai.db")).unwrap();
            conn.execute_batch("PRAGMA user_version=3;").unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.list_decisions(task_id).unwrap().len(), 5, "a re-applied rung creates no decision row");
        assert_eq!(store.events(task_id, None).unwrap().iter().filter(|e| e.kind == event_kind::DECISIONS_BACKFILLED).count(), 3, "a re-applied rung creates no fact event");
    }

    /// The ratified OQ2 mechanism, pinned: the backfill id is a pure
    /// function of the source event id, lands in the UUIDv5 version space
    /// the partial unique index predicates on, never aliases another
    /// event's id, and never collides with the live v7 id space.
    #[test]
    fn backfill_decision_id_is_deterministic_and_lives_in_the_indexed_space() {
        let a = new_id().to_string();
        let b = new_id().to_string();
        assert_eq!(backfill_decision_id(&a), backfill_decision_id(&a), "the same event recomputes the same id");
        assert_ne!(backfill_decision_id(&a), backfill_decision_id(&b), "distinct events get distinct ids");
        let id = backfill_decision_id(&a).to_string();
        assert_eq!(&id[14..15], "5", "backfilled ids enter the v5 space the index selects (substr(id,15,1)='5')");
        assert_eq!(&new_id().to_string()[14..15], "7", "live ids stay OUTSIDE that space — the index never constrains the write-path");
    }

    /// REQ-BF-007 (clean database): no candidate events → zero rows, zero
    /// fact events — the 003 "already honest" precedent.
    #[test]
    fn decisions_backfill_on_clean_database_writes_nothing() {
        let dir = tempfile::TempDir::new().unwrap();
        let task_id;
        {
            let store = Store::open(dir.path()).unwrap();
            let t = Task::new("clean", "fixture", None, Utc::now());
            store.create_task(&t).unwrap();
            task_id = t.id;
            store.conn.execute_batch("PRAGMA user_version=3;").unwrap();
        }
        let store = Store::open(dir.path()).unwrap();
        assert!(store.list_decisions(task_id).unwrap().is_empty());
        assert_eq!(store.events(task_id, None).unwrap().iter().filter(|e| e.kind == event_kind::DECISIONS_BACKFILLED).count(), 0);
    }

    /// REQ-BF-003 AC3 (atomicity, the 003 precedent): if the fact-event
    /// write fails, the created decision rows, the DDL and the version bump
    /// roll back together — a half-backfilled database is impossible; a retry
    /// completes exactly once.
    #[test]
    fn decisions_backfill_is_atomic_when_the_event_write_fails() {
        let dir = tempfile::TempDir::new().unwrap();
        let tid: String;
        let e_a = new_id().to_string();
        {
            let store = Store::open(dir.path()).unwrap();
            let t = Task::new("atomic", "fixture", None, Utc::now());
            store.create_task(&t).unwrap();
            tid = t.id.to_string();
            seed_event(&store, &e_a, &tid, event_kind::REQUIREMENTS_APPROVED, "human:legacy", r#"{"rationale":"keep task active"}"#, "2026-10-01T00:00:00.000000Z");
            // Block every event INSERT — the rung must fail AFTER its rows.
            store.conn.execute_batch("CREATE TRIGGER test_fail_event_insert BEFORE INSERT ON workflow_events BEGIN SELECT RAISE(ABORT,'boom'); END;").unwrap();
            // Drop the fresh-install copies of the rung's indexes so the
            // rollback check below observes the RUNG's DDL, not schema.sql's.
            store.conn.execute_batch("DROP INDEX decisions_task; DROP INDEX decisions_backfill_source_event; PRAGMA user_version=4;").unwrap();
        }
        assert!(Store::open(dir.path()).is_err(), "a mid-rung failure must surface");
        {
            let conn = Connection::open(dir.path().join("devai.db")).unwrap();
            let n: i64 = conn.query_row("SELECT count(*) FROM decisions", [], |r| r.get(0)).unwrap();
            assert_eq!(n, 0, "the rolled-back rung left no decision row");
            let v: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0)).unwrap();
            assert_eq!(v, 4, "the version bump rolled back with the rows");
            let idx: i64 = conn
                .query_row("SELECT count(*) FROM sqlite_master WHERE type='index' AND name IN ('decisions_task','decisions_backfill_source_event')", [], |r| r.get(0))
                .unwrap();
            assert_eq!(idx, 0, "the rung's DDL rolled back too");
            conn.execute_batch("DROP TRIGGER test_fail_event_insert;").unwrap();
        }
        // With writes restored, the next open completes the backfill once.
        let store = Store::open(dir.path()).unwrap();
        assert_eq!(store.conn.query_row("SELECT count(*) FROM decisions WHERE refs LIKE '%' || ?1 || '%'", params![e_a], |r| r.get::<_, i64>(0)).unwrap(), 1, "the retry created exactly one row");
        assert_eq!(store.conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), SCHEMA_VERSION);
        assert_eq!(store.conn.query_row("SELECT count(*) FROM decisions WHERE id=?1", params![backfill_decision_id(&e_a).to_string()], |r| r.get::<_, i64>(0)).unwrap(), 1, "the retried row carries the SAME deterministic id — the retry cannot fork a second row for one event");
        let idx: i64 = store
            .conn
            .query_row("SELECT count(*) FROM sqlite_master WHERE type='index' AND name IN ('decisions_task','decisions_backfill_source_event')", [], |r| r.get(0))
            .unwrap();
        assert_eq!(idx, 2, "the completed retry landed the rung's full DDL");
    }
}
