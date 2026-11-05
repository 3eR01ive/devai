//! The MUTATION plane of the platform API (cycle 2, REQ-MUT-01..08).
//!
//! Every write the CLI used to perform through its own `Store` handle is a
//! typed operation here: typed request in, typed result out, failure as an
//! `ApiError` with a machine-readable `ErrorCode` (REQ-MUT-01). The
//! operations DELEGATE to the same `devai_core::workflow` commands and the
//! same store calls the pre-cycle-2 CLI executed — no new business rules
//! live here (workflow stays the single owner of transitions; the store
//! keeps append-only and single-writer semantics untouched).
//!
//! Presentation stays with the consumer: an operation returns the facts the
//! CLI rendered (states, counts, event kinds, ids) and the CLI formats
//! them. Where the pre-API path printed between two store effects (manual
//! artifact publish prints the artifact row before the workflow advance
//! runs), the effects stay individually callable
//! (`artifact_publish_row` + `artifact_advance`) beside the compound
//! `artifact_publish` — parity of output on failing commands included
//! (REQ-MUT-06).
//!
//! Bootstrap note (human-ratified Q2 of cycle 2): `Api::open`/`seed_config`
//! encapsulate store opening and seeding, so no consumer ever holds a
//! `Store`. `init`/seed are still NOT registered operations — the cycle-1
//! ratified exception (OQ-10) moved from "direct store access" to
//! "encapsulated bootstrap", never into the operation surface.

use chrono::Utc;
use std::path::{Path, PathBuf};

use devai_core::types::*;
use devai_core::workflow::{self, Mutation};
use devai_runtime::{liveness, Gate, Engine, RunReport};
use devai_store::{bundle, guess_media_type, seed, ArtifactDraft};
use uuid::Uuid;

use crate::{Api, ApiError, ApiResult};

// ---------------------------------------------------------------------------
// Typed requests and results (REQ-MUT-01 AC1)
// ---------------------------------------------------------------------------

/// `project.add` — the fields `Project::new` consumed directly before.
#[derive(Debug, Clone)]
pub struct ProjectAddReq {
    pub name: String,
    pub description: String,
    pub repos: Vec<String>,
}

/// `task.create` — the creation input of `task new`. `project_id` is the
/// caller-resolved project row (resolving the name is the `project.get`
/// READ, so the not-found refusal text keeps its exact position in the
/// command's error order); parent/blocked_by are task refs registered as
/// edges at the moment of creation (REQ-DEP-07).
#[derive(Debug, Clone)]
pub struct TaskCreateReq {
    pub title: String,
    pub project_id: Option<Id>,
    pub description: String,
    pub parent: Vec<String>,
    pub blocked_by: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct TaskCreated {
    pub task: Task,
    pub linked: Vec<TaskDependency>,
}

/// The workflow mutations the old CLI `op()` helper dispatched over a
/// closure table. The dispatch stays one code path
/// (`Api::task_op`); every named operation delegates to it
/// (REQ-MUT-05 AC3: dispatch and dedicated command execute the same path).
#[derive(Debug, Clone)]
pub enum WorkflowOp {
    Start,
    Review { verdict: Verdict, summary: Option<String> },
    Approve { rationale: Option<String> },
    Reject { rationale: Option<String> },
    Accept { rationale: Option<String> },
    Return { rationale: Option<String> },
    Pause { reason: Option<String> },
    Resume,
    Block { reason: Option<String> },
    Cancel { reason: Option<String> },
    Fail { reason: Option<String> },
}

/// The committed outcome of one workflow mutation — the facts the CLI's
/// `op()` rendered: transition text, event kind, STALE/SUPERSEDED counts
/// and the next allowed operations. No unit proxy: the caller reads
/// real workflow data (REQ-MUT-01 AC1).
#[derive(Debug, Clone)]
pub struct MutationResult {
    pub task: Task,
    pub previous_state: TaskState,
    /// Event kinds of the mutation, in commit order (the CLI renders
    /// `events[0].kind` as «recorded (...)» exactly as the old path did).
    pub event_kinds: Vec<String>,
    pub staled: usize,
    pub superseded: usize,
    pub allowed_operations: Vec<String>,
}

/// `task.approve` — the mutation PLUS the `requirements_approval`
/// decision row the approve act persists (precedent 01a1a0b5-0fcf).
#[derive(Debug, Clone)]
pub struct ApproveOutcome {
    pub mutation: MutationResult,
    pub decision: Decision,
}

/// `task.run` — the engine drive of one task through the autonomous
/// stages. All store effects stay inside the engine; the outcome carries
/// the rendering facts (fresh-report identity, gate report text).
#[derive(Debug, Clone)]
pub struct RunReq {
    pub id: String,
    pub agent: String,
    pub workspace: Option<PathBuf>,
    pub force_reaudit: bool,
}

#[derive(Debug, Clone)]
pub struct RunOutcome {
    pub report: RunReport,
    /// The CURRENT quality report row (the `ReportFresh` gate names it).
    pub current_quality_report: Option<Artifact>,
    /// The stored §2.6 rendering shown at the human gates, when present.
    pub gate_report_text: Option<String>,
    pub allowed_operations: Vec<String>,
}

/// `task.revise` — publish a REQUIREMENTS_SPEC from consumer-supplied
/// bytes (the file read is input handling, not persistence) and move the
/// task to REVIEW through the same core command.
#[derive(Debug, Clone)]
pub struct ReviseReq {
    pub id: String,
    /// Display name of the source (the stored artifact name, as before).
    pub name: String,
    pub content: Vec<u8>,
    pub reason: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ReviseOutcome {
    pub task: Task,
    /// Ids of artifacts marked STALE by the revision (full ids, as the
    /// old rendering printed them).
    pub staled_ids: Vec<String>,
    pub allowed_operations: Vec<String>,
}

/// `artifact.publish` — the full manual publication through the contract
/// gate. `contract`/`verdict` arrive as raw strings so the API parses
/// them at the exact position the pre-cycle-2 CLI did (error-order
/// parity): the contract reference AFTER the execution-row check, the
/// verdict only when REVIEW_REPORT advances.
#[derive(Debug, Clone)]
pub struct PublishReq {
    pub id: String,
    pub kind: ArtifactKind,
    pub name: String,
    pub content: Vec<u8>,
    /// Artifact refs the result depends on (resolved to ids here).
    pub depends: Vec<String>,
    pub contract: Option<String>,
    pub execution: Option<String>,
    pub category: DataCategory,
    /// Verdict token for the REVIEW_REPORT advance phase (compound
    /// `artifact_publish` only; the phased CLI path hands it to
    /// `artifact_advance`). Parsed at the advance position, as before.
    pub verdict: Option<String>,
}

#[derive(Debug, Clone)]
pub struct PublishedRow {
    pub artifact: Artifact,
    /// The task row as resolved BEFORE publication (its state feeds the
    /// «state: X → Y» line of a later advance).
    pub task: Task,
}

/// The workflow advance certain artifact kinds carry
/// (`artifact.advance`, and the second phase of `artifact_publish`).
#[derive(Debug, Clone)]
pub struct AdvanceReq {
    pub id: String,
    pub artifact_id: String,
    pub kind: ArtifactKind,
    pub verdict: Option<String>,
}

#[derive(Debug, Clone)]
pub struct AdvanceOutcome {
    pub from: TaskState,
    pub to: TaskState,
    /// True when the empty-rework gate parked the task in
    /// WAITING_FOR_INPUT on an Implementation-Report advance.
    pub rework_gate: bool,
}

#[derive(Debug, Clone)]
pub struct PublishOutcome {
    pub publish: PublishedRow,
    pub advanced: Option<AdvanceOutcome>,
}

/// `task.depend` / `task.unlink` — typed dependency edges. The kinds are
/// parsed by the consumer from its argument text (`DependencyKind::parse`,
/// a core rule), the edges are written here.
#[derive(Debug, Clone)]
pub struct DependReq {
    pub id: String,
    pub on: String,
    pub kind: DependencyKind,
}

/// `task.unlink` result: the edge AS IT WAS while ACTIVE (the store hands
/// the superseded row back unchanged) plus who superseded it — the row's
/// own `actor` is the original author, never the superseder.
#[derive(Debug, Clone)]
pub struct UnlinkResult {
    pub edge: TaskDependency,
    pub superseded_by: String,
}

/// `task.override` — the human dependency-gate escape (ratified OQ2).
#[derive(Debug, Clone)]
pub struct OverrideResult {
    pub task: Task,
    pub override_blockers: Vec<String>,
}

/// `task.merge` — the PERSISTENCE half of the human-ordered merge. The
/// repository act itself (git merge into the checked-out baseline) stays
/// with the consumer that a human invoked: REQ-WT-07 placed the single
/// merge path behind the human CLI subcommand so no engine or runtime code
/// reaches it. This operation records the act as data (decision + event),
/// never touching task state.
#[derive(Debug, Clone)]
pub struct MergeRecordReq {
    pub id: String,
    pub branch: String,
    pub workspace: String,
    pub head_before: String,
    pub head_after: String,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone)]
pub struct MergeResult {
    pub task_id: Id,
    pub branch: String,
    pub workspace: String,
    pub head_before: String,
    pub head_after: String,
}

/// `task.ratify_empty_rework` — the EMPTY-REWORK GATE escape (REQ-RG-03):
/// records/reuses the pair-bound decision, releases the gate question,
/// runs the withheld transition. One operation, one set of facts.
#[derive(Debug, Clone)]
pub struct RatifyResult {
    /// The task as it stood before the withheld transition landed (its
    /// state feeds the «state: X → QA» line).
    pub task: Task,
    pub decision: Decision,
    pub recorded_now: bool,
    pub prev_report: String,
    pub current_report: String,
    /// The state the transition produced (QA — or the command failed).
    pub state: TaskState,
    pub allowed_operations: Vec<String>,
}

/// `question.ask` — raise a question; blocking parks the task through
/// `workflow::raise_question` exactly as before.
#[derive(Debug, Clone)]
pub struct AskReq {
    pub id: String,
    pub text: String,
    pub context: Option<String>,
    pub options: Vec<String>,
    pub blocking: bool,
    pub execution: Option<String>,
}

#[derive(Debug, Clone)]
pub struct QuestionAsked {
    pub question: Question,
    /// The task moved (blocking question parked it in WAITING_FOR_INPUT).
    pub parked: bool,
}

/// `question.answer` — the answered row plus the state transition the
/// answer drove (the old handler printed «state: X → Y»).
#[derive(Debug, Clone)]
pub struct AnsweredOutcome {
    pub question_id: Id,
    pub from_state: TaskState,
    pub to_state: TaskState,
}

/// `agent.add` — credentials stay outside the store, as before.
pub struct AgentAddReq {
    pub name: String,
    pub provider: String,
    pub model: String,
    pub runtime: String,
}

/// `contract.add` result — the registration as data (no unit proxy).
#[derive(Debug, Clone)]
pub struct ContractAddReq {
    pub name: String,
    pub version: u32,
    pub schema: String,
}

#[derive(Debug, Clone)]
pub struct ContractRegistration {
    pub name: String,
    pub version: u32,
}

/// `exec.start` — the platform-side facts (role version, workspace
/// revision, owner columns) are computed INSIDE the operation, never
/// supplied by the caller (REQ-001/REQ-006, REQ-SW-03).
#[derive(Debug, Clone)]
pub struct ExecStartReq {
    pub id: String,
    pub role: String,
    pub agent: Option<String>,
    pub inputs: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct ExecStarted {
    pub execution: RoleExecution,
    pub task: Task,
}

// ---------------------------------------------------------------------------
// operations
// ---------------------------------------------------------------------------

impl Api {
    // ---- project ----------------------------------------------------------

