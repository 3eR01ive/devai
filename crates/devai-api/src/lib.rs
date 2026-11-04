//! Devai platform API — the typed operation surface every consumer calls
//! instead of touching the store (ratified OQ-1: an in-process library
//! crate; a daemon/network layer is a thin wrapper over this same contract,
//! out of v1 scope).
//!
//! The API owns NO business rules: it invokes `devai_core::workflow` and
//! `devai-runtime` exactly as the engine does (REQ-API-02 C2, store-is-cache
//! — the API reads stored state, never re-derives meaning at read time).
//! The CLI is its first consumer (REQ-API-03); nothing here may assume a
//! terminal, argument parsing, or human formatting (REQ-API-08).
//!
//! Cycle 1 (REQ-API-11) ships the READ plane as working code plus the
//! published operation registry; mutation operations are named by the
//! registry contract and move onto this surface in cycle 2.

mod error;
mod registry;

pub use error::{ApiError, ApiResult, ErrorCode};
pub use registry::{OpClass, OpDescriptor, operations};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use devai_core::types::*;
use devai_core::workflow;
use devai_runtime::liveness;
use devai_store::{Store, bundle};
use serde::Serialize;

/// Borrowed handle to one store home. Cheap to create per command; the
/// single-writer discipline lives in the store, not here.
pub struct Api<'a> {
    store: &'a Store,
    home: PathBuf,
    actor: String,
}

impl<'a> Api<'a> {
    pub fn new(store: &'a Store, home: &Path, actor: &str) -> Api<'a> {
        Api { store, home: home.to_path_buf(), actor: actor.to_string() }
    }

    pub fn actor(&self) -> &str {
        &self.actor
    }

    pub fn home(&self) -> &Path {
        &self.home
    }

    /// EXCEPTION PATH (RATIFIED OQ-10 + REQ-API-03 C2 bootstrap entry): the
    /// store handle is exposed ONLY for `init`/`seed` bootstrap — the store
    /// is opened before it exists and seeding cannot be expressed through
    /// this API — and for the cycle-2-pending mutation handlers. It is NOT a
    /// read channel: every listed read operation of the registry has a typed
    /// method here.
    pub fn store(&self) -> &'a Store {
        self.store
    }

    // ------------------------------------------------------------------
    // registry (REQ-API-06 AC4)
    // ------------------------------------------------------------------

    pub fn api_registry(&self) -> Vec<OpDescriptor> {
        registry::operations()
    }

    // ------------------------------------------------------------------
    // projects
    // ------------------------------------------------------------------

    pub fn project_list(&self) -> ApiResult<Vec<Project>> {
        Ok(self.store.list_projects()?)
    }

    pub fn project_get(&self, id_or_name: &str) -> ApiResult<Option<Project>> {
        Ok(self.store.get_project(id_or_name)?)
    }

    // ------------------------------------------------------------------
    // tasks
    // ------------------------------------------------------------------

    pub fn task_list(&self) -> ApiResult<Vec<Task>> {
        Ok(self.store.list_tasks()?)
    }

    pub fn task_get(&self, id_prefix: &str) -> ApiResult<Task> {
        Ok(self.store.resolve_task(id_prefix)?)
    }

    /// Current state plus the operations the platform permits next.
    pub fn task_status(&self, id_prefix: &str) -> ApiResult<StatusView> {
        let t = self.store.resolve_task(id_prefix)?;
        // allowed_ops depends only on the task row — computed before the row
        // moves into the view (no re-derivation, one read each).
        let ops: Vec<String> = workflow::allowed_ops(&t).into_iter().map(|s| s.to_string()).collect();
        let artifacts = self.store.artifacts(t.id)?;
        let questions = self.store.list_questions(t.id)?;
        Ok(StatusView {
            task: t,
            allowed_operations: ops,
            current_artifacts: artifacts.into_iter().filter(|a| a.status == ArtifactStatus::Current).collect(),
            open_questions: questions.into_iter().filter(|q| q.status == QuestionStatus::Open).collect(),
        })
    }

