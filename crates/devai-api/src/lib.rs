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
mod homes;
mod inbox;
mod models;
mod mutations;
mod quality_gate;
mod registry;
mod signature;

/// Machine codes of the homes vocabulary — one dictionary, referenced by
/// producers here and by the UI bootstrap (R3 quality return).
pub mod codes;

pub use devai_runtime::convergence;
pub use devai_runtime::convergence::{
    ConvergenceCounters, ConvergencePoint, ConvergenceView, GAP_MILLI, STAGE_QUALITY,
    STAGE_SPEC_REVIEW, StageConvergence,
};
pub use error::{ApiError, ApiResult, ErrorCode};
pub use homes::{
    HOME_ENTRY_SCHEMA, HOME_ISSUE_SCHEMA, HOME_RECORD_SCHEMA, HOME_VIEW_SCHEMA, HOMES_FILE,
    HOMES_FORMAT_VERSION, HOMES_LOCK_FILE, HomeEntry, HomeIssue, HomeRecord, HomeRecordsView,
    Homes, LaunchResolution, ProjectSource, schema_names,
};
pub use inbox::{
    InboxSnapshotView, InboxTaskFacts, SpecBodyView, SpecFacts, factory_inbox_wait_policy,
    pick_spec_artifact,
};
// The home-layout facts of the store crate, re-exported so the UI reaches
// them through the API door only (REQ-UI-02: the UI graph must not gain a
// devai-store dependency to name `devai.db` or `.devai` correctly).
pub use devai_store::{DB_FILE_NAME, HOME_DIR_NAME};
pub use models::{
    ActiveBundleView, BundleFormInput, BundleFormView, BundleHandleView, BundleMutationView,
    BundleStatusView, FailuresBanner, ModelBundleView, ModelEventView, ModelSnapshot, ProbeView,
    RunningBundleRow, SwitchResultView, bundle_identity_text, stamp_text,
};
pub use mutations::{
    AdvanceOutcome, AdvanceReq, AgentAddReq, AnsweredOutcome, ApproveOutcome, AskReq,
    ContractAddReq, ContractRegistration, DependReq, ExecStartReq, ExecStarted, MergeRecordReq,
    MergeResult, MutationResult, OverrideResult, ProfileRatifyOutcome, ProfileRatifyReq,
    ProfileRevokeOutcome, ProfileRevokeReq, ProfileState, ProfileStateItem, ProjectAddReq,
    PublishOutcome, PublishReq, PublishedRow, QuestionAsked, RatifyResult, ReviseOutcome,
    ReviseReq, RunOutcome, RunReq, TaskCreateReq, TaskCreated, TaskProjectBackfillOutcome,
    TaskProjectSetOutcome, TaskProjectSetReq, TaskRenameOutcome, TaskRenameReq, UnlinkResult,
    WorkflowOp,
};
pub use quality_gate::{
    AUDIT_RUNNING, AUDIT_RUNNING_LABEL, DENIAL_CODES as QUALITY_GATE_DENIAL_CODES,
    NO_CURRENT_REPORT, OPEN_BLOCKERS, QualityGateDenial, QualityGateView, audit_running,
    is_audit_role, is_live_audit, quality_gate,
};
pub use registry::{OpClass, OpDescriptor, RegistryPublication, SURFACE_VERSION, operations};
pub use signature::{
    BodyError, BodyProblem, BodyProblemKind, DENIAL_CODES, DiffLine, DiffParagraphState,
    DiffRefusal, ParagraphSource, PreviousView, RequirementView, ReviewParagraphSource, ReviewView,
    SignatureFact, SignatureKind, SignatureSource, SignatureView, SpecDiff, SpecPresentation,
    SpecView, approve_denied, diff_specs, last_review_artifact, parse_review_body, parse_spec_body,
    pick_previous_spec, reject_denied, signature_view, spec_presentation,
};

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use devai_core::inbox::WaitGroup;
use devai_core::types::*;
use devai_core::workflow;
use devai_runtime::liveness;
use devai_runtime::{RuntimeError, worktree};
use devai_store::{Store, bundle};
use serde::Serialize;

/// Child rows (questions / artifacts) indexed by their stored task id:
/// built in one pass, consumed in one lookup per task. `Id` is the stored
/// uuid, never a display prefix.
type TaskIndex<T> = HashMap<Id, Vec<T>>;

/// Group stored rows by task id in a single pass — the batch read's cost
/// guard: the per-task loop below is O(T+Q+A), never O(T×(Q+A)).
fn group_by_task<T>(rows: Vec<T>, task_id: impl Fn(&T) -> Id) -> TaskIndex<T> {
    let mut index: TaskIndex<T> = HashMap::new();
    for row in rows {
        index.entry(task_id(&row)).or_default().push(row);
    }
    index
}