    pub fn project_add(&self, req: ProjectAddReq) -> ApiResult<Project> {
        let p = Project::new(req.name, req.description, req.repos, Utc::now());
        self.store.create_project(&p)?;
        Ok(p)
    }

    // ---- task: creation ---------------------------------------------------

    pub fn task_create(&self, req: TaskCreateReq) -> ApiResult<TaskCreated> {
        let t = Task::new(req.title, req.description, req.project_id, Utc::now());
        self.store.create_task(&t)?;
        // REQ-DEP-07: a derived task records WHY it exists at the moment it
        // is created; dangling references fail loudly on resolve, exactly
        // as they would via `task.depend`.
        let mut linked: Vec<TaskDependency> = Vec::new();
        for p in &req.parent {
            let pt = self.store.resolve_task(p)?;
            linked.push(self.store.link_task_dependency(t.id, pt.id, DependencyKind::Parent, &self.actor, Utc::now())?);
        }
        for b in &req.blocked_by {
            let bt = self.store.resolve_task(b)?;
            linked.push(self.store.link_task_dependency(t.id, bt.id, DependencyKind::Blocks, &self.actor, Utc::now())?);
        }
        Ok(TaskCreated { task: t, linked })
    }

    // ---- task: workflow dispatch (the former CLI `op()` helper) -----------

    /// Load task + artifacts, run one core workflow command, commit with
    /// optimistic locking — the single serialization point the old CLI op()
    /// was. Every named workflow operation delegates here (REQ-MUT-05 AC3).
    pub fn task_op(&self, id: &str, op: WorkflowOp) -> ApiResult<MutationResult> {
        match op {
            WorkflowOp::Start => self.run_op(id, |t, _a| workflow::start(t, &self.actor, Utc::now())),
            WorkflowOp::Review { verdict, summary } => self.run_op(id, move |t, _a| workflow::review_recorded(t, verdict, summary.as_deref(), &self.actor, Utc::now())),
            WorkflowOp::Approve { rationale } => self.run_op(id, move |t, a| {
                // The approve gate reads the CURRENT spec exactly as the old
                // closure did — same guard, same refusal text.
                if a.iter().any(|x| x.kind == ArtifactKind::RequirementsSpec && x.status == ArtifactStatus::Current) {
                    workflow::approve_requirements(t, rationale.as_deref(), &self.actor, Utc::now())
                } else {
                    Err(devai_core::CoreError::guard("no CURRENT REQUIREMENTS_SPEC artifact to approve"))
                }
            }),
            WorkflowOp::Reject { rationale } => self.run_op(id, move |t, _a| workflow::reject_requirements(t, rationale.as_deref(), &self.actor, Utc::now())),
            WorkflowOp::Accept { rationale } => {
                // REQ-DEP-03: the acceptance gate consults the stored
                // dependency graph and the override decisions before the
                // core command runs — the loads keep the old position.
                let task = self.store.resolve_task(id)?;
                let deps = self.store.task_dependencies(task.id)?;
                let decisions = self.store.list_decisions(task.id)?;
                self.run_op(id, move |t, a| workflow::accept(t, a, &deps, &decisions, rationale.as_deref(), &self.actor, Utc::now()))
            }
            WorkflowOp::Return { rationale } => self.run_op(id, move |t, _a| workflow::return_to_rework(t, rationale.as_deref(), &self.actor, Utc::now())),
            WorkflowOp::Pause { reason } => self.run_op(id, move |t, _a| workflow::pause(t, reason.as_deref(), &self.actor, Utc::now())),
            WorkflowOp::Resume => {
                let task = self.store.resolve_task(id)?;
                let deps = self.store.task_dependencies(task.id)?;
                let decisions = self.store.list_decisions(task.id)?;
                self.run_op(id, move |t, _a| workflow::resume(t, &deps, &decisions, &self.actor, Utc::now()))
            }
            WorkflowOp::Block { reason } => self.run_op(id, move |t, _a| workflow::block(t, reason.as_deref(), &self.actor, Utc::now())),
            WorkflowOp::Cancel { reason } => self.run_op(id, move |t, _a| workflow::cancel(t, reason.as_deref(), &self.actor, Utc::now())),
            WorkflowOp::Fail { reason } => self.run_op(id, move |t, _a| workflow::fail(t, reason.as_deref(), &self.actor, Utc::now())),
        }
    }