    /// Pipeline position + running executions, purely from engine data
    /// (the progress renderer owns no state-machine knowledge).
    pub fn task_progress(&self, id_prefix: &str) -> ApiResult<ProgressView> {
        let t = self.store.resolve_task(id_prefix)?;
        let now = Utc::now();
        let order = workflow::pipeline_order();
        let position = workflow::pipeline_position(&t);
        let pos_idx = position.and_then(|p| order.iter().position(|s| *s == p));
        let running: Vec<RunningExecutionView> = self
            .store
            .list_executions(Some(t.id))?
            .into_iter()
            .filter(|e| e.status == ExecutionStatus::Running)
            .map(|e| RunningExecutionView {
                id: e.id.to_string(),
                role: format!("{}@{}", e.role_slug, e.role_version),
                role_slug: e.role_slug,
                role_version: e.role_version,
                agent_name: e.agent_name,
                started_at: e.started_at,
                elapsed_seconds: (now - e.started_at).num_seconds().max(0),
            })
            .collect();
        Ok(ProgressView {
            task: t.clone(),
            snapshot_at: now,
            pipeline: PipelineView {
                order: order.iter().map(|s| s.as_str().to_string()).collect(),
                current: t.state.as_str().to_string(),
                position: position.map(|s| s.as_str().to_string()),
                position_index: pos_idx,
                off_spine: workflow::pipeline_off_spine(&t),
                terminal: t.state.is_terminal(),
            },
            allowed_operations: workflow::allowed_ops(&t).into_iter().map(|s| s.to_string()).collect(),
            running_executions: running,
        })
    }