/// WHO is calling an operation, as the DOOR declared it (RV4 AC2). A
/// human-only act must not be reachable from a role/engine run, and the
/// caller's actor string is not evidence of what it is — the session kind is.
/// The API, not the door, decides what a non-human session may not do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SessionKind {
    /// The operator's own process: an interactive shell, no run marker.
    Human,
    /// A role/engine run. The platform exports `DEVAI_EVIDENCE_DIR` to every
    /// role process it spawns (REQ-WT-05 — the sink is platform-chosen, the
    /// agent never chooses it), so a CLI invocation inheriting that marker
    /// is a run of a role, not the operator's hand.
    Role,
}

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
    session: SessionKind,
    /// The probe SEAM (R8 quality return): the real driver call in
    /// production, a controlled runner in tests — so the probe/switch path is
    /// testable without launching an external process.
    probe_runner: ProbeRunner,
}

/// One probe call as the surface invokes it. The default is
/// [`devai_runtime::probe::probe`]; a test substitutes a runner that returns
/// a chosen outcome through the same request type.
pub type ProbeRunner = std::sync::Arc<
    dyn Fn(&devai_runtime::probe::ProbeRequest) -> devai_runtime::probe::ProbeOutcome + Send + Sync,
>;

impl Api {
    /// Bootstrap entry (the ratified OQ-10 exception, encapsulated per the
    /// human-ratified cycle-2 Q2): opening a home is not an operation of
    /// the registry — it happens before the domain exists — but it is also
    /// no longer a consumer-side `Store::open`. The handle stays inside.
    pub fn open(home: &Path, actor: &str) -> ApiResult<Api> {
        Self::open_session(home, actor, SessionKind::Human)
    }

    /// The same entry with the session's provenance made explicit: the CLI
    /// classifies its own process (see [`SessionKind`]) and the API enforces
    /// the human-only acts against it.
    pub fn open_session(home: &Path, actor: &str, session: SessionKind) -> ApiResult<Api> {
        Ok(Api {
            store: Store::open(home)?,
            home: home.to_path_buf(),
            actor: actor.to_string(),
            session,
            probe_runner: std::sync::Arc::new(devai_runtime::probe::probe),
        })
    }

    /// Substitute the probe runner (tests / diagnostics). The default is the
    /// real driver call; nothing else about the surface changes.
    pub fn with_probe_runner(mut self, runner: ProbeRunner) -> Self {
        self.probe_runner = runner;
        self
    }

    /// Observation entry (REQ-HOME-09): the same handle shape as
    /// `Api::open`, but backed by the store's read-only observation open.
    /// The home is read exactly as it is on disk, whatever its schema
    /// rank — no directory creation, no seeding, no migration ladder, no
    /// `user_version` bump. This is the entry `home.activate` uses: taking
    /// the stored-active marker is a REGISTRY act, and it must never
    /// migrate a home of another version by the act of opening it (the
    /// ratified OQ-HOME-1/REQ-HOME-09 rule, RA1 quality return).
    pub fn open_observed(home: &Path, actor: &str) -> ApiResult<Api> {
        Ok(Api {
            store: Store::open_read_only(home)?,
            home: home.to_path_buf(),
            actor: actor.to_string(),
            // An observation handle is the human UI's read plane; it writes
            // nothing, so the human-only gate never sees it.
            session: SessionKind::Human,
            probe_runner: std::sync::Arc::new(devai_runtime::probe::probe),
        })
    }