    fn run_op<F>(&self, id: &str, f: F) -> ApiResult<MutationResult>
    where
        F: FnOnce(&Task, &[Artifact]) -> devai_core::CoreResult<Mutation>,
    {
        let task = self.store.resolve_task(id)?;
        let artifacts = self.store.artifacts(task.id)?;
        let m = f(&task, &artifacts)?;
        let staled = m.artifact_status.iter().filter(|(_, s)| *s == ArtifactStatus::Stale).count();
        let superseded = m.artifact_status.iter().filter(|(_, s)| *s == ArtifactStatus::Superseded).count();
        self.store.commit(task.lock_version, &m)?;
        Ok(MutationResult {
            task: m.task.clone(),
            previous_state: task.state,
            event_kinds: m.events.iter().map(|e| e.kind.to_string()).collect(),
            staled,
            superseded,
            allowed_operations: workflow::allowed_ops(&m.task).into_iter().map(|s| s.to_string()).collect(),
        })
    }

    pub fn task_start(&self, id: &str) -> ApiResult<MutationResult> {
        self.task_op(id, WorkflowOp::Start)
    }
    pub fn task_review(&self, id: &str, verdict: Verdict, summary: Option<String>) -> ApiResult<MutationResult> {
        self.task_op(id, WorkflowOp::Review { verdict, summary })
    }
    pub fn task_reject(&self, id: &str, rationale: Option<String>) -> ApiResult<MutationResult> {
        self.task_op(id, WorkflowOp::Reject { rationale })
    }
    pub fn task_accept(&self, id: &str, rationale: Option<String>) -> ApiResult<MutationResult> {
        self.task_op(id, WorkflowOp::Accept { rationale })
    }
    pub fn task_return(&self, id: &str, rationale: Option<String>) -> ApiResult<MutationResult> {
        self.task_op(id, WorkflowOp::Return { rationale })
    }
    pub fn task_pause(&self, id: &str, reason: Option<String>) -> ApiResult<MutationResult> {
        self.task_op(id, WorkflowOp::Pause { reason })
    }
    pub fn task_resume(&self, id: &str) -> ApiResult<MutationResult> {
        self.task_op(id, WorkflowOp::Resume)
    }
    pub fn task_block(&self, id: &str, reason: Option<String>) -> ApiResult<MutationResult> {
        self.task_op(id, WorkflowOp::Block { reason })
    }
    pub fn task_cancel(&self, id: &str, reason: Option<String>) -> ApiResult<MutationResult> {
        self.task_op(id, WorkflowOp::Cancel { reason })
    }
    pub fn task_fail(&self, id: &str, reason: Option<String>) -> ApiResult<MutationResult> {
        self.task_op(id, WorkflowOp::Fail { reason })
    }

    /// APPROVAL → IMPLEMENTATION plus the `requirements_approval` decision
    /// row (the rationale becomes auditable data, precedent 01a1a0b5-0fcf).
    /// The spec-ref load and the refusal positions mirror the old handler.
    pub fn task_approve(&self, id: &str, rationale: Option<String>) -> ApiResult<ApproveOutcome> {
        let task = self.store.resolve_task(id)?;
        let spec_ref = self
            .store
            .artifacts(task.id)?
            .iter()
            .find(|x| x.kind == ArtifactKind::RequirementsSpec && x.status == ArtifactStatus::Current)
            .map(|x| x.id.to_string())
            .unwrap_or_default();
        let mutation = self.task_op(id, WorkflowOp::Approve { rationale: rationale.clone() })?;
        let decision = Decision {
            id: new_id(),
            task_id: task.id,
            kind: decision_kind::REQUIREMENTS_APPROVAL.into(),
            verdict: Verdict::Approved,
            actor: self.actor.clone(),
            rationale,
            refs: if spec_ref.is_empty() { Vec::new() } else { vec![spec_ref] },
            created_at: Utc::now(),
        };
        self.store.record_decision(&decision)?;
        Ok(ApproveOutcome { mutation, decision })
    }

    // ---- task: run / revise ------------------------------------------------

    pub fn task_run(&self, req: RunReq) -> ApiResult<RunOutcome> {
        // REQ-SW-05: the labeled early single-writer refusal. A pre-check
        // failure on an unknown id resolves inside the engine exactly as
        // before; the enforcement point stays the store index.
        if let Ok(task) = self.store.resolve_task(&req.id) {
            self.guard_running_writer(&task)?;
        }
        let mut engine = Engine::new(&self.store).with_force_reaudit(req.force_reaudit);
        if let Some(w) = req.workspace.as_deref() {
            engine = engine.with_workspace(w);
        }
        let report = engine.advance(&req.id, &req.agent, &self.actor)?;
        // The gate renderings read the CURRENT quality report — the same
        // status-only selection (REQ-005) the old handler performed.
        let (current_quality_report, gate_report_text) = match report.gate {
            Gate::WaitingAccept => (None, self.quality_report_text(report.task_id)?),
            Gate::ReportFresh => (
                self.store.artifacts(report.task_id)?.into_iter().find(|x| x.kind == ArtifactKind::QualityReport && x.status == ArtifactStatus::Current),
                self.quality_report_text(report.task_id)?,
            ),
            _ => (None, None),
        };
        let t = self.store.get_task(&report.task_id.to_string())?;
        Ok(RunOutcome {
            report,
            current_quality_report,
            gate_report_text,
            allowed_operations: workflow::allowed_ops(&t).into_iter().map(|s| s.to_string()).collect(),
        })
    }

    pub fn task_revise(&self, req: ReviseReq) -> ApiResult<ReviseOutcome> {
        let task = self.store.resolve_task(&req.id)?;
        let _a = self.publish_row_internal(task.id, ArtifactKind::RequirementsSpec, &req.name, req.content, None, None, vec![], None, DataCategory::Artifact)?;
        let after = self.store.artifacts(task.id)?;
        let m = workflow::requirements_published(&task, &after, req.reason.as_deref(), &self.actor, Utc::now())?;
        self.store.commit(task.lock_version, &m)?;
        Ok(ReviseOutcome {
            task: m.task.clone(),
            staled_ids: m.artifact_status.iter().filter(|(_, s)| *s == ArtifactStatus::Stale).map(|(i, _)| i.to_string()).collect(),
            allowed_operations: workflow::allowed_ops(&m.task).into_iter().map(|s| s.to_string()).collect(),
        })
    }

    // ---- artifacts ---------------------------------------------------------

