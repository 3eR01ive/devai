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
use rusqlite::{Connection, OptionalExtension, params};
use sha2::{Digest, Sha256};

pub const SCHEMA_VERSION: i64 = 2;

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
        let conn = Connection::open(root.join("devai.db"))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
        if version == 0 {
            conn.execute_batch(include_str!("../schema.sql"))?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        } else if version == 1 {
            // v1 -> v2: additive migration only — new nullable column and a
            // new insert-only table; no existing row is touched.
            conn.execute_batch(include_str!("../migrations/002_workspace_revision.sql"))?;
            conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;
        } else if version != SCHEMA_VERSION {
            return Err(StoreError::SchemaVersion { found: version, expected: SCHEMA_VERSION });
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

    pub fn publish_artifact(&self, draft: ArtifactDraft) -> StoreResult<Artifact> {
        let sha = hex_encode(&Sha256::digest(&draft.content));
        let path = self.blob_path(&sha);
        std::fs::create_dir_all(path.parent().unwrap())?;
        if !path.exists() {
            // write-then-rename keeps the store crash-safe
            let tmp = path.with_extension("tmp");
            std::fs::write(&tmp, &draft.content)?;
            std::fs::rename(&tmp, &path)?;
        }
        let seq: u32 = self
            .conn
            .query_row(
                "SELECT COALESCE(MAX(seq),0)+1 FROM artifacts WHERE task_id=?1 AND kind=?2",
                params![draft.task_id.to_string(), draft.kind.as_str()],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or(1);
        let artifact = Artifact {
            id: new_id(),
            task_id: draft.task_id,
            kind: draft.kind,
            name: draft.name,
            seq,
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
                    "artifact_id": artifact.id.to_string(),
                    "kind": artifact.kind.as_str(),
                    "name": artifact.name,
                    "seq": artifact.seq,
                    "sha256": artifact.sha256,
                    "size": artifact.size,
                    "contract": artifact.contract.as_ref().map(|c| c.to_string()),
                })),
                rows::ts(&artifact.created_at),
            ],
        )?;
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

    /// REQ-002: a v1 database migrates forward ADDITIVELY — the old
    /// execution row survives untouched with a NULL revision, and nothing
    /// pre-existing is rewritten or deleted.
    #[test]
    fn v1_database_migrates_to_v2_without_touching_history() {
        let dir = tempfile::TempDir::new().unwrap();
        let old_id = new_id().to_string();
        let new_id_s = new_id().to_string();
        let task_uuid = new_id().to_string();
        {
            // Hand-build a v1-shaped database: role_executions WITHOUT the
            // workspace_revision column, one historical row.
            let conn = Connection::open(dir.path().join("devai.db")).unwrap();
            conn.execute_batch(
                "PRAGMA user_version=1;
                 CREATE TABLE role_executions(
                   id TEXT PRIMARY KEY, task_id TEXT NOT NULL, role_slug TEXT NOT NULL,
                   role_version INTEGER NOT NULL, agent_name TEXT NULL, status TEXT NOT NULL,
                   inputs TEXT NOT NULL DEFAULT '[]', summary TEXT NULL, error TEXT NULL,
                   started_at TEXT NOT NULL, finished_at TEXT NULL);",
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
