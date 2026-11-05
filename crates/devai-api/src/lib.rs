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
//! Cycle 1 (REQ-API-11) shipped the READ plane as working code plus the
//! published operation registry; cycle 2 (REQ-MUT-*) moved the MUTATION
//! plane onto this same surface — every write the CLI used to issue
//! through its own store handle is a typed operation here, and the
//! `Api::store()` accessor is gone (no consumer gets a bypass).
//! Bootstrap (open + seed) is encapsulated in `Api::open`/`seed_config`
//! per the human-ratified cycle-2 answer to Q2: the prohibition on direct
//! store access in consumers is total.

mod error;
mod mutations;
mod registry;

pub use error::{ApiError, ApiResult, ErrorCode};
pub use mutations::{
    AdvanceOutcome, AdvanceReq, AgentAddReq, AnsweredOutcome, ApproveOutcome, AskReq,
    ContractAddReq, ContractRegistration, DependReq, ExecStartReq, ExecStarted, MergeRecordReq,
    MergeResult, MutationResult, OverrideResult, ProjectAddReq, PublishOutcome, PublishReq,
    PublishedRow, QuestionAsked, RatifyResult, ReviseOutcome, ReviseReq, RunOutcome, RunReq,
    TaskCreateReq, TaskCreated, UnlinkResult, WorkflowOp,
};
pub use registry::{OpClass, OpDescriptor, operations};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use devai_core::types::*;
use devai_core::workflow;
use devai_runtime::liveness;
use devai_runtime::{RuntimeError, worktree};
use devai_store::{Store, bundle};
use serde::Serialize;

/// One store home behind the typed operation surface. The `Store` handle
/// is OWNED here and never exposed (REQ-MUT-03): cycle 1 borrowed it from
/// the CLI (the accepted cycle-1 boundary); cycle 2 inverted the
/// ownership — the consumer reaches the home through `Api::open` and every
/// read/write through operations. The single-writer discipline lives in
/// the store, not here.
pub struct Api {
    store: Store,
    home: PathBuf,
    actor: String,
}

impl Api {
    /// Bootstrap entry (the ratified OQ-10 exception, encapsulated per the
    /// human-ratified cycle-2 Q2): opening a home is not an operation of
    /// the registry — it happens before the domain exists — but it is also
    /// no longer a consumer-side `Store::open`. The handle stays inside.
    pub fn open(home: &Path, actor: &str) -> ApiResult<Api> {
        Ok(Api {
            store: Store::open(home)?,
            home: home.to_path_buf(),
            actor: actor.to_string(),
        })
    }

    pub fn actor(&self) -> &str {
        &self.actor
    }

    pub fn home(&self) -> &Path {
        &self.home
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
        let ops: Vec<String> = workflow::allowed_ops(&t)
            .into_iter()
            .map(|s| s.to_string())
            .collect();
        let artifacts = self.store.artifacts(t.id)?;
        let questions = self.store.list_questions(t.id)?;
        Ok(StatusView {
            task: t,
            allowed_operations: ops,
            current_artifacts: artifacts
                .into_iter()
                .filter(|a| a.status == ArtifactStatus::Current)
                .collect(),
            open_questions: questions
                .into_iter()
                .filter(|q| q.status == QuestionStatus::Open)
                .collect(),
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
            allowed_operations: workflow::allowed_ops(&t)
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
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
                let live = liveness::check_owner(
                    e.owner_pid,
                    e.owner_host.as_deref(),
                    e.owner_process_start,
                );
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
                    worktree: prepared.then(|| {
                        devai_runtime::worktree::path_for(&self.home, e.task_id)
                            .display()
                            .to_string()
                    }),
                }
            })
            .collect();
        Ok(ActiveSnapshot {
            snapshot_at: now,
            rows,
        })
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
            .ok_or_else(|| {
                ApiError::Store(devai_store::StoreError::NotFound(
                    "quality report".into(),
                    format!(
                        "task {} has no CURRENT quality report — QA has not run",
                        short_hint(&t.id)
                    ),
                ))
            })?;
        let blob = self.store.read_blob(&a.sha256)?;
        let value: serde_json::Value =
            serde_json::from_slice(&blob).unwrap_or(serde_json::Value::Null);
        let contract = a
            .contract
            .as_ref()
            .map(|c| format!("{}@{}", c.name, c.version))
            .unwrap_or_else(|| "—".into());
        let report_text = value
            .get("report_text")
            .and_then(|s| s.as_str())
            .map(|s| s.to_string());
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