    /// Publish the artifact row through the contract gate — WITHOUT the
    /// workflow advance. Exposed beside `artifact_publish` so a consumer
    /// that renders between the two effects (the CLI prints the artifact
    /// row before a failing advance aborts) keeps byte parity (REQ-MUT-06).
    pub fn artifact_publish_row(&self, req: PublishReq) -> ApiResult<PublishedRow> {
        let task = self.store.resolve_task(&req.id)?;
        let mut depends_ids: Vec<Id> = Vec::new();
        for p in &req.depends {
            depends_ids.push(self.store.get_artifact(p)?.id);
        }
        // Downstream role results are implicitly children of the current
        // approved specification (a later requirements revision marks them
        // STALE through the dependency graph — unless deps were given).
        if req.depends.is_empty() && req.kind.is_downstream_of_spec() {
            if let Some(spec) = self.store.artifacts(task.id)?.iter().find(|a| a.kind == ArtifactKind::RequirementsSpec && a.status == ArtifactStatus::Current).map(|a| a.id) {
                depends_ids.push(spec);
            }
        }
        let execution_id = match req.execution.as_deref() {
            Some(e) => {
                // Ratified OQ-4: publication into a closed row is a LOUD
                // refusal — «зомби не пишет в закрытую строку».
                let ex = self.store.get_execution(e)?;
                if ex.status != ExecutionStatus::Running {
                    return Err(ApiError::refused("execution_not_running", format!(
                        "execution {} is {}, not RUNNING — artifact publication requires the producing execution to be RUNNING (a closed row stays closed: work that continues starts a new execution)",
                        ex.id, ex.status.as_str()
                    )));
                }
                Some(ex.id)
            }
            None => None,
        };
        let created_by = execution_id.map(|id| format!("execution:{}", id));
        // The contract reference parses AFTER the execution check — the
        // exact position the pre-cycle-2 CLI held (error-order parity).
        let contract = match req.contract.as_deref() {
            Some(c) => Some(ContractRef::parse(c).map_err(devai_core::CoreError::from)?),
            None => None,
        };
        let artifact = self.publish_row_internal(task.id, req.kind, &req.name, req.content, contract, execution_id, depends_ids, created_by.as_deref(), req.category)?;
        Ok(PublishedRow { artifact, task })
    }

    /// The workflow advance certain artifact kinds carry — the SAME core
    /// commands the engine runs (REQ-RG-01: no producer path bypasses the
    /// empty-rework gate). Membership of the advancing set is owned by
    /// core (`ArtifactKind::advances_state`, QA R4); a standalone call for
    /// a NON-advancing kind refuses fail-closed with a machine code
    /// (QA R5) — the silent `from == to` no-op is gone, so a kind that
    /// later joins the advancing set can only be dispatched loudly (arm
    /// below) or refused loudly, never quietly skipped.
    pub fn artifact_advance(&self, req: AdvanceReq) -> ApiResult<AdvanceOutcome> {
        let task = self.store.resolve_task(&req.id)?;
        let a = self.store.get_artifact(&req.artifact_id)?;
        match req.kind {
            ArtifactKind::RequirementsSpec => {
                let arts = self.store.artifacts(task.id)?;
                let m = workflow::requirements_published(&task, &arts, None, &self.actor, Utc::now())?;
                self.store.commit(task.lock_version, &m)?;
                Ok(AdvanceOutcome { from: task.state, to: m.task.state, rework_gate: false })
            }
            ArtifactKind::ReviewReport => {
                let v = match req.verdict.as_deref() {
                    Some(s) => Self::parse_verdict(s)?,
                    None => return Err(ApiError::refused("bad_request", "publishing REVIEW_REPORT requires --verdict approved|needs_revision")),
                };
                let m = workflow::review_recorded(&task, v, None, &self.actor, Utc::now())?;
                self.store.commit(task.lock_version, &m)?;
                Ok(AdvanceOutcome { from: task.state, to: m.task.state, rework_gate: false })
            }
            ArtifactKind::ImplementationReport => {
                // The manual publication crosses the SAME core command as
                // the engine — including the empty-rework gate.
                let arts = self.store.artifacts(task.id)?;
                let facts = devai_runtime::gate::collect_implementation_report_facts(&self.store, task.id, &a, &arts, None)?;
                let m = workflow::implementation_reported(&task, &facts, &self.actor, Utc::now())?;
                self.store.commit(task.lock_version, &m)?;
                Ok(AdvanceOutcome { from: task.state, to: m.task.state, rework_gate: m.task.state == TaskState::WaitingForInput })
            }
            // Fail-closed (QA R5): the kind carries no advance. The refusal
            // lists the advancing kinds DERIVED from the core predicate —
            // never a hand-copied set that could drift (QA R4).
            other => Err(ApiError::refused("artifact_kind_not_advancing", format!(
                "{} carries no workflow advance — the advancing kinds are {} (the row alone lands via `artifact.publish_row`; publish through `artifact.publish` to get the row and its advance in one call)",
                other.as_str(),
                ArtifactKind::ALL.iter().filter(|k| k.advances_state()).map(|k| k.as_str()).collect::<Vec<_>>().join(", ")
            ))),
        }
    }

    /// `artifact.publish` — the compound operation: the row through the
    /// contract gate, then the kind-carried advance. Identical store
    /// effects to the two phases called in sequence. Membership of the
    /// advancing set comes from the core predicate (`advances_state`,
    /// QA R4) — the same single source the standalone advance and the
    /// consumer render derive from.
    pub fn artifact_publish(&self, req: PublishReq) -> ApiResult<PublishOutcome> {
        let kind = req.kind;
        let verdict = req.verdict.clone();
        let publish = self.artifact_publish_row(req)?;
        let advanced = if kind.advances_state() {
            Some(self.artifact_advance(AdvanceReq { id: publish.task.id.to_string(), artifact_id: publish.artifact.id.to_string(), kind, verdict })?)
        } else {
            None
        };
        Ok(PublishOutcome { publish, advanced })
    }

    /// Parse an operator verdict token. The synonym dictionary is core
    /// vocabulary (`Verdict::from_token`, QA R3 — the API owns NO business
    /// rules, only delegation); the API maps the miss to the machine-coded
    /// refusal carrying the exact CLI text pinned by parity (REQ-MUT-06).
    pub fn parse_verdict(s: &str) -> ApiResult<Verdict> {
        Verdict::from_token(s).ok_or_else(|| ApiError::refused("unknown_variant", format!("unknown verdict `{}` ({})", s.to_ascii_lowercase(), Verdict::token_hint())))
    }

    /// The old CLI `publish_raw`: unregistered contract and contract-gate
    /// violations refuse with machine-readable codes carrying the exact
    /// operator texts (REQ-MUT-08); the presentation (the warning lines)
    /// belongs to the consumer, which reads the structured violations.
    #[allow(clippy::too_many_arguments)]
    fn publish_row_internal(
        &self,
        task_id: Id,
        kind: ArtifactKind,
        name: &str,
        content: Vec<u8>,
        contract: Option<ContractRef>,
        execution: Option<Id>,
        depends: Vec<Id>,
        created_by: Option<&str>,
        category: DataCategory,
    ) -> ApiResult<Artifact> {
        if let Some(c) = &contract {
            let schema = self.store.get_contract(&c.name, c.version)?;
            let Some(schema) = schema else {
                return Err(ApiError::ContractUnregistered { contract: c.to_string() });
            };
            // Gate parity (ratified OQ-FG-05, REQ-FG-08): the manual path
            // runs the SAME contract gate as the engine loop; enforcement
            // is the API's, the presentation (warning + full list) the
            // consumer's.
            let violations = devai_runtime::contract_violations(String::from_utf8_lossy(&content).as_ref(), Some(schema.as_str()))?;
            if !violations.is_empty() {
                return Err(ApiError::ContractRejected { contract: c.to_string(), violations });
            }
        }
        Ok(self.store.publish_artifact(ArtifactDraft {
            task_id,
            kind,
            name: name.to_string(),
            category,
            media_type: guess_media_type(Path::new(name)),
            content,
            contract,
            created_by: created_by.unwrap_or(&self.actor).to_string(),
            execution_id: execution,
            depends_on: depends,
        })?)
    }