    /// Cross-task RUNNING snapshot with per-row liveness triage — the
    /// machine form of «что сейчас активно» (REQ-ACT doctrine, REQ-API-07
    /// AC3: orphans are distinguishable by data, not by operator pgrep).
    pub fn task_active(&self) -> ApiResult<ActiveSnapshot> {
        let now = Utc::now();
        let mut running: Vec<RoleExecution> = self
            .store
            .list_executions(None)?
            .into_iter()
            .filter(|e| e.status == ExecutionStatus::Running)
            .collect();
        // Oldest work first, id tie-break keeps snapshots deterministic.
        running.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.id.cmp(&b.id)));

        let mut facts: HashMap<Id, (Task, bool)> = HashMap::new();
        for e in &running {
            if !facts.contains_key(&e.task_id) {
                let t = self.store.resolve_task(&e.task_id.to_string())?;
                let prepared = self
                    .store
                    .all_events_for_task(t.id)?
                    .iter()
                    .any(|ev| ev.kind == event_kind::WORKTREE_PREPARED);
                facts.insert(e.task_id, (t, prepared));
            }
        }

        // One liveness observation per row, computed ONCE (read-only:
        // REQ-SW-06 — detection never writes and never signals a process).
        let rows = running
            .iter()
            .map(|e| {
                let (t, prepared) = &facts[&e.task_id];
                let live = liveness::check_owner(e.owner_pid, e.owner_host.as_deref(), e.owner_process_start);
                let terminal = t.state.is_terminal();
                ActiveRow {
                    id: e.id.to_string(),
                    task_id: e.task_id.to_string(),
                    role: format!("{}@{}", e.role_slug, e.role_version),
                    role_slug: e.role_slug.clone(),
                    role_version: e.role_version,
                    agent_name: e.agent_name.clone(),
                    started_at: e.started_at,
                    elapsed_seconds: (now - e.started_at).num_seconds().max(0),
                    state: t.state.as_str().to_string(),
                    stale: terminal || live == liveness::Liveness::Dead,
                    stale_terminal_task: terminal,
                    stale_dead_owner: live == liveness::Liveness::Dead,
                    owner_pid: e.owner_pid,
                    owner_host: e.owner_host.clone(),
                    owner_liveness: live.as_str().to_string(),
                    branch: prepared.then(|| devai_runtime::worktree::branch_name(e.task_id)),
                    worktree: prepared.then(|| devai_runtime::worktree::path_for(&self.home, e.task_id).display().to_string()),
                }
            })
            .collect();
        Ok(ActiveSnapshot { snapshot_at: now, rows })
    }

    /// The CURRENT quality report the human signs at the QA gate — full
    /// machine body plus the stored rendering (REQ-API-07: observable
    /// outside the session that produced it).
    pub fn task_report(&self, id_prefix: &str) -> ApiResult<QualityReportView> {
        let t = self.store.resolve_task(id_prefix)?;
        let arts = self.store.artifacts(t.id)?;
        let a = arts
            .iter()
            .find(|x| x.kind == ArtifactKind::QualityReport && x.status == ArtifactStatus::Current)
            .ok_or_else(|| ApiError::Store(devai_store::StoreError::NotFound("quality report".into(), format!("task {} has no CURRENT quality report — QA has not run", short_hint(&t.id)))))?;
        let blob = self.store.read_blob(&a.sha256)?;
        let value: serde_json::Value = serde_json::from_slice(&blob).unwrap_or(serde_json::Value::Null);
        let contract = a
            .contract
            .as_ref()
            .map(|c| format!("{}@{}", c.name, c.version))
            .unwrap_or_else(|| "—".into());
        let report_text = value.get("report_text").and_then(|s| s.as_str()).map(|s| s.to_string());
        Ok(QualityReportView {
            task: t.id.to_string(),
            artifact: a.id.to_string(),
            seq: a.seq,
            contract,
            created_at: a.created_at,
            execution: a.execution_id.map(|e| e.to_string()),
            report: value,
            report_text,
        })
    }

    pub fn task_events(&self, id_prefix: &str, limit: Option<usize>) -> ApiResult<Vec<WorkflowEvent>> {
        let t = self.store.resolve_task(id_prefix)?;
        Ok(self.store.events(t.id, limit)?)
    }

    pub fn task_decisions(&self, id_prefix: &str) -> ApiResult<Vec<Decision>> {
        let t = self.store.resolve_task(id_prefix)?;
        Ok(self.store.list_decisions(t.id)?)
    }

    pub fn task_dependencies(&self, id_prefix: &str) -> ApiResult<DependenciesView> {
        let t = self.store.resolve_task(id_prefix)?;
        let outgoing = self.store.task_dependencies(t.id)?;
        let incoming = self.store.incoming_dependencies(t.id)?;
        Ok(DependenciesView { task: t, outgoing, incoming })
    }

    pub fn task_epics(&self) -> ApiResult<Vec<EpicVertex>> {
        Ok(self.store.epic_heads()?)
    }

    pub fn task_graph(&self) -> ApiResult<GraphView> {
        let nodes = self.store.dependency_graph()?;
        let ready: Vec<String> = nodes
            .iter()
            .filter(|n| !n.task.state.is_terminal() && workflow::open_blockers(&n.task, &n.dependencies).is_empty())
            .map(|n| n.task.id.to_string())
            .collect();
        Ok(GraphView { graph: nodes, ready })
    }

    // ------------------------------------------------------------------
    // artifacts
    // ------------------------------------------------------------------

    pub fn artifact_list(&self, id_prefix: &str) -> ApiResult<Vec<Artifact>> {
        let t = self.store.resolve_task(id_prefix)?;
        Ok(self.store.artifacts(t.id)?)
    }

    /// Raw stored bytes — presentation is the consumer's problem (A-4).
    pub fn artifact_bytes(&self, artifact_id: &str) -> ApiResult<Vec<u8>> {
        let a = self.store.get_artifact(artifact_id)?;
        Ok(self.store.read_blob(&a.sha256)?)
    }

    pub fn artifact_text(&self, artifact_id: &str) -> ApiResult<String> {
        let bytes = self.artifact_bytes(artifact_id)?;
        String::from_utf8(bytes).map_err(|_| ApiError::NotUtf8)
    }

    // ------------------------------------------------------------------
    // questions / roles / agents / contracts
    // ------------------------------------------------------------------

    pub fn question_list(&self, id_prefix: &str) -> ApiResult<Vec<Question>> {
        let t = self.store.resolve_task(id_prefix)?;
        Ok(self.store.list_questions(t.id)?)
    }

    pub fn role_list(&self) -> ApiResult<Vec<RoleDefinition>> {
        Ok(self.store.list_roles()?)
    }

    pub fn role_get(&self, slug: &str) -> ApiResult<Option<RoleDefinition>> {
        Ok(self.store.get_role(slug)?)
    }

    pub fn agent_list(&self) -> ApiResult<Vec<AgentInstance>> {
        Ok(self.store.list_agents()?)
    }

    pub fn contract_get(&self, name: &str, version: u32) -> ApiResult<Option<String>> {
        Ok(self.store.get_contract(name, version)?)
    }

    // ------------------------------------------------------------------
    // executions (REQ-API-07: a running process is observable from any
    // consumer without the launching terminal)
    // ------------------------------------------------------------------

    pub fn exec_list(&self, task_ref: Option<&str>) -> ApiResult<Vec<RoleExecution>> {
        let tid = match task_ref {
            Some(id) => Some(self.store.resolve_task(id)?.id),
            None => None,
        };
        Ok(self.store.list_executions(tid)?)
    }

    pub fn exec_get(&self, exec_id: &str) -> ApiResult<RoleExecution> {
        Ok(self.store.get_execution(exec_id)?)
    }

    /// Liveness of the recorded owner of one execution row. Read-only:
    /// never writes, never signals (REQ-SW-06).
    pub fn exec_liveness(&self, exec_id: &str) -> ApiResult<LivenessView> {
        let e = self.store.get_execution(exec_id)?;
        let live = liveness::check_owner(e.owner_pid, e.owner_host.as_deref(), e.owner_process_start);
        Ok(LivenessView {
            id: e.id.to_string(),
            task_id: e.task_id.to_string(),
            status: e.status.as_str().to_string(),
            owner_pid: e.owner_pid,
            owner_host: e.owner_host.clone(),
            owner_process_start: e.owner_process_start,
            liveness: live.as_str().to_string(),
        })
    }

    // ------------------------------------------------------------------
    // export / import
    // ------------------------------------------------------------------

    /// The same bundle path the CLI used directly: identical data and
    /// artifact version identity through the API (REQ-API-02 AC3 — both
    /// paths converge on `bundle::export` over the same store).
    pub fn export_bundle(&self, id_prefix: &str, out_dir: &Path) -> ApiResult<bundle::BundleManifest> {
        let t = self.store.resolve_task(id_prefix)?;
        Ok(bundle::export(self.store, &t, out_dir)?)
    }

    // internal: none — every read above is one store call per fact.
}