    pub fn task_events(
        &self,
        id_prefix: &str,
        limit: Option<usize>,
    ) -> ApiResult<Vec<WorkflowEvent>> {
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
        Ok(DependenciesView {
            task: t,
            outgoing,
            incoming,
        })
    }

    pub fn task_epics(&self) -> ApiResult<Vec<EpicVertex>> {
        Ok(self.store.epic_heads()?)
    }

    pub fn task_graph(&self) -> ApiResult<GraphView> {
        let nodes = self.store.dependency_graph()?;
        let ready: Vec<String> = nodes
            .iter()
            .filter(|n| {
                !n.task.state.is_terminal()
                    && workflow::open_blockers(&n.task, &n.dependencies).is_empty()
            })
            .map(|n| n.task.id.to_string())
            .collect();
        Ok(GraphView {
            graph: nodes,
            ready,
        })
    }

    // ------------------------------------------------------------------
    // workspace view (REQ-WV-01/02/03/07): the committed changeset of the
    // task's isolated worktree against the stored bound baseline, and the
    // per-file diff. Pure read: stored facts compose the range, read-only
    // git plumbing answers the content — nothing here writes to the store
    // or opens a git write channel (REQ-WV-08).
    // ------------------------------------------------------------------

    /// The changeset file list of the task's `rework/<task-id>` branch
    /// against the baseline the platform branched from (REQ-WV-03: exactly
    /// the `bound_head` stored in the first `worktree.prepared` event —
    /// never the main branch's moving HEAD). The view distinguishes the
    /// four states of REQ-WV-01 AC2 / REQ-WV-07 by DATA (`state` token +
    /// `zero_changes` / `partial` markers); "no isolation" and "zero
    /// changes" are successful verdicts, not errors.
    pub fn task_workspace_changes(&self, id_prefix: &str) -> ApiResult<WorkspaceChangesView> {
        let f = self.workspace_facts(id_prefix)?;
        let mut view = WorkspaceChangesView {
            task: f.task.clone(),
            state: f.state.to_string(),
            isolated: f.isolated,
            zero_changes: false,
            partial: f.partial,
            branch: f.branch.clone(),
            worktree: f.worktree.clone(),
            bound_head: f.bound_head.clone(),
            tip: f.tip.clone(),
            head_before: f.head_before.clone(),
            head_after: f.head_after.clone(),
            commits: f.commits.clone(),
            files: Vec::new(),
            note: f.note.clone(),
        };
        if let (Some(repo), Some(baseline), Some(tip)) = (&f.repo, &f.bound_head, &f.tip) {
            // The range is the one the facts resolved (live branch tip, or
            // the last stored commit stamp when the branch is gone — the
            // same resolution the companion diff operation reuses).
            view.files = worktree::diff_name_status(repo, baseline, tip)
                .map_err(|e| ApiError::Runtime(RuntimeError::Worktree(e)))?
                .into_iter()
                .map(|(status, path)| WorkspaceFileChange { status, path })
                .collect();
            view.zero_changes = view.files.is_empty();
        }
        Ok(view)
    }