    // ---- task: dependencies -----------------------------------------------

    pub fn task_depend(&self, req: DependReq) -> ApiResult<TaskDependency> {
        let task = self.store.resolve_task(&req.id)?;
        let depends_on = self.store.resolve_task(&req.on)?;
        // Registration is LOUD at write (REQ-DEP-07 AC2): self-reference,
        // duplicate, dangling or an intra-kind cycle (ratified OQ4) never
        // land — the store enforces, unchanged.
        Ok(self.store.link_task_dependency(task.id, depends_on.id, req.kind, &self.actor, Utc::now())?)
    }

    pub fn task_unlink(&self, req: DependReq) -> ApiResult<UnlinkResult> {
        let task = self.store.resolve_task(&req.id)?;
        let depends_on = self.store.resolve_task(&req.on)?;
        // RATIFIED DECISION OQ7: edges are IMMUTABLE — removal is a
        // SUPERSEDED marker; the row stays as history forever.
        let edge = self.store.supersede_task_dependency(task.id, depends_on.id, req.kind, &self.actor, Utc::now())?;
        Ok(UnlinkResult { edge, superseded_by: self.actor.clone() })
    }

    pub fn task_override(&self, id: &str, blocker: Vec<String>, rationale: Option<String>) -> ApiResult<OverrideResult> {
        // RATIFIED DECISION OQ2 — the human escape of the dependency gate.
        // `refs` = the FULL task ids covered, so a NEW blocker is never
        // covered by an old decision (the gate stands for anything not
        // named here).
        let task = self.store.resolve_task(id)?;
        let mut blockers: Vec<String> = Vec::new();
        for b in &blocker {
            blockers.push(self.store.resolve_task(b)?.id.to_string());
        }
        if blockers.is_empty() {
            // Without explicit blockers, ratify against exactly the
            // blockers OPEN right now (computed from stored data).
            let deps = self.store.task_dependencies(task.id)?;
            blockers = workflow::open_blockers(&task, &deps);
            if blockers.is_empty() {
                return Err(ApiError::refused("bad_request", format!("task {} has no open blocking dependencies — nothing to override", short_id(&task.id))));
            }
        }
        self.store.record_decision(&Decision {
            id: new_id(),
            task_id: task.id,
            kind: decision_kind::DEPENDENCY_OVERRIDE.into(),
            verdict: Verdict::Approved,
            actor: self.actor.clone(),
            rationale: rationale.clone(),
            refs: blockers.clone(),
            created_at: Utc::now(),
        })?;
        self.store.insert_event(Some(task.id), event_kind::TASK_DEPENDENCY_OVERRIDDEN, &self.actor, serde_json::json!({ "blockers": blockers }))?;
        Ok(OverrideResult { task, override_blockers: blockers })
    }

    pub fn task_merge_record(&self, req: MergeRecordReq) -> ApiResult<MergeResult> {
        // The MERGE GATE persistence (REQ-WT-07): the repository act was
        // performed by the human-invoked command; this records it as data.
        // Task state is untouched by design — a merge is not a workflow
        // transition.
        let task = self.store.resolve_task(&req.id)?;
        self.store.record_decision(&Decision {
            id: new_id(),
            task_id: task.id,
            kind: decision_kind::REWORK_MERGE.into(),
            verdict: Verdict::Approved,
            actor: self.actor.clone(),
            rationale: req.rationale.clone(),
            refs: vec![req.branch.clone(), req.head_after.clone()],
            created_at: Utc::now(),
        })?;
        self.store.insert_event(
            Some(task.id),
            event_kind::REWORK_MERGED,
            &self.actor,
            serde_json::json!({
                "branch": req.branch,
                "workspace": req.workspace,
                "head_before": req.head_before,
                "head_after": req.head_after,
            }),
        )?;
        Ok(MergeResult { task_id: task.id, branch: req.branch, workspace: req.workspace, head_before: req.head_before, head_after: req.head_after })
    }

    /// The EMPTY-REWORK GATE escape (REQ-RG-03), human-only like the merge
    /// gate: records (or reuses) the pair-bound `rework_noop_ratified`
    /// decision, releases the gate's blocking question, and runs the
    /// withheld REWORK → QA transition through the same core command —
    // which now finds the ratification and cites its decision id (AC3).
    pub fn task_ratify_empty_rework(&self, id: &str, rationale: Option<String>) -> ApiResult<RatifyResult> {
        let task = self.store.resolve_task(id)?;
        match task.state {
            // The gate hold is WAITING_FOR_INPUT returning to REWORK. If
            // the operator answered the question first (back to REWORK, no
            // ratification) — or a previous run died between the two acts
            // — ratifying from REWORK converges here too.
            TaskState::WaitingForInput if task.return_state == Some(TaskState::Rework) => {}
            TaskState::Rework => {}
            s => return Err(ApiError::refused("empty_rework_gate", format!(
                "ratify-empty-rework applies to the empty-rework gate (REWORK, or WAITING_FOR_INPUT returning to REWORK); the task is in {}",
                s.as_str()
            ))),
        }
        let arts = self.store.artifacts(task.id)?;
        let Some(current) = arts.iter().filter(|a| a.kind == ArtifactKind::ImplementationReport).max_by_key(|a| a.seq) else {
            return Err(ApiError::refused("empty_rework_gate", "the task has no implementation report — nothing to ratify"));
        };
        let events = self.store.all_events_for_task(task.id)?;
        let current_id_s = current.id.to_string();
        // Only the trip of the LATEST report is ratifiable (REQ-RG-03 AC4).
        let trip = events
            .iter()
            .rev()
            .find(|e| e.kind == event_kind::QA_REWORK_GATE_TRIPPED && e.payload.get(json_field::CURRENT_REPORT).and_then(|v| v.as_str()) == Some(current_id_s.as_str()))
            .ok_or_else(|| ApiError::refused("empty_rework_gate", format!(
                "no empty-rework gate recorded for report {} — the gate fires when a REWORK publication names the unchanged commit; if the latest report is newer, let the engine publish it from REWORK first",
                short_id(&current.id)
            )))?;
        let prev_report = trip
            .payload
            .get(json_field::PREV_REPORT)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| ApiError::refused("corrupt", "the gate trip event names no previous report — history corruption"))?
            .to_string();

        // The ratification for exactly this pair, recorded once; a retry
        // reuses the stored row (never a double-written judgment).
        let decisions = self.store.list_decisions(task.id)?;
        let existing = decisions
            .iter()
            .find(|d| d.kind == decision_kind::REWORK_NOOP_RATIFIED && d.refs.iter().any(|r| r == &prev_report) && d.refs.iter().any(|r| r == &current_id_s))
            .cloned();
        let (decision, recorded_now) = match existing {
            Some(d) => (d, false),
            None => {
                let d = Decision {
                    id: new_id(),
                    task_id: task.id,
                    kind: decision_kind::REWORK_NOOP_RATIFIED.into(),
                    verdict: Verdict::Approved,
                    actor: self.actor.clone(),
                    rationale,
                    refs: vec![prev_report.clone(), current_id_s.clone()],
                    created_at: Utc::now(),
                };
                self.store.record_decision(&d)?;
                (d, true)
            }
        };

