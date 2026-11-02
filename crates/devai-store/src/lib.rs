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

/// The schema version this build creates and expects. The ladder of schema
/// versions is DATA (`SCHEMA_STEPS`, REQ-004 / arch-registry-in-data v5):
/// order, `from`/`to`, id and SQL script path per rung live in the
/// manifest; `open()` is a generic executor that walks it and never grows
/// a branch. Bumping this const without registering a matching rung is a
/// compile-time error (the const assertion below the registry).
pub const SCHEMA_VERSION: i64 = 3;

/// Actor prefix on migration events: `migration:<rung id>` — derived from
/// the registry record, so renaming a rung relabels its events consistently.
const MIGRATION_ACTOR_PREFIX: &str = "migration:";

/// Why the supersession backfill flipped rows — cited verbatim in its
/// events so the one-time repair is auditable, not silent (REQ-003).
const SUPERSESSION_BACKFILL_REASON: &str = "publication did not honor supersession before v3: within each (task_id, kind) the max-seq CURRENT row is kept CURRENT, the rest marked SUPERSEDED (status transition only — nothing deleted)";

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

/// One rung of the schema ladder. `apply` is deliberately a function
/// pointer — the code-vs-data boundary of the registry (REQ-004): pure SQL
/// can express every rung's DDL/DML verbatim, but two things cannot be
/// data: (1) observing the pre-repair state BEFORE the mutation so the
/// repair events can cite it (`supersession_backfill`), and (2) — for
/// future rungs — shape detection before a rebuild. `apply` therefore
/// stays code with this justification comment; the rung's identity, order,
/// version range and script are manifest data read by the executor.
type RungApply = fn(&Transaction<'_>, &SchemaStep) -> StoreResult<()>;

struct SchemaStep {
    /// The version this rung upgrades FROM. Its position in SCHEMA_STEPS
    /// is the execution order (the ladder is continuous: from+1 == to).
    from: i64,
    to: i64,
    /// Stable id: cited in migration events (`backfill`) and errors.
    id: &'static str,
    /// Repository-relative path of the shipped SQL, manifest data (cited
    /// in migration event payloads for auditability).
    script_path: &'static str,
    /// The script content itself, baked at compile time from `script_path`
    /// (the single-binary discipline of `include_str!`).
    script: &'static str,
    apply: RungApply,
}

const SCHEMA_STEPS: &[SchemaStep] = &[
    SchemaStep {
        from: 1,
        to: 2,
        id: "002_workspace_revision",
        script_path: "migrations/002_workspace_revision.sql",
        script: include_str!("../migrations/002_workspace_revision.sql"),
        apply: apply_script,
    },
    SchemaStep {
        from: 2,
        to: 3,
        id: "003_artifact_supersession",
        script_path: "migrations/003_artifact_supersession.sql",
        script: include_str!("../migrations/003_artifact_supersession.sql"),
        apply: supersession_backfill,
    },
];

/// The ladder is one rung per step, continuous 1→SCHEMA_VERSION: a version
/// bump without a registered rung, or a rung whose `to` is not `from+1`,
/// fails HERE (compile time), not on some user's `open()`.
const _: () = {
    let mut i = 0;
    while i < SCHEMA_STEPS.len() {
        assert!(SCHEMA_STEPS[i].from + 1 == SCHEMA_STEPS[i].to);
        i += 1;
    }
    assert!(SCHEMA_STEPS.len() as i64 + 1 == SCHEMA_VERSION);
};

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
            // Generic ladder executor (REQ-004, sequence-as-data): walk the
            // registry from the stored version up to SCHEMA_VERSION. ONE
            // `BEGIN IMMEDIATE … COMMIT` per rung: the DDL, the backfill
            // UPDATEs, the repair events and the `user_version` bump land
            // atomically together — a crash can never leave a
            // half-repaired group (REQ-003 acceptance "сбой посреди ранга").
            // Rungs are additionally idempotent, so a database found in a
            // hypothetical partial form (index present, version behind) is
            // carried to completion instead of aborted.
            let mut from = version;
            while from < SCHEMA_VERSION {
                let step = SCHEMA_STEPS.iter().find(|s| s.from == from).ok_or(StoreError::SchemaVersion { found: from, expected: SCHEMA_VERSION })?;
                let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
                (step.apply)(&tx, step)?;
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
        tx.commit()?;
        Ok(())
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
            "INSERT INTO role_executions(id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at,workspace_revision)
             VALUES(?1,?2,?3,?4,?5,?6,?7,NULL,NULL,?8,NULL,?9)",
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
        let tx = self.conn.unchecked_transaction()?;
        tx.execute(
            "UPDATE role_executions SET status=?1, summary=?2, error=?3, finished_at=?4 WHERE id=?5",
            params![status.as_str(), summary, error, rows::ts(&now), e.id.to_string()],
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
                    "SELECT id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at,workspace_revision
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
                "SELECT id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at,workspace_revision FROM role_executions WHERE task_id=?1 ORDER BY started_at",
                vec![Some(t.to_string())],
            ),
            None => (
                "SELECT id,task_id,role_slug,role_version,agent_name,status,inputs,summary,error,started_at,finished_at,workspace_revision FROM role_executions ORDER BY started_at",
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
fn apply_script(tx: &Transaction<'_>, step: &SchemaStep) -> StoreResult<()> {
    tx.execute_batch(step.script)?;
    Ok(())
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
fn supersession_backfill(tx: &Transaction<'_>, step: &SchemaStep) -> StoreResult<()> {
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

    /// REQ-004: the ladder is manifest data — continuous 1→SCHEMA_VERSION.
    #[test]
    fn schema_registry_is_continuous_data() {
        assert_eq!(SCHEMA_STEPS.first().unwrap().from, 1);
        assert_eq!(SCHEMA_STEPS.last().unwrap().to, SCHEMA_VERSION);
        for s in SCHEMA_STEPS {
            assert_eq!(s.from + 1, s.to);
            assert!(!s.id.is_empty() && !s.script.is_empty() && !s.script_path.is_empty());
        }
        // Rungs are found by data, not by if/else: an unregistered hole fails.
        assert!(SCHEMA_STEPS.iter().all(|s| s.from < SCHEMA_VERSION));
    }
}