    /// The readable unified diff of ONE file between exactly the baseline
    /// and tip the changeset view used (REQ-WV-02 AC1: same range, one
    /// resolution path — consistency is structural, not a convention).
    /// A path outside the changeset is a typed `not_found` the consumer
    /// branches on by code; binary content answers as a binary fact with
    /// the stored size, raw bytes are never rendered (AC3).
    pub fn task_workspace_diff(&self, id_prefix: &str, path: &str) -> ApiResult<WorkspaceDiffView> {
        let f = self.workspace_facts(id_prefix)?;
        let mut view = WorkspaceDiffView {
            task: f.task.clone(),
            path: path.to_string(),
            state: f.state.to_string(),
            branch: f.branch.clone(),
            baseline: f.bound_head.clone(),
            tip: f.tip.clone(),
            binary: false,
            size_bytes: None,
            diff: None,
            note: f.note.clone(),
        };
        let (Some(repo), Some(baseline), Some(tip)) = (&f.repo, &f.bound_head, &f.tip) else {
            // No_isolation / unavailable: the typed verdict (state + stored
            // facts) is the honest answer — there is no list to be in.
            return Ok(view);
        };
        let git_err = |e| ApiError::Runtime(RuntimeError::Worktree(e));
        let listed = worktree::diff_name_status(repo, baseline, tip)
            .map_err(git_err)?
            .into_iter()
            .any(|(_, p)| p == path);
        if !listed {
            // REQ-WV-02 AC2: the UI branches on the API code, not the text.
            return Err(ApiError::Store(devai_store::StoreError::NotFound(
                "workspace file".into(),
                format!(
                    "`{path}` is not in the changeset of task {}",
                    devai_core::types::short_id_str(&f.task)
                ),
            )));
        }
        if worktree::diff_is_binary(repo, baseline, tip, path).map_err(git_err)? {
            view.binary = true;
            view.size_bytes = worktree::blob_size(repo, tip, path)
                .or_else(|| worktree::blob_size(repo, baseline, path));
            view.note = Some("modified, binary content — raw bytes are not rendered".into());
            return Ok(view);
        }
        view.diff = Some(worktree::diff_text(repo, baseline, tip, path).map_err(git_err)?);
        Ok(view)
    }