fn short_hint(id: &Id) -> String {
    id.to_string()[..12].to_string()
}

// ---------------------------------------------------------------------------
// Typed views (machine-complete outputs; REQ-API-06 AC2)
// ---------------------------------------------------------------------------

#[derive(Serialize, Clone, Debug)]
pub struct StatusView {
    pub task: Task,
    pub allowed_operations: Vec<String>,
    pub current_artifacts: Vec<Artifact>,
    pub open_questions: Vec<Question>,
}

#[derive(Serialize, Clone, Debug)]
pub struct PipelineView {
    pub order: Vec<String>,
    pub current: String,
    pub position: Option<String>,
    pub position_index: Option<usize>,
    pub off_spine: bool,
    pub terminal: bool,
}

#[derive(Serialize, Clone, Debug)]
pub struct RunningExecutionView {
    pub id: String,
    pub role: String,
    pub role_slug: String,
    pub role_version: u32,
    pub agent_name: Option<String>,
    pub started_at: DateTime<Utc>,
    pub elapsed_seconds: i64,
}

#[derive(Serialize, Clone, Debug)]
pub struct ProgressView {
    pub task: Task,
    pub snapshot_at: DateTime<Utc>,
    pub pipeline: PipelineView,
    pub allowed_operations: Vec<String>,
    pub running_executions: Vec<RunningExecutionView>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ActiveRow {
    pub id: String,
    pub task_id: String,
    pub role: String,
    pub role_slug: String,
    pub role_version: u32,
    pub agent_name: Option<String>,
    pub started_at: DateTime<Utc>,
    pub elapsed_seconds: i64,
    pub state: String,
    pub stale: bool,
    pub stale_terminal_task: bool,
    pub stale_dead_owner: bool,
    pub owner_pid: Option<i64>,
    pub owner_host: Option<String>,
    pub owner_liveness: String,
    pub branch: Option<String>,
    pub worktree: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ActiveSnapshot {
    pub snapshot_at: DateTime<Utc>,
    pub rows: Vec<ActiveRow>,
}

#[derive(Serialize, Clone, Debug)]
pub struct QualityReportView {
    pub task: String,
    pub artifact: String,
    pub seq: u32,
    pub contract: String,
    pub created_at: Timestamp,
    pub execution: Option<String>,
    /// The machine body of the report (the stored artifact content).
    pub report: serde_json::Value,
    /// The §2.6 rendering the human signs, when the report carries one.
    pub report_text: Option<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct DependenciesView {
    pub task: Task,
    pub outgoing: Vec<TaskDependency>,
    pub incoming: Vec<TaskDependency>,
}

#[derive(Serialize, Clone, Debug)]
pub struct GraphView {
    pub graph: Vec<DependencyNode>,
    pub ready: Vec<String>,
}

#[derive(Serialize, Clone, Debug)]
pub struct LivenessView {
    pub id: String,
    pub task_id: String,
    pub status: String,
    pub owner_pid: Option<i64>,
    pub owner_host: Option<String>,
    pub owner_process_start: Option<i64>,
    /// "alive" | "dead" | "unknown" — orphan detect as data (REQ-API-07 AC3).
    pub liveness: String,
}