    /// The `home.activate` transaction as ONE API-plane act (the homes
    /// cycle QA, da/architecture-consistency: the validate→reopen→commit
    /// orchestration is platform mechanics, not consumer mechanics):
    ///   1. validate the registry record (`Homes::activation_target`);
    ///   2. re-probe right before the reopen — `Api::open` BOOTSTRAPS
    ///      (creates) a home, so an activation may only aim at a store
    ///      that is on disk at this very moment; a vanished home is never
    ///      silently resurrected as an empty one (REQ-HOME-07);
    ///   3. reopen the chosen home as an OBSERVATION (`Api::open_observed`
    ///      over `Store::open_read_only`): the operator's activation is a
    ///      selection of what to watch, never a migration of the observed
    ///      store — a home behind or ahead of this build is opened with
    ///      its rank untouched (`home.list` stamps the version note);
    ///   4. only then commit the stored-active flip in the registry.
    /// A failed reopen returns before step 4, so the registry can never
    /// point at a home the caller does not hold (REQ-HOME-06 AC1).
    pub fn activate_home(homes: &mut Homes, path: &str, actor: &str) -> ApiResult<Api> {
        let target = homes.activation_target(path)?;
        if !target.join(devai_store::DB_FILE_NAME).exists() {
            return Err(ApiError::refused(
                codes::HOME_UNAVAILABLE,
                format!("home {} disappeared during activation", target.display()),
            ));
        }
        let api = Api::open_observed(&target, actor)?;
        homes.set_active(&target)?;
        Ok(api)
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

    /// R3 QA (da/api-contract): the versioned machine-readable publication
    /// — `devai api registry --json` renders exactly this document. The
    /// surface version has ONE source (`registry::SURFACE_VERSION`); no
    /// consumer restamps it.
    pub fn api_registry_publication(&self) -> RegistryPublication {
        registry::publication()
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
        // REQ-QG-08: the QA gate's public availability travels WITH the status
        // read — the window does not derive it. The dependency/decision/
        // execution loads happen only at the gate; other states have no gate.
        let quality_gate = if t.state == TaskState::Qa {
            let deps = self.store.task_dependencies(t.id)?;
            let decisions = self.store.list_decisions(t.id)?;
            let executions = self.store.list_executions(Some(t.id))?;
            quality_gate::quality_gate(&t, &artifacts, &deps, &decisions, &executions)
        } else {
            None
        };
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
            quality_gate,
        })
    }

    /// The APPROVAL signature read (REQ-SIGN-07): paragraph, requirements,
    /// document questions, reviewer block, previous-version diff, signature
    /// rights and the newest stored signature fact — ONE operation, so the
    /// window makes one call for the whole card instead of a fan-out.
    pub fn task_signature(&self, id_prefix: &str) -> ApiResult<SignatureView> {
        let task = self.store.resolve_task(id_prefix)?;
        signature::signature_view(&self.store, task)
    }

    /// Pipeline position + running executions, purely from engine data
    /// (the progress renderer owns no state-machine knowledge). The answer
    /// also carries the convergence slice of the task (REQ-CV-12): one
    /// computed object, so a consumer never asks for it separately.
    pub fn task_progress(&self, id_prefix: &str) -> ApiResult<ProgressView> {
        let t = self.store.resolve_task(id_prefix)?;
        let now = Utc::now();
        let order = workflow::pipeline_order();
        let position = workflow::pipeline_position(&t);
        let pos_idx = position.and_then(|p| order.iter().position(|s| *s == p));
        let executions = self.store.list_executions(Some(t.id))?;
        let running: Vec<RunningExecutionView> = executions
            .iter()
            .filter(|e| e.status == ExecutionStatus::Running)
            .map(|e| RunningExecutionView {
                id: e.id.to_string(),
                role: format!("{}@{}", e.role_slug, e.role_version),
                role_slug: e.role_slug.clone(),
                role_version: e.role_version,
                agent_name: e.agent_name.clone(),
                started_at: e.started_at,
                elapsed_seconds: (now - e.started_at).num_seconds().max(0),
            })
            .collect();
        let artifacts = self.store.artifacts(t.id)?;
        let convergence = self.task_convergence(&t, &artifacts, &executions);
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
            convergence,
        })
    }

    /// Cross-task RUNNING snapshot with per-row liveness triage — the
    /// machine form of «что сейчас активно» (REQ-ACT doctrine, REQ-API-07
    /// AC3: orphans are distinguishable by data, not by operator pgrep).
    pub fn task_active(&self) -> ApiResult<ActiveSnapshot> {
        let now = Utc::now();
        let running = self.running_executions()?;
        let facts = self.execution_task_facts(&running)?;
        let rows = running
            .iter()
            .map(|e| self.active_row(e, &facts, now))
            .collect();
        Ok(ActiveSnapshot {
            snapshot_at: now,
            rows,
        })
    }

    /// The RUNNING rows, oldest work first; the id tie-break keeps snapshots
    /// deterministic.
    fn running_executions(&self) -> ApiResult<Vec<RoleExecution>> {
        let mut running: Vec<RoleExecution> = self
            .store
            .list_executions(None)?
            .into_iter()
            .filter(|e| e.status == ExecutionStatus::Running)
            .collect();
        running.sort_by(|a, b| a.started_at.cmp(&b.started_at).then(a.id.cmp(&b.id)));
        Ok(running)
    }

    /// One task read per DISTINCT task of the snapshot, plus whether its
    /// worktree was ever prepared — the per-row facts, computed once.
    fn execution_task_facts(
        &self,
        running: &[RoleExecution],
    ) -> ApiResult<HashMap<Id, (Task, bool)>> {
        let mut facts: HashMap<Id, (Task, bool)> = HashMap::new();
        for e in running {
            if facts.contains_key(&e.task_id) {
                continue;
            }
            let t = self.store.resolve_task(&e.task_id.to_string())?;
            let prepared = self
                .store
                .all_events_for_task(t.id)?
                .iter()
                .any(|ev| ev.kind == event_kind::WORKTREE_PREPARED);
            facts.insert(e.task_id, (t, prepared));
        }
        Ok(facts)
    }

    /// One row of the snapshot: the stored facts plus ONE liveness
    /// observation (read-only: REQ-SW-06 — detection never writes and never
    /// signals a process).
    fn active_row(
        &self,
        e: &RoleExecution,
        facts: &HashMap<Id, (Task, bool)>,
        now: DateTime<Utc>,
    ) -> ActiveRow {
        let (t, prepared) = &facts[&e.task_id];
        let live =
            liveness::check_owner(e.owner_pid, e.owner_host.as_deref(), e.owner_process_start);
        let terminal = t.state.is_terminal();
        // A live quality audit at the QA gate is a PROCESS, not a human wait
        // (REQ-QG-09 AC3): the active row carries the platform's words. The
        // SAME predicate the gate uses decides «live» (rework R1/R4), so an
        // orphaned row is never labelled as a running audit.
        let note = (t.state == TaskState::Qa && quality_gate::is_live_audit(e))
            .then(|| quality_gate::AUDIT_RUNNING_LABEL.to_string());
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
            note,
        }
    }

    /// The quality report the human signs at the QA gate — full machine body
    /// plus the stored human rendering (REQ-API-07: observable outside the
    /// session that produced it).
    ///
    /// REQ-QH-05/06: CURRENT is preferred, but when the task has been
    /// returned to rework the rejected report is still SHOWN — with its
    /// STALE status and a freshness marker naming the re-audit — instead of
    /// a «QA has not run» lie or a stale text posing as the decision.
    pub fn task_report(&self, id_prefix: &str) -> ApiResult<QualityReportView> {
        let t = self.store.resolve_task(id_prefix)?;
        // ONE presentation pipeline (rework R2): row selection, blob read,
        // rendering, corrupt-body outcome and the freshness fact all come from
        // `presented_quality` — the same call the CLI gates use. The view can
        // never describe a different store state than the gate text.
        let Some(p) = self.presented_quality(&t)? else {
            return Err(ApiError::Store(devai_store::StoreError::NotFound(
                "quality report".into(),
                format!(
                    "task {} has no quality report yet — QA has not run",
                    short_hint(&t.id)
                ),
            )));
        };
        let a = &p.artifact;
        let contract = a
            .contract
            .as_ref()
            .map(|c| format!("{}@{}", c.name, c.version))
            .unwrap_or_else(|| "—".into());
        Ok(QualityReportView {
            task: t.id.to_string(),
            artifact: a.id.to_string(),
            seq: a.seq,
            contract,
            created_at: a.created_at,
            execution: a.execution_id.map(|e| e.to_string()),
            status: a.status.as_str().to_string(),
            report: p.body,
            report_text: Some(p.presented.text),
            source: p.presented.source.as_str().to_string(),
            freshness: Some(p.presented.freshness),
            first_layer: p.presented.first_layer,
        })
    }

    /// The convergence slice of a task (REQ-CV-12), computed from data the
    /// store already holds. NOT a registry operation of its own: the slice
    /// rides on `task.progress` and on `inbox.snapshot`, the two reads that
    /// existed before (REQ-CV-14: no new operations). A TERMINAL task
    /// answers `None` — the slice does not read the bodies of finished work.
    fn task_convergence(
        &self,
        task: &Task,
        arts: &[Artifact],
        execs: &[RoleExecution],
    ) -> Option<ConvergenceView> {
        if task.state.is_terminal() {
            return None;
        }
        let read = |a: &Artifact| self.store.read_artifact_body(a);
        let bodies = convergence::BodyCache::from_reader(&read);
        Some(convergence::compute(arts, execs, &bodies))
    }

    /// The human-wait section in ONE batch read (REQ-GATE-01..10): the
    /// declarative demand table of this home plus every task's stored
    /// questions and gate presentation. One store snapshot per data type
    /// (tasks / questions / artifacts) instead of a per-task fan-out — the
    /// caller's per-row cost is O(1) IPC (one call per load).
    ///
    /// The presentation is loaded for the stored gate tokens that present
    /// it: APPROVAL → the spec (CURRENT, else the LAST one with the honest
    /// «not current» mark, ratified OQ-GATE-3) and QA → the CURRENT quality
    /// report (or the platform's text when there is none). Membership and
    /// grouping are the UI's one decision site (`wait_group_of`).
    ///
    /// Source-of-operations note (SPEC-FIDELITY, RATIFIED): this ONE batch
    /// operation replaces the spec's `task.list` composition and
    /// `artifact.cat_text` calls. The operator ratified the replacement in
    /// words («Ратифицирую», 07.11); the word is stored in the QA-return
    /// rationale of this task (R5 of the 20:25 return, R6 of the 20:48 one)
    /// — the platform keeps no separate decision row for it. An approved
    /// improvement, not a deviation; the old operations are not to be
    /// restored.
    pub fn inbox_snapshot(&self) -> ApiResult<InboxSnapshotView> {
        let snapshot_at = Utc::now();
        let policy =
            devai_store::config::load_inbox_wait_policy(&self.config_dir().join("workflow"))?;
        let tasks = self.store.list_tasks()?;
        // Batch-read cost: index the two child tables by task id in one
        // pass each, so the per-task loop below is O(T+Q+A), not O(T×(Q+A)).
        // `list_tasks` returns each task once, so taking rows OUT of the
        // index (remove) is safe and avoids cloning every row.
        let mut questions = group_by_task(self.store.questions_all()?, |q| q.task_id);
        let mut artifacts = group_by_task(self.store.artifacts_all()?, |a| a.task_id);
        // ONE execution snapshot for the whole section: the convergence
        // fact of a gate row needs the rows its carriers name, and a
        // per-row query would put the fan-out back (REQ-CV-09 AC5).
        let mut executions = group_by_task(self.store.list_executions(None)?, |e| e.task_id);
        // ONE body cache for the whole snapshot (REQ-CV-12 AC7/AC8): a spec
        // bound to N reviews of five tasks is read once, not once per row.
        let read = |a: &Artifact| self.store.read_artifact_body(a);
        let bodies = convergence::BodyCache::from_reader(&read);

        let facts = tasks
            .into_iter()
            .map(|task| {
                self.task_facts(
                    task,
                    &policy,
                    &mut questions,
                    &mut artifacts,
                    &mut executions,
                    &bodies,
                )
            })
            .collect::<ApiResult<Vec<_>>>()?;

        Ok(InboxSnapshotView {
            snapshot_at,
            policy,
            tasks: facts,
        })
    }

    /// One task's slice of the batch read: its stored questions and the
    /// click-presentation of its gate, taken out of the pre-built indexes.
    fn task_facts(
        &self,
        task: Task,
        policy: &devai_core::inbox::InboxWaitPolicy,
        questions: &mut TaskIndex<Question>,
        artifacts: &mut TaskIndex<Artifact>,
        executions: &mut TaskIndex<RoleExecution>,
        bodies: &convergence::BodyCache<'_>,
    ) -> ApiResult<InboxTaskFacts> {
        let owned_questions = questions.remove(&task.id).unwrap_or_default();
        let owned_artifacts = artifacts.remove(&task.id).unwrap_or_default();
        let owned_executions = executions.remove(&task.id).unwrap_or_default();
        let (spec, report, report_note) = self.gate_presentation(&task, &owned_artifacts)?;
        // REQ-QG-09: a live audit is a fact of the SAME execution snapshot the
        // convergence slice already reads (no extra store call).
        let audit_running = quality_gate::audit_running(&owned_executions);
        // The SAME object the progress read answers (REQ-CV-12 AC1/AC5),
        // built ONLY for a row the wait section actually shows: a state the
        // declarative table puts in neither group (and every terminal task)
        // answers null and its bodies are never read (REQ-CV-16 AC3).
        let wait_row = policy
            .state(task.state)
            .map(|s| s.group != WaitGroup::NotWait)
            .unwrap_or(false);
        let convergence = if wait_row && !task.state.is_terminal() {
            Some(convergence::compute(
                &owned_artifacts,
                &owned_executions,
                bodies,
            ))
        } else {
            None
        };
        Ok(InboxTaskFacts {
            task,
            questions: owned_questions,
            spec,
            report,
            report_note,
            audit_running,
            convergence,
        })
    }

    /// The gate presentation for the stored state that presents one:
    /// APPROVAL → the spec body (ratified OQ-GATE-3/4), QA → the CURRENT
    /// quality report (or the platform's honest text). Any other state has
    /// no click-presentation — that decision stays in ONE place.
    fn gate_presentation(
        &self,
        task: &Task,
        arts: &[Artifact],
    ) -> ApiResult<(Option<SpecFacts>, Option<QualityReportView>, Option<String>)> {
        let spec = match task.state {
            TaskState::Approval => self.spec_facts(arts)?,
            _ => None,
        };
        let (report, note) = match task.state {
            TaskState::Qa => match self.task_report(&task.id.to_string()) {
                Ok(view) => (Some(view), None),
                Err(e) => (None, Some(e.to_string())),
            },
            _ => (None, None),
        };
        Ok((spec, report, note))
    }

    /// The APPROVAL text of the spec artifact: CURRENT REQUIREMENTS_SPEC when
    /// one exists, else the LAST one with `current: false`. A binary body is
    /// an honest size fact; a read failure is the platform's text — the
    /// presentation never presents raw bytes or an empty body (ratified
    /// OQ-GATE-3/4).
    fn spec_facts(&self, arts: &[Artifact]) -> ApiResult<Option<SpecFacts>> {
        // Selection is a pure unit (`inbox::pick_spec_artifact`): CURRENT,
        // else the LAST one with `current: false`.
        let Some((artifact, current)) = inbox::pick_spec_artifact(arts) else {
            return Ok(None);
        };
        let body = match self.store.read_artifact_body(&artifact) {
            Ok(bytes) => match String::from_utf8(bytes) {
                Ok(text) => SpecBodyView::Text { text },
                Err(_) => SpecBodyView::Binary {
                    size: artifact.size,
                },
            },
            Err(e) => SpecBodyView::Error {
                message: e.to_string(),
            },
        };
        Ok(Some(SpecFacts {
            artifact: artifact.clone(),
            current,
            body,
        }))
    }

    /// The home's config directory, resolved like the engine's humanize
    /// policy (REQ-HP-04): `DEVAI_CONFIG` when set, else `<home>/config`.
    fn config_dir(&self) -> PathBuf {
        std::env::var_os("DEVAI_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.store.root().join("config"))
    }

    /// The config directory as the model-bundle loader sees it. Kept
    /// `pub(crate)`: a consumer reads bundles through the snapshot, never by
    /// walking the home itself (REQ-MS-07: the window does not compose).
    pub(crate) fn model_config_dir(&self) -> PathBuf {
        self.config_dir()
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
        let mut view = workspace_diff_view(&f, path);
        let (Some(repo), Some(baseline), Some(tip)) = (&f.repo, &f.bound_head, &f.tip) else {
            // No_isolation / unavailable: the typed verdict (state + stored
            // facts) is the honest answer — there is no list to be in.
            return Ok(view);
        };
        Self::require_in_changeset(
            repo,
            baseline,
            tip,
            path,
            &devai_core::types::short_id_str(&f.task),
        )?;
        Self::fill_diff(&mut view, repo, baseline, tip, path)?;
        Ok(view)
    }

    /// REQ-WV-02 AC2: a path outside the changeset is a typed `not_found`
    /// the consumer branches on by code, not by text.
    fn require_in_changeset(
        repo: &Path,
        baseline: &str,
        tip: &str,
        path: &str,
        task: &str,
    ) -> ApiResult<()> {
        let listed = worktree::diff_name_status(repo, baseline, tip)
            .map_err(|e| ApiError::Runtime(RuntimeError::Worktree(e)))?
            .into_iter()
            .any(|(_, p)| p == path);
        if listed {
            return Ok(());
        }
        Err(ApiError::Store(devai_store::StoreError::NotFound(
            "workspace file".into(),
            format!("`{path}` is not in the changeset of task {task}"),
        )))
    }

    /// The content half (REQ-WV-02 AC3): binary content is a binary fact with
    /// the stored size, raw bytes are never rendered; text is the unified
    /// diff over exactly the range the changeset view used.
    fn fill_diff(
        view: &mut WorkspaceDiffView,
        repo: &Path,
        baseline: &str,
        tip: &str,
        path: &str,
    ) -> ApiResult<()> {
        let git_err = |e| ApiError::Runtime(RuntimeError::Worktree(e));
        if worktree::diff_is_binary(repo, baseline, tip, path).map_err(git_err)? {
            view.binary = true;
            view.size_bytes = worktree::blob_size(repo, tip, path)
                .or_else(|| worktree::blob_size(repo, baseline, path));
            view.note = Some("modified, binary content — raw bytes are not rendered".into());
            return Ok(());
        }
        view.diff = Some(worktree::diff_text(repo, baseline, tip, path).map_err(git_err)?);
        Ok(())
    }

    /// The one resolution both workspace operations share, split into two
    /// private units (R2): `workspace_event_facts` — a pure fold over the
    /// task's stored workflow events — and `resolve_workspace_repo` — the
    /// reachability question of which recorded directory can answer git.
    /// Both operations go through this ONE path, which is why list and diff
    /// cannot disagree; the split changes the shape, not the resolution.
    /// No re-derivation: baseline = stored `bound_head`, tip = live branch
    /// or the last stored `worktree.committed` stamp.
    fn workspace_facts(&self, id_prefix: &str) -> ApiResult<WorkspaceFacts> {
        let t = self.store.resolve_task(id_prefix)?;
        let events = self.store.all_events_for_task(t.id)?;
        let mut f = workspace_event_facts(&t, &events);
        if f.isolated {
            self.resolve_workspace_repo(&mut f, &t, &events)?;
        }
        Ok(f)
    }

    /// Repo-resolution unit: candidates strictly from stored paths (the
    /// recorded worktree directory, the workspace recorded at
    /// `rework.merged`, then the task's project repositories). The owner of
    /// the facts is the one that resolves them — REQ-WV-07 AC3 makes this a
    /// rev-resolvability question. The branch probe is typed
    /// (`worktree::branch_tip`): "git itself failed" propagates as a
    /// distinguishable failure (R3) and never degrades into a
    /// `historical`/`unavailable` verdict masquerading as an honest answer.
    fn resolve_workspace_repo(
        &self,
        f: &mut WorkspaceFacts,
        t: &Task,
        events: &[WorkflowEvent],
    ) -> ApiResult<()> {
        let candidates = self.repo_candidates(f, t, events)?;
        let Some(repo) = Self::owning_repo(&candidates, t.id, f)? else {
            f.state = workspace_state::UNAVAILABLE;
            f.note = Some(
                "no recorded repository path is reachable and owns the stored commits — only the stored facts can be shown"
                    .into(),
            );
            return Ok(());
        };
        f.repo = Some(repo.clone());
        Self::resolve_workspace_tip(f, &repo, t.id)?;
        Ok(())
    }

    /// The candidate directories, strictly from stored paths: the recorded
    /// worktree directory, the workspace recorded at `rework.merged`, then
    /// the task's project repositories.
    fn repo_candidates(
        &self,
        f: &WorkspaceFacts,
        t: &Task,
        events: &[WorkflowEvent],
    ) -> ApiResult<Vec<PathBuf>> {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(dir) = &f.worktree {
            candidates.push(PathBuf::from(dir));
        }
        if let Some(ws) = Self::merged_workspace(events) {
            candidates.push(PathBuf::from(ws));
        }
        if let Some(pid) = t.project_id {
            if let Some(p) = self.store.get_project(&pid.to_string())? {
                for r in p.repos {
                    candidates.push(PathBuf::from(r));
                }
            }
        }
        Ok(candidates)
    }

    /// The workspace recorded by the LAST `rework.merged` event, if any.
    fn merged_workspace(events: &[WorkflowEvent]) -> Option<String> {
        events
            .iter()
            .rev()
            .find(|e| e.kind == event_kind::REWORK_MERGED)
            .and_then(|ev| string_field(&ev.payload, "workspace"))
    }

    /// The first candidate that is a git repository AND owns the task's
    /// stored facts (its branch, or the stored commits). A git failure in
    /// the probe propagates as an Err, never as a verdict.
    fn owning_repo(
        candidates: &[PathBuf],
        task_id: Id,
        f: &WorkspaceFacts,
    ) -> ApiResult<Option<PathBuf>> {
        let bound = f.bound_head.as_deref();
        let last_commit = f.commits.last().map(|c| c.commit.as_str());
        for c in candidates {
            if !worktree::is_git_repo(c) {
                continue;
            }
            let owns = match worktree::branch_tip(c, task_id)
                .map_err(|e| ApiError::Runtime(RuntimeError::Worktree(e)))?
            {
                Some(_) => true,
                None => {
                    bound.is_some_and(|h| worktree::commit_resolvable(c, h))
                        || last_commit.is_some_and(|k| worktree::commit_resolvable(c, k))
                }
            };
            if owns {
                return Ok(Some(c.clone()));
            }
        }
        Ok(None)
    }

    /// Live branch first; branch gone → the stored commits still resolvable
    /// in the object store (`historical`, partial) or neither (`unavailable`).
    /// A git failure of the probe is an Err (R3: never a verdict).
    fn resolve_workspace_tip(f: &mut WorkspaceFacts, repo: &Path, task_id: Id) -> ApiResult<()> {
        let bound = f.bound_head.clone();
        let last_commit = f.commits.last().map(|c| c.commit.clone());
        if let Some(tip) = worktree::branch_tip(repo, task_id)
            .map_err(|e| ApiError::Runtime(RuntimeError::Worktree(e)))?
        {
            f.state = workspace_state::LIVE;
            f.tip = Some(tip);
            return Ok(());
        }
        // Branch gone: (b) is the stored shas still resolvable in the
        // object store, (c) is everything else.
        let both_resolvable = bound
            .as_deref()
            .is_some_and(|h| worktree::commit_resolvable(repo, h))
            && last_commit
                .as_deref()
                .is_some_and(|c| worktree::commit_resolvable(repo, c));
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
        Ok(())
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
        Ok(self.store.read_artifact_body(&a)?)
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
    /// REQ-QG-08/09: the QA gate's per-action availability and platform
    /// refusal; `None` outside QA (the actions do not exist there).
    pub quality_gate: Option<QualityGateView>,
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
    /// The convergence slice (REQ-CV-12); `null` on terminal tasks and on
    /// rows outside the wait section.
    pub convergence: Option<ConvergenceView>,
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
    /// REQ-QG-09 AC3: the platform's word for what the live run IS. A running
    /// quality audit is «идёт аудит», never «ждёт вашего решения».
    pub note: Option<String>,
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
    /// The stored artifact status of the row being shown: CURRENT, or the
    /// STALE/SUPERSEDED predecessor shown under a «устарел» marker.
    pub status: String,
    /// The machine body of the report (the stored artifact content).
    pub report: serde_json::Value,
    /// The human rendering the operator signs: the STORED human form when
    /// the report carries one, the legacy flat `report_text` for pre-@5
    /// bodies.
    pub report_text: Option<String>,
    /// What that rendering came from — `human_form`, `flat_text`,
    /// `corrupt_form`, `corrupt_body` or `absent` (REQ-QH-06 / rework R4):
    /// a consumer can tell a form from a fallback without sniffing the text.
    pub source: String,
    /// REQ-QH-05: freshness as store fact — `stale` plus the marker and the
    /// re-audit execution it names. Never inferred from the report body.
    pub freshness: Option<ReportFreshness>,
    /// REQ-SAY-01..05: the human FIRST LAYER of the same stored form — «что
    /// нашёл аудит» in words, the findings with their human phrase / site /
    /// what they cost, and «не проверено» kept apart. Read off the stored
    /// facts at presentation time (the `report_text` precedent), never a
    /// recomputation of the report. `None` when the shown body has no readable
    /// human form — such a report stays the honest flat text / marker.
    pub first_layer: Option<FirstLayerView>,
}

/// REQ-SAY-01..05: the human first layer published beside a stored report.
pub use devai_runtime::human_view::first_layer::FirstLayerView;
/// Why the report on screen is (or is not) the last word — raw store facts:
/// the task phase, the stored artifact status, and the auditor execution.
/// The shape lives with the human form's presentation module (one owner for
/// writers and readers) and is re-exported here because the API views publish
/// it.
pub use devai_runtime::human_view::present::ReportFreshness;

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

/// The stored-facts half of `task.workspace_diff`: the typed verdict and the
/// range as RESOLVED (before any content is read). Shared by the diff
/// operation and its tests; the content half is `Api::fill_diff`.
fn workspace_diff_view(f: &WorkspaceFacts, path: &str) -> WorkspaceDiffView {
    WorkspaceDiffView {
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
    }
}

/// The raw composition facts of one task's event stream — the fold's output,
/// before they become a `WorkspaceFacts` view. One struct so the fold and the
/// constructor are separate cohesive units.
#[derive(Default)]
struct EventFacts {
    isolated: bool,
    bound_head: Option<String>,
    worktree_dir: Option<String>,
    merged: Option<(Option<String>, Option<String>)>,
    commits: Vec<WorkspaceCommitStamp>,
}

impl EventFacts {
    /// A pure fold over the stored events: which composition facts the
    /// workspace has (baseline = the `bound_head` of the FIRST
    /// `worktree.prepared`, per-run commit stamps, merge heads) and whether
    /// isolation happened at all. No git, no filesystem: data only.
    fn fold(events: &[WorkflowEvent]) -> EventFacts {
        let mut f = EventFacts::default();
        for ev in events {
            f.apply(ev);
        }
        f
    }

    fn apply(&mut self, ev: &WorkflowEvent) {
        match ev.kind.as_str() {
            event_kind::WORKTREE_PREPARED => {
                self.isolated = true;
                // The FIRST prepared stamp owns the baseline: isolation
                // branched from the bound head of its first composition
                // (REQ-WV-03); a later rework reuses the branch.
                if self.bound_head.is_none() {
                    self.bound_head = string_field(&ev.payload, "bound_head");
                }
                self.worktree_dir = string_field(&ev.payload, "worktree");
            }
            event_kind::WORKTREE_COMMITTED => {
                if let Some(commit) = string_field(&ev.payload, "commit") {
                    self.commits.push(WorkspaceCommitStamp {
                        execution: string_field(&ev.payload, "execution").unwrap_or_default(),
                        commit,
                        at: ev.created_at,
                    });
                }
            }
            event_kind::REWORK_MERGED => {
                self.merged = Some((
                    string_field(&ev.payload, "head_before"),
                    string_field(&ev.payload, "head_after"),
                ));
            }
            _ => {}
        }
    }
}

/// Events-scan unit (the R2 split): the stored workflow events become the
/// workspace facts of the view. The repository question is a separate unit
/// (`Api::resolve_workspace_repo`), so both workspace operations keep one
/// shared resolution and the list/diff stay consistent by construction.
fn workspace_event_facts(t: &Task, events: &[WorkflowEvent]) -> WorkspaceFacts {
    let e = EventFacts::fold(events);
    let (head_before, head_after) = e.merged.unwrap_or((None, None));
    WorkspaceFacts {
        task: t.id.to_string(),
        isolated: e.isolated,
        // The branch name is deterministic from the task id — the same
        // spelling task.active publishes for a prepared worktree
        // (worktree::branch_name is the only place the shape exists).
        branch: e.isolated.then(|| worktree::branch_name(t.id)),
        worktree: e.worktree_dir,
        bound_head: e.bound_head,
        commits: e.commits,
        head_before,
        head_after,
        state: workspace_state::NO_ISOLATION,
        repo: None,
        tip: None,
        partial: false,
        note: no_isolation_note(e.isolated),
    }
}

/// The honest note of a task that never isolated its work (REQ-WV-01 AC2).
fn no_isolation_note(isolated: bool) -> Option<String> {
    (!isolated).then(|| {
        "the task has no worktree.prepared event — its work was never isolated; there is no changeset to show"
            .to_string()
    })
}

#[cfg(test)]
mod tests {
    use super::ReportFreshness;

    /// The CURRENT quality-report contract, as seeded (highest version).
    fn quality_report_schema() -> serde_json::Value {
        let (_, _, raw) = devai_store::seed::contracts()
            .into_iter()
            .filter(|(name, _, _)| *name == "quality-report")
            .max_by_key(|(_, version, _)| *version)
            .expect("the quality-report contract is seeded");
        serde_json::from_str(&raw).expect("the seeded schema is valid JSON")
    }

    /// R1 (da/api-contract 0.7): the freshness marker this view publishes is
    /// declared by the quality-report contract — every field the view
    /// serialises is declared, and `required` is exactly the set a reader
    /// cannot do without (the two execution facts are nullable, so they stay
    /// optional). A panel reading `freshness.task_state` must never face an
    /// undeclared field.
    #[test]
    fn the_freshness_marker_matches_the_declared_shape() {
        let schema = quality_report_schema();
        let declared = &schema["properties"]["freshness"];
        let marker = ReportFreshness {
            stale: true,
            marker: "⚠ отчёт v1 УСТАРЕЛ — задача в REWORK".into(),
            report_seq: 1,
            report_status: "STALE".into(),
            task_state: "REWORK".into(),
            execution: Some("01a1b1b8-exec".into()),
            execution_status: Some("RUNNING".into()),
        };
        let value = serde_json::to_value(&marker).expect("the freshness fact serialises");
        let props = declared["properties"]
            .as_object()
            .expect("the contract declares freshness properties");
        for key in value.as_object().expect("serialises to an object").keys() {
            assert!(
                props.contains_key(key),
                "the API publishes freshness.{key}, the contract does not declare it"
            );
        }
        let required: Vec<&str> = declared["required"]
            .as_array()
            .expect("the contract declares freshness.required")
            .iter()
            .filter_map(|v| v.as_str())
            .collect();
        assert_eq!(
            required,
            vec![
                "stale",
                "marker",
                "report_seq",
                "report_status",
                "task_state"
            ],
            "required = the facts a reader cannot do without"
        );
        for key in &required {
            assert!(
                value.get(key).is_some(),
                "freshness.{key} is required, the writer does not emit it"
            );
        }
        // The nullable execution facts are honestly OPTIONAL: a marker with
        // only the required facts still loads as the reader's own type.
        let mut minimal = value.clone();
        let obj = minimal.as_object_mut().expect("object");
        obj.remove("execution");
        obj.remove("execution_status");
        let loaded: ReportFreshness =
            serde_json::from_value(minimal).expect("required-only freshness loads");
        assert_eq!(loaded.execution, None);
        assert_eq!(loaded.execution_status, None);
    }
}