    /// The one resolution both workspace operations share: stored facts
    /// first (events + project rows), then — and only as a reachability
    /// question — which recorded repository directory can answer git.
    /// No re-derivation: baseline = stored `bound_head`, tip = live branch
    /// or the last stored `worktree.committed` stamp.
    fn workspace_facts(&self, id_prefix: &str) -> ApiResult<WorkspaceFacts> {
        let t = self.store.resolve_task(id_prefix)?;
        let events = self.store.all_events_for_task(t.id)?;

        let mut bound_head: Option<String> = None;
        let mut worktree_dir: Option<String> = None;
        let mut merged: Option<(Option<String>, Option<String>)> = None;
        let mut commits: Vec<WorkspaceCommitStamp> = Vec::new();
        let mut isolated = false;
        for ev in &events {
            match ev.kind.as_str() {
                event_kind::WORKTREE_PREPARED => {
                    isolated = true;
                    // The FIRST prepared stamp owns the baseline: isolation
                    // branched from the bound head of its first composition
                    // (REQ-WV-03); a later rework reuses the branch.
                    if bound_head.is_none() {
                        bound_head = string_field(&ev.payload, "bound_head");
                    }
                    worktree_dir = string_field(&ev.payload, "worktree");
                }
                event_kind::WORKTREE_COMMITTED => {
                    if let Some(commit) = string_field(&ev.payload, "commit") {
                        commits.push(WorkspaceCommitStamp {
                            execution: string_field(&ev.payload, "execution").unwrap_or_default(),
                            commit,
                            at: ev.created_at,
                        });
                    }
                }
                event_kind::REWORK_MERGED => {
                    merged = Some((
                        string_field(&ev.payload, "head_before"),
                        string_field(&ev.payload, "head_after"),
                    ));
                }
                _ => {}
            }
        }
        let (head_before, head_after) = merged.unwrap_or((None, None));

        let mut f = WorkspaceFacts {
            task: t.id.to_string(),
            isolated,
            // The branch name is deterministic from the task id — the same
            // spelling task.active publishes for a prepared worktree
            // (worktree::branch_name is the only place the shape exists).
            branch: isolated.then(|| worktree::branch_name(t.id)),
            worktree: worktree_dir.clone(),
            bound_head: bound_head.clone(),
            commits: commits.clone(),
            head_before,
            head_after,
            state: workspace_state::NO_ISOLATION,
            repo: None,
            tip: None,
            partial: false,
            note: None,
        };
        if !isolated {
            f.note = Some(
                "the task has no worktree.prepared event — its work was never isolated; there is no changeset to show"
                    .into(),
            );
            return Ok(f);
        }

        // Candidate repositories, strictly from stored paths: the recorded
        // worktree directory, the workspace recorded at `rework.merged`,
        // then the task's project repositories. The owner of the facts is
        // the one that resolves them (branch ref or commit objects) —
        // REQ-WV-07 AC3 makes this a rev resolvability question.
        let last_commit = commits.last().map(|c| c.commit.clone());
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(dir) = &worktree_dir {
            candidates.push(PathBuf::from(dir));
        }
        if let Some(ev) = events
            .iter()
            .rev()
            .find(|e| e.kind == event_kind::REWORK_MERGED)
        {
            if let Some(ws) = string_field(&ev.payload, "workspace") {
                candidates.push(PathBuf::from(ws));
            }
        }
        if let Some(pid) = t.project_id {
            if let Some(p) = self.store.get_project(&pid.to_string())? {
                for r in p.repos {
                    candidates.push(PathBuf::from(r));
                }
            }
        }
        let owns_facts = |dir: &Path| -> bool {
            worktree::branch_tip(dir, t.id).is_some()
                || bound_head
                    .as_deref()
                    .is_some_and(|h| worktree::commit_resolvable(dir, h))
                || last_commit
                    .as_deref()
                    .is_some_and(|c| worktree::commit_resolvable(dir, c))
        };
        let repo = candidates
            .iter()
            .find(|c| worktree::is_git_repo(c) && owns_facts(c))
            .cloned();
        let Some(repo) = repo else {
            f.state = workspace_state::UNAVAILABLE;
            f.note = Some(
                "no recorded repository path is reachable and owns the stored commits — only the stored facts can be shown"
                    .into(),
            );
            return Ok(f);
        };
        f.repo = Some(repo.clone());

        if let Some(tip) = worktree::branch_tip(&repo, t.id) {
            f.state = workspace_state::LIVE;
            f.tip = Some(tip);
            return Ok(f);
        }
        // Branch gone: (b) is the stored shas still resolvable in the
        // object store, (c) is everything else.
        let both_resolvable = bound_head
            .as_deref()
            .is_some_and(|h| worktree::commit_resolvable(&repo, h))
            && last_commit
                .as_deref()
                .is_some_and(|c| worktree::commit_resolvable(&repo, c));
        if both_resolvable {
            f.state = workspace_state::HISTORICAL;
            f.partial = true;
            f.tip = last_commit;
            f.note = Some(
                "the branch no longer exists — the view is rebuilt between the stored baseline and the last stored commit stamp, resolvable in the object store (partial: content after that stamp is unknowable)".into(),
            );
        } else {
            f.state = workspace_state::UNAVAILABLE;
            f.note = Some(
                "the branch is gone and its stored commits no longer resolve in the object store — only the stored facts can be shown"
                    .into(),
            );
        }
        Ok(f)
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
        let live =
            liveness::check_owner(e.owner_pid, e.owner_host.as_deref(), e.owner_process_start);
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
    pub fn export_bundle(
        &self,
        id_prefix: &str,
        out_dir: &Path,
    ) -> ApiResult<bundle::BundleManifest> {
        let t = self.store.resolve_task(id_prefix)?;
        Ok(bundle::export(&self.store, &t, out_dir)?)
    }

    // internal: none — every read above is one store call per fact.
}

fn short_hint(id: &Id) -> String {
    // Single source for the display width + rationale (R1 QA no-magic:
    // the width is devai_core's, not a duplicated literal).
    devai_core::types::short_id(id)
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

/// One changed file in the task's committed changeset: git's own status
/// token (A/M/D — `--no-renames`, the canonical digest precedent) plus the
/// path, in git's path-sorted order (REQ-WV-05 AC2).
#[derive(Serialize, Clone, Debug)]
pub struct WorkspaceFileChange {
    pub status: String,
    pub path: String,
}

/// One platform commit stamp: the `worktree.committed` event data — which
/// execution produced which commit on the task's branch (REQ-WV-03 AC1:
/// per-run history).
#[derive(Serialize, Clone, Debug)]
pub struct WorkspaceCommitStamp {
    pub execution: String,
    pub commit: String,
    pub at: Timestamp,
}

/// The workspace-view state tokens (REQ-WV-01 AC2, REQ-WV-07): the split
/// is a typed verdict in DATA, and "no isolation" / "zero changes" are
/// successful answers, never errors. `historical` = branch gone, stored
/// shas still resolvable (partial view); `unavailable` = only the stored
/// facts can be shown.
pub mod workspace_state {
    pub const NO_ISOLATION: &str = "no_isolation";
    pub const LIVE: &str = "live";
    pub const HISTORICAL: &str = "historical";
    pub const UNAVAILABLE: &str = "unavailable";
}

/// Output of `task.workspace_changes` — the file list plus the stored
/// facts the range was composed from (baseline, tip, stamps, merge heads).
#[derive(Serialize, Clone, Debug)]
pub struct WorkspaceChangesView {
    pub task: String,
    pub state: String,
    pub isolated: bool,
    /// Marker for the legitimate zero-change run (REQ-WV-01 AC4): isolation
    /// happened, the list is empty — distinct from `unavailable` by token.
    pub zero_changes: bool,
    /// Marker for `historical`: rebuilt from stored stamps, not the live
    /// branch (REQ-WV-07 AC2: never presented as if the branch were live).
    pub partial: bool,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub bound_head: Option<String>,
    pub tip: Option<String>,
    pub head_before: Option<String>,
    pub head_after: Option<String>,
    pub commits: Vec<WorkspaceCommitStamp>,
    pub files: Vec<WorkspaceFileChange>,
    pub note: Option<String>,
}

/// Output of `task.workspace_diff` — one file's readable diff over exactly
/// the range the changeset view used.
#[derive(Serialize, Clone, Debug)]
pub struct WorkspaceDiffView {
    pub task: String,
    pub path: String,
    pub state: String,
    pub branch: Option<String>,
    pub baseline: Option<String>,
    pub tip: Option<String>,
    pub binary: bool,
    pub size_bytes: Option<u64>,
    pub diff: Option<String>,
    pub note: Option<String>,
}

/// Internal resolution shared by the two workspace operations: the stored
/// facts, plus the git-reachable repository (if any) that owns them and
/// the range to present. Never written anywhere — recomputed per call from
/// the same rows, presentation over single-source facts.
pub(crate) struct WorkspaceFacts {
    pub task: String,
    pub isolated: bool,
    pub branch: Option<String>,
    pub worktree: Option<String>,
    pub bound_head: Option<String>,
    pub commits: Vec<WorkspaceCommitStamp>,
    pub head_before: Option<String>,
    pub head_after: Option<String>,
    pub state: &'static str,
    pub repo: Option<PathBuf>,
    pub tip: Option<String>,
    pub partial: bool,
    pub note: Option<String>,
}

fn string_field(payload: &serde_json::Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
}