        // Release the blocking gate question while the hold is still up.
        let mut cur = task.clone();
        if cur.state == TaskState::WaitingForInput {
            let q_ref = trip.payload.get(json_field::QUESTION_ID).and_then(|v| v.as_str()).unwrap_or_default();
            let q = self.store.get_question(q_ref)?;
            if q.status == QuestionStatus::Open {
                self.store.answer_question(q.id, &format!("operator ratified the empty rework (decision {})", decision.id), &self.actor, Utc::now())?;
            }
            let m = workflow::answer_question(&cur, q.id, &self.actor, Utc::now())?;
            self.store.commit(cur.lock_version, &m)?;
            cur = self.store.get_task(&cur.id.to_string())?;
        }

        // Run the withheld transition through the single serialization
        // point; with the stored ratification in the facts, the core
        // command admits REWORK → QA and puts `ratified_by` on the event.
        let arts = self.store.artifacts(cur.id)?;
        let facts = devai_runtime::gate::collect_implementation_report_facts(&self.store, cur.id, current, &arts, Some(format!("empty rework ratified by decision {}", decision.id)))?;
        let m = workflow::implementation_reported(&cur, &facts, &self.actor, Utc::now())?;
        if m.task.state != TaskState::Qa {
            return Err(ApiError::refused("empty_rework_gate", format!(
                "the withheld transition did not land in QA (state {}); decision {} stands — inspect `devai task events {}`",
                m.task.state.as_str(),
                decision.id,
                short_id(&cur.id)
            )));
        }
        self.store.commit(cur.lock_version, &m)?;
        Ok(RatifyResult {
            task: cur,
            decision,
            recorded_now,
            prev_report,
            current_report: current_id_s,
            state: m.task.state,
            allowed_operations: workflow::allowed_ops(&m.task).into_iter().map(|s| s.to_string()).collect(),
        })
    }

    // ---- questions --------------------------------------------------------

    pub fn question_ask(&self, req: AskReq) -> ApiResult<QuestionAsked> {
        let task = self.store.resolve_task(&req.id)?;
        let execution_id = match req.execution.as_deref() {
            Some(e) => Some(self.store.get_execution(e)?.id),
            None => None,
        };
        let q = Question {
            id: new_id(),
            task_id: task.id,
            execution_id,
            text: req.text,
            context: req.context,
            options: req.options,
            blocking: req.blocking,
            status: QuestionStatus::Open,
            answer: None,
            answered_by: None,
            answered_at: None,
            created_at: Utc::now(),
        };
        self.store.create_question(&q)?;
        let m = workflow::raise_question(&task, q.id, q.blocking, &self.actor, Utc::now())?;
        self.store.commit(task.lock_version, &m)?;
        Ok(QuestionAsked { question: q, parked: req.blocking && m.task.state != task.state })
    }

    pub fn question_answer(&self, question_id: &str, text: &str) -> ApiResult<AnsweredOutcome> {
        let q = self.store.get_question(question_id)?;
        if q.status == QuestionStatus::Answered {
            return Err(ApiError::refused("bad_request", format!("question {} is already answered", q.id)));
        }
        let task = self.store.get_task(&q.task_id.to_string())?;
        self.store.answer_question(q.id, text, &self.actor, Utc::now())?;
        let m = workflow::answer_question(&task, q.id, &self.actor, Utc::now())?;
        self.store.commit(task.lock_version, &m)?;
        Ok(AnsweredOutcome { question_id: q.id, from_state: task.state, to_state: m.task.state })
    }

    // ---- roles / agents / contracts ----------------------------------------

    /// `role.add`: version 0 in the definition means latest+1 for the slug,
    /// a nil id gets a fresh one, `created_at` is stamped here (exactly the
    /// old CLI defaulting, now behind the API).
    pub fn role_add(&self, mut r: RoleDefinition) -> ApiResult<RoleDefinition> {
        if r.version == 0 {
            r.version = self.store.get_role(&r.slug)?.map(|e| e.version + 1).unwrap_or(1);
        }
        if r.id == Uuid::nil() {
            r.id = new_id();
        }
        r.created_at = Utc::now();
        self.store.put_role(&r)?;
        Ok(r)
    }

    pub fn agent_add(&self, req: AgentAddReq) -> ApiResult<AgentInstance> {
        let a = AgentInstance { id: new_id(), name: req.name.clone(), provider: req.provider, model: req.model, runtime: req.runtime, created_at: Utc::now() };
        self.store.put_agent(&a)?;
        Ok(a)
    }

    pub fn contract_add(&self, req: ContractAddReq) -> ApiResult<ContractRegistration> {
        self.store.put_contract(&req.name, req.version, &req.schema)?;
        Ok(ContractRegistration { name: req.name, version: req.version })
    }

    // ---- executions --------------------------------------------------------

    pub fn exec_start(&self, req: ExecStartReq) -> ApiResult<ExecStarted> {
        let task = self.store.resolve_task(&req.id)?;
        workflow::check_execution_allowed(&req.role, &task)?;
        // REQ-SW-02 AC2: the labeled early refusal of a second RUNNING row
        // — before the index would reject it, in the old order (check,
        // then guard).
        self.guard_running_writer(&task)?;
        let rd = self.store.get_role(&req.role)?;
        let version = rd.as_ref().map(|r| r.version).unwrap_or(1);
        let inputs_str: Vec<String> = {
            let mut v = Vec::new();
            for p in &req.inputs {
                v.push(self.store.get_artifact(p)?.id.to_string());
            }
            v
        };
        // REQ-001/REQ-006: even a manually started execution binds to the
        // workspace state the platform observes right now — computed
        // platform-side, never supplied by the caller.
        let revision = Engine::new(&self.store).current_revision()?;
        // REQ-SW-03: the owner columns are captured platform-side — this
        // process is the one OPENING the row (the orphan detect, REQ-SW-04,
        // finds exactly that when it exits with the row still RUNNING).
        let (owner_pid, owner_host, owner_process_start) = liveness::current_owner().into_columns();
        let e = RoleExecution {
            id: new_id(),
            task_id: task.id,
            role_slug: req.role,
            role_version: version,
            agent_name: req.agent,
            status: ExecutionStatus::Running,
            inputs: inputs_str,
            summary: None,
            error: None,
            started_at: Utc::now(),
            finished_at: None,
            workspace_revision: Some(revision),
            // A manual start is its own root attempt (REQ-FG-06).
            attempt: 1,
            parent_execution: None,
            failure_class: None,
            owner_pid,
            owner_host,
            owner_process_start,
        };
        self.store.start_execution(&e)?;
        Ok(ExecStarted { execution: e, task })
    }

    pub fn exec_finish(&self, exec_id: &str, summary: Option<String>) -> ApiResult<RoleExecution> {
        self.exec_close(exec_id, ExecutionStatus::Completed, summary.as_deref(), None)
    }

    pub fn exec_fail(&self, exec_id: &str, error: &str) -> ApiResult<RoleExecution> {
        self.exec_close(exec_id, ExecutionStatus::Failed, None, Some(error))
    }

    pub fn exec_cancel(&self, exec_id: &str) -> ApiResult<RoleExecution> {
        self.exec_close(exec_id, ExecutionStatus::Cancelled, None, None)
    }

    fn exec_close(&self, exec_id: &str, status: ExecutionStatus, summary: Option<&str>, error: Option<&str>) -> ApiResult<RoleExecution> {
        let e = self.store.get_execution(exec_id)?;
        if e.status != ExecutionStatus::Running {
            return Err(ApiError::refused("execution_not_running", format!("execution {} is {}, not RUNNING", e.id, e.status.as_str())));
        }
        self.store.finish_execution(&e, status, summary, error, Utc::now())?;
        Ok(e)
    }

    // ---- export / import ----------------------------------------------------

    pub fn import_bundle(&self, dir: &Path) -> ApiResult<bundle::ImportOutcome> {
        Ok(bundle::import(&self.store, dir)?)
    }

    // ---- bootstrap (human-ratified Q2: encapsulated, never registered) ------

    /// Open a home through the API: the store handle never surfaces to a
    /// consumer (REQ-MUT-03/04). This is the bootstrap entry the ratified
    /// OQ-10 exception always described — now with the handle owned here.
    /// (Lives in lib.rs beside `Api`'s fields; declared reachable so the
    /// doc above stays single-sourced.)
    ///
    /// `init` seeding, encapsulated: `None` seeds the built-in defaults
    /// into `<home>/config`, `Some(dir)` seeds from the given config tree.
    /// Returns the number of seeded/updated role versions — the exact
    /// facts the init handler renders. Not an operation of the registry:
    /// bootstrap runs when the domain has none.
    pub fn seed_config(&self, config_dir: Option<&Path>) -> ApiResult<usize> {
        Ok(match config_dir {
            Some(dir) => seed::seed_from(&self.store, dir, Utc::now())?,
            None => seed::seed_defaults(&self.store, Utc::now())?,
        })
    }

    // ---- internal -----------------------------------------------------------

    /// REQ-SW-02/05: the labeled refusal of a second writer, shown BEFORE
    /// an attempt is made when the task already carries a RUNNING
    /// execution. The ENFORCEMENT point stays the SQLite partial unique
    /// index (a race between the read and the INSERT still lands on
    /// `StoreError::SingleWriterBusy`). The refusal is distinct per
    /// ratified liveness class (OQ-3/OQ-5); the only resolution is closing
    /// the row with `exec finish|fail|cancel` — the process is never
    /// touched (REQ-SW-06). The message keeps the operator-facing text the
    /// CLI printed while this check lived there (parity, REQ-MUT-06); the
    /// machine token is `single_writer_busy`.
    fn guard_running_writer(&self, task: &Task) -> ApiResult<()> {
        let blocker = self.store.list_executions(Some(task.id))?.into_iter().find(|e| e.status == ExecutionStatus::Running);
        let Some(e) = blocker else { return Ok(()) };
        let owner = match (e.owner_pid, e.owner_host.as_deref()) {
            (Some(pid), Some(host)) => format!("pid {pid}@{host}"),
            (Some(pid), None) => format!("pid {pid} (host unrecorded)"),
            (None, _) => "owner unrecorded (row predates the v7 guard)".to_string(),
        };
        let verdict = match liveness::check_owner(e.owner_pid, e.owner_host.as_deref(), e.owner_process_start) {
            liveness::Liveness::Alive => format!("a LIVE writer is in flight ({owner}) — wait for it to finish, or stop it explicitly"),
            liveness::Liveness::Dead => format!("an ORPHAN ({owner} is no longer running) — nobody is collecting this writer's work"),
            liveness::Liveness::Unknown => format!("a writer whose owner cannot be verified ({owner}) — never assumed dead"),
        };
        Err(ApiError::refused("single_writer_busy", format!(
            "single-writer guard: task `{}` already has a RUNNING execution `{}` — {} ({}). The second writer is refused by design: no queueing, no overwrite. Resolve the open row first (a human decision, the process is not touched): `devai exec finish|fail|cancel {}`",
            task.id, e.id, verdict, short_id(&e.id), e.id
        )))
    }

    /// The §2.6 rendering of the CURRENT quality report, when one is
    /// present (the human gate's viewing channel; reports assembled before
    /// @2 render the placeholder the old CLI printed). `Ok(None)` is the
    /// honest «no CURRENT quality report»; a store failure or an unreadable
    /// (non-JSON) blob propagates as `Err` with its machine code — never a
    /// silent «absence» (QA R2: `.ok()?` swallowed corrupt data into None,
    /// indistinguishable from «no report»).
    pub fn quality_report_text(&self, task_id: Id) -> ApiResult<Option<String>> {
        let arts = self.store.artifacts(task_id)?;
        // REQ-005: status-only selection (single CURRENT pinned at v3).
        let Some(a) = arts.iter().find(|a| a.kind == ArtifactKind::QualityReport && a.status == ArtifactStatus::Current) else {
            return Ok(None);
        };
        let blob = self.store.read_blob(&a.sha256)?;
        let v: serde_json::Value = serde_json::from_slice(&blob).map_err(|e| ApiError::refused("corrupt", format!(
            "the CURRENT quality report {} is stored but does not parse as JSON: {e} (re-publish the report; the gate cannot render a corrupt blob as «no report»)",
            short_id(&a.id)
        )))?;
        Ok(v.get("report_text")
            .and_then(|t| t.as_str())
            .map(|s| s.to_string())
            .or_else(|| Some("(report assembled before @2 — no rendering)".into())))
    }
}

// ---------------------------------------------------------------------------
// tests (REQ-MUT-05 AC3: dispatch == dedicated; REQ-MUT-06: effect parity)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::ErrorCode;

    fn api(dir: &Path) -> Api {
        Api::open(dir, "api-tester").expect("open api")
    }

    fn fresh_task(api: &Api, title: &str) -> Id {
        api.task_create(TaskCreateReq { title: title.into(), project_id: None, description: String::new(), parent: vec![], blocked_by: vec![] })
            .expect("create")
            .task
            .id
    }

    /// REQ-MUT-05 AC3: the named operation and the `task_op` dispatch run
    /// the SAME path — identical mutation facts, only task ids differ.
    #[test]
    fn named_operation_equals_dispatch() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let a = fresh_task(&api, "dispatch-a");
        let b = fresh_task(&api, "dispatch-b");

        let via_dispatch = api.task_op(&a.to_string(), WorkflowOp::Start).unwrap();
        let via_named = api.task_start(&b.to_string()).unwrap();

        assert_eq!(via_dispatch.event_kinds, via_named.event_kinds);
        assert_eq!(via_dispatch.previous_state, via_named.previous_state);
        assert_eq!(via_dispatch.task.state, via_named.task.state);
        assert_eq!(via_dispatch.staled, via_named.staled);
        assert_eq!(via_dispatch.superseded, via_named.superseded);
        assert_eq!(via_dispatch.allowed_operations, via_named.allowed_operations);
        assert_eq!(via_named.event_kinds, vec![event_kind::WORKFLOW_STARTED.to_string()]);
    }

    /// Approve through the operation writes BOTH the transition and the
    /// requirements_approval decision (the QA-fed fact, precedent
    /// 01a1a0b5-0fcf), and refuses without a CURRENT spec — the same guard
    /// text the pre-cycle-2 CLI closure produced.
    #[test]
    fn approve_carries_decision_and_guard() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let t = fresh_task(&api, "approve-me");
        let id = t.to_string();
        api.task_start(&id).unwrap();

        // No spec yet → the guard refuses with the old text, no decision.
        let e = api.task_approve(&id, Some("early".into())).unwrap_err();
        assert_eq!(e.code(), ErrorCode("guard"));
        assert!(format!("{e}").contains("no CURRENT REQUIREMENTS_SPEC artifact to approve"), "{e}");
        assert!(api.task_decisions(&id).unwrap().is_empty());

        // Publish the spec through the compound operation, review it in,
        // then approve: mutation + decision row land together.
        let content = br#"{"requirements": "ok"}"#.to_vec();
        api.artifact_publish(PublishReq { id: id.clone(), kind: ArtifactKind::RequirementsSpec, name: "spec.json".into(), content, depends: vec![], contract: None, execution: None, category: DataCategory::Artifact, verdict: None })
            .expect("publish spec");
        api.task_review(&id, Verdict::Approved, None).unwrap();
        let o = api.task_approve(&id, Some("ratified".into())).unwrap();
        assert_eq!(o.mutation.task.state, TaskState::Implementation);
        let ds = api.task_decisions(&id).unwrap();
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].kind, decision_kind::REQUIREMENTS_APPROVAL);
        assert_eq!(ds[0].rationale.as_deref(), Some("ratified"));
        assert_eq!(ds[0].refs.len(), 1, "the decision refs bind the CURRENT spec id");
    }

    /// The compound `artifact_publish` and the phased
    /// `artifact_publish_row` + `artifact_advance` converge on the SAME
    /// store effects — the registry dispatch and the CLI command execute
    /// one path (REQ-MUT-05 AC3, REQ-MUT-06).
    #[test]
    fn compound_publish_equals_phased_calls() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let a = fresh_task(&api, "pub-compound");
        let b = fresh_task(&api, "pub-phased");
        // Same precondition as the pre-cycle-2 path: requirements can only
        // be published from ANALYSIS onward.
        api.task_start(&a.to_string()).unwrap();
        api.task_start(&b.to_string()).unwrap();
        let content = |x: &str| x.as_bytes().to_vec();

        let compound = api
            .artifact_publish(PublishReq { id: a.to_string(), kind: ArtifactKind::RequirementsSpec, name: "s1".into(), content: content("s1"), depends: vec![], contract: None, execution: None, category: DataCategory::Artifact, verdict: None })
            .unwrap();
        let row = api
            .artifact_publish_row(PublishReq { id: b.to_string(), kind: ArtifactKind::RequirementsSpec, name: "s2".into(), content: content("s2"), depends: vec![], contract: None, execution: None, category: DataCategory::Artifact, verdict: None })
            .unwrap();
        let adv = api
            .artifact_advance(AdvanceReq { id: b.to_string(), artifact_id: row.artifact.id.to_string(), kind: ArtifactKind::RequirementsSpec, verdict: None })
            .unwrap();

        assert_eq!(compound.publish.artifact.kind, row.artifact.kind);
        assert_eq!(compound.publish.artifact.seq, row.artifact.seq);
        assert_eq!(compound.publish.task.state, TaskState::Analysis, "compound advance ran too: the task moved");
        // Both paths converge on the same transition facts.
        let after_compound = api.task_get(&a.to_string()).unwrap();
        assert_eq!(after_compound.state, TaskState::Review);
        assert_eq!(after_compound.requirements_seq, 1);
        let after_phased = api.task_get(&b.to_string()).unwrap();
        assert_eq!(after_phased.state, after_compound.state);
        assert_eq!(after_phased.requirements_seq, after_compound.requirements_seq);
        assert_eq!(adv.from, TaskState::Analysis);
        assert_eq!(adv.to, TaskState::Review);
    }

    /// REQ-SW-02/05: the labeled refusal keeps the operator text AND a
    /// machine token; the second writer never reaches the store write.
    #[test]
    fn single_writer_guard_refuses_with_parity_text_and_code() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let t = fresh_task(&api, "writer");
        let id = t.to_string();
        api.task_start(&id).unwrap();
        api.exec_start(ExecStartReq { id: id.clone(), role: "analyst".into(), agent: None, inputs: vec![] }).unwrap();

        let e = api.exec_start(ExecStartReq { id: id.clone(), role: "analyst".into(), agent: None, inputs: vec![] }).unwrap_err();
        assert_eq!(e.code(), ErrorCode("single_writer_busy"));
        let text = format!("{e}");
        assert!(text.starts_with(&format!("single-writer guard: task `{t}` already has a RUNNING execution")), "{text}");
        assert!(text.contains("`devai exec finish|fail|cancel "), "{text}");
        // Only one RUNNING row exists — the refusal wrote nothing.
        assert_eq!(api.exec_list(Some(&id)).unwrap().iter().filter(|x| x.status == ExecutionStatus::Running).count(), 1);
    }

    /// QA R5: the standalone advance of a NON-advancing kind refuses
    /// fail-closed with a machine code — no silent `from == to` no-op.
    /// The refusal enumerates the advancing kinds DERIVED from the core
    /// predicate (QA R4): the message can never list a stale set.
    #[test]
    fn advance_refuses_non_advancing_kind_fail_closed() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let t = fresh_task(&api, "advance-refuse");
        api.task_start(&t.to_string()).unwrap();
        let row = api
            .artifact_publish_row(PublishReq { id: t.to_string(), kind: ArtifactKind::TestReport, name: "tests".into(), content: b"ok".to_vec(), depends: vec![], contract: None, execution: None, category: DataCategory::Artifact, verdict: None })
            .unwrap();
        let e = api.artifact_advance(AdvanceReq { id: t.to_string(), artifact_id: row.artifact.id.to_string(), kind: ArtifactKind::TestReport, verdict: None }).unwrap_err();
        assert_eq!(e.code(), ErrorCode("artifact_kind_not_advancing"));
        let text = format!("{e}");
        assert!(text.contains("TEST_REPORT carries no workflow advance"), "{text}");
        assert!(text.contains("REQUIREMENTS_SPEC, REVIEW_REPORT, IMPLEMENTATION_REPORT"), "advancing set derived from the core predicate: {text}");
        // The refusal wrote nothing: the task is where the row phase left it.
        assert_eq!(api.task_get(&t.to_string()).unwrap().state, TaskState::Analysis);
    }

    /// QA R2: `quality_report_text` distinguishes «no report» (Ok(None))
    /// from «report unreadable» (Err with a machine code). A corrupt CURRENT
    /// blob must never be silently rendered as absence of a report.
    #[test]
    fn quality_report_text_distinguishes_absence_from_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let t = fresh_task(&api, "gate-text");
        assert_eq!(api.quality_report_text(t).unwrap(), None, "no report yet — honest absence");

        let row = api
            .artifact_publish_row(PublishReq { id: t.to_string(), kind: ArtifactKind::QualityReport, name: "qr".into(), content: br#"{"report_text":"all good"}"#.to_vec(), depends: vec![], contract: None, execution: None, category: DataCategory::Artifact, verdict: None })
            .unwrap();
        assert_eq!(api.quality_report_text(t).unwrap().as_deref(), Some("all good"));

        // Corrupt the stored blob under the CURRENT row: the next read must
        // fail loudly, not fall through to None.
        std::fs::write(api.store.blob_path(&row.artifact.sha256), b"not-json-at-all").unwrap();
        let e = api.quality_report_text(t).unwrap_err();
        assert_eq!(e.code(), ErrorCode("corrupt"));
        assert!(format!("{e}").contains("does not parse as JSON"), "{e}");
    }

    /// REQ-MUT-03/04 + human Q2: `Api::open` is the only bootstrap entry —
    /// the home is created through the API and seed lands through the API.
    #[test]
    fn bootstrap_is_encapsulated() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("fresh-home");
        let api = api(&home);
        assert!(home.join("devai.db").exists(), "opening through the API creates the store");
        let seeded = api.seed_config(None).unwrap();
        assert!(seeded > 0);
        assert!(!api.role_list().unwrap().is_empty(), "seeded roles are visible through the read plane");
    }
}
