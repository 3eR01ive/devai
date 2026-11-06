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
use devai_runtime::human_view::present::{self, PresentedReport, ReportFreshness};
use devai_runtime::{Engine, Gate, RunReport, liveness};
use devai_store::{ArtifactDraft, StoreError, bundle, guess_media_type, seed};
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
/// `idempotency_key` (REQ-API-02, ratified OQ-1): the caller-declared
/// identity of THIS CREATE COMMAND. A repeated call with a key whose
/// commit is already registered returns the original task and its edges
/// — the recovery path when a successful create's response was lost.
/// `None` keeps the plain every-call-creates semantics.
#[derive(Debug, Clone)]
pub struct TaskCreateReq {
    pub title: String,
    pub project_id: Option<Id>,
    pub description: String,
    pub parent: Vec<String>,
    pub blocked_by: Vec<String>,
    pub idempotency_key: Option<String>,
}

#[derive(Debug, Clone)]
pub struct TaskCreated {
    pub task: Task,
    pub linked: Vec<TaskDependency>,
}

/// `task.project.set` (REQ-BIND-02, C5 D1) — assign/replace the OWNING
/// project of an EXISTING task. `id` is the task ref (full id or unique
/// prefix, resolved by the store exactly like every other task read);
/// `project` is a project ref (full id or name, resolved by `project.get`
/// semantics) — an unknown ref on either side refuses with the typed
/// `not_found`, never silently. `expected_lock` is the OPTIONAL optimistic
/// base for machine consumers that loaded the task earlier: a stale version
/// refuses with the typed `conflict` BEFORE anything is written. The CLI
/// passes `None` and gets the store's own concurrency guard.
#[derive(Debug, Clone)]
pub struct TaskProjectSetReq {
    pub id: String,
    pub project: String,
    pub expected_lock: Option<u64>,
}

/// The outcome of `task.project.set`: the task AS STORED after the call
/// (lock bumped only when the binding actually changed) and `changed` —
/// false for an idempotent re-apply of the project already bound, which
/// writes NO row change and NO event (REQ-BIND-02 AC2).
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskProjectSetOutcome {
    pub task: Task,
    pub changed: bool,
}

/// The outcome of the REQ-BIND-03 backfill: the four disjoint task-id
/// groups the ratified rule (bind every NON-TERMINAL unbound task —
/// terminal history and other projects' tasks are left untouched) sorts
/// the store into. `bound` lists what the call changed; the rest document
/// what it deliberately did NOT.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct TaskProjectBackfillOutcome {
    pub bound: Vec<Id>,
    pub already_bound: Vec<Id>,
    pub skipped_terminal: Vec<Id>,
    pub skipped_other_project: Vec<Id>,
}

/// The workflow mutations the old CLI `op()` helper dispatched over a
/// closure table. The dispatch stays one code path
/// (`Api::task_op`); every named operation delegates to it
/// (REQ-MUT-05 AC3: dispatch and dedicated command execute the same path).
#[derive(Debug, Clone)]
pub enum WorkflowOp {
    Start,
    Review {
        verdict: Verdict,
        summary: Option<String>,
    },
    Approve {
        rationale: Option<String>,
    },
    Reject {
        rationale: Option<String>,
    },
    Accept {
        rationale: Option<String>,
    },
    Return {
        rationale: Option<String>,
    },
    Pause {
        reason: Option<String>,
    },
    Resume,
    Block {
        reason: Option<String>,
    },
    Cancel {
        reason: Option<String>,
    },
    Fail {
        reason: Option<String>,
    },
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
    /// The rendered quality report shown at the human gates, when present:
    /// the stored human form for quality-report@5 bodies, the legacy flat
    /// `report_text` for pre-@5 bodies (a damaged form renders an explicit
    /// marker, never silence).
    pub gate_report_text: Option<String>,
    /// REQ-QH-05: whether the report shown at the gate is still the last
    /// word — the marker names the task phase and the re-audit execution.
    pub gate_report_freshness: Option<ReportFreshness>,
    /// REQ-HP-05: the human paragraphs the gate presents FIRST, before any
    /// formal structure — `(artifact label, stored paragraph)` pairs read
    /// verbatim from the CURRENT artifacts' bodies (REQ-HP-07: the gate
    /// shows exactly the text that passed the detector, no read-time
    /// re-translation). Approval/review gates carry the spec + review
    /// paragraphs; acceptance gates carry the implementation-report
    /// paragraph (ratified OQ-HP-1: it is what the operator reads there).
    /// Additive to the machine surface — `--json` keeps rendering `report`.
    pub gate_human_paragraphs: Vec<(String, String)>,
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
        // R2 QA (input-validation): an empty/whitespace-only key must never
        // reach the registry — once stored, `''` would become a SHARED key
        // and the second empty-keyed call would silently return the first
        // task. The typed refusal happens BEFORE the first store touch
        // (including the registry lookup below): it can neither read nor
        // write, so a refusal leaves the store byte-identical.
        if let Some(key) = req.idempotency_key.as_deref() {
            if key.trim().is_empty() {
                return Err(ApiError::InvalidInput {
                    field: "idempotency_key",
                    message: "must not be empty or whitespace-only".into(),
                });
            }
        }
        // REQ-API-02 (ratified OQ-1): a retry carrying the key of an
        // ALREADY COMMITTED create returns the registered task and its
        // stored ACTIVE edges — the lost-response retry never duplicates
        // the effect. The registry row lands inside the creation
        // transaction, so finding it is proof the first create committed.
        if let Some(key) = req.idempotency_key.as_deref() {
            if let Some(existing) = self.store.task_by_creation_key(key)? {
                return Ok(TaskCreated {
                    linked: self.store.task_dependencies(existing.id)?,
                    task: existing,
                });
            }
        }
        // REQ-DEP-07: a derived task records WHY it exists at the moment it
        // is created; dangling references fail loudly on resolve, exactly as
        // they would via `task depend`.
        // REQ-API-01 (the QA-01a1acab-18b1 fix): EVERY ref is resolved
        // BEFORE anything is written. The pre-fix order inserted the task,
        // then resolved parent and blocked_by in loops — an Ambiguous or
        // NotFound halfway left an ORPHAN task (and possibly a partial
        // parent edge set) in the store, and the retry doubled the effect.
        // Resolution order stays parents-then-blockers, so the first
        // failing ref produces the identical error text of the old path.
        let mut parents: Vec<Task> = Vec::new();
        for p in &req.parent {
            parents.push(self.store.resolve_task(p)?);
        }
        let mut blockers: Vec<Task> = Vec::new();
        for b in &req.blocked_by {
            blockers.push(self.store.resolve_task(b)?);
        }
        let mut deps: Vec<(&Task, DependencyKind)> = Vec::new();
        for t in &parents {
            deps.push((t, DependencyKind::Parent));
        }
        for t in &blockers {
            deps.push((t, DependencyKind::Blocks));
        }
        // The task, its edges and the key registration land in ONE store
        // transaction (REQ-API-01: all effects of this call, or none), so
        // `TaskCreated` can never describe an edge absent from the store.
        let t = Task::new(req.title, req.description, req.project_id, Utc::now());
        let linked = self.store.create_task_with_dependencies(
            &t,
            &deps,
            &self.actor,
            Utc::now(),
            req.idempotency_key.as_deref(),
        )?;
        Ok(TaskCreated { task: t, linked })
    }

    // ---- task: project binding --------------------------------------------

    /// REQ-BIND-02 (C5 D1): assign/replace the owning project of an
    /// existing task. Reads resolve FIRST (a bad task ref or a bad project
    /// ref refuses typed `not_found` before any write — the atomicity
    /// discipline of `task_create`, REQ-API-01); a stale `expected_lock`
    /// refuses typed `conflict` before any write; re-applying the project
    /// the task already carries is a NO-OP (same row, no lock bump, no
    /// event); an effective change goes through the store's one-transaction
    /// writer (row + append-only `task.project.set` event together).
    pub fn task_project_set(&self, req: TaskProjectSetReq) -> ApiResult<TaskProjectSetOutcome> {
        let task = self.store.resolve_task(&req.id)?;
        let project = self
            .store
            .get_project(&req.project)?
            .ok_or_else(|| StoreError::NotFound("project".into(), req.project.clone()))?;
        if req.expected_lock.is_some_and(|l| l != task.lock_version) {
            return Err(StoreError::Conflict {
                what: format!("task {}", task.id),
                expected: req.expected_lock.unwrap(),
                found: task.lock_version,
            }
            .into());
        }
        if task.project_id == Some(project.id) {
            return Ok(TaskProjectSetOutcome {
                task,
                changed: false,
            });
        }
        let updated = self
            .store
            .set_task_project(&task, project.id, &self.actor, Utc::now())?;
        Ok(TaskProjectSetOutcome {
            task: updated,
            changed: true,
        })
    }

    /// REQ-BIND-03 (C5 D1, backfill under the ratified OQ-BIND-2 rule):
    /// bind `project` to EVERY task that lives unbound and non-terminal —
    /// terminal (ACCEPTED/CANCELLED) history stays where it lived and
    /// judged, tasks owned by ANOTHER project are never re-bound. Each
    /// binding goes through [`Api::task_project_set`] — one registered
    /// operation, one event, one lock bump per task; repeated application
    /// finds no candidates and changes nothing (idempotent). Returns the
    /// task ids bound NOW, the ids already carrying this project, the
    /// terminal ids skipped and the ids bound to another project.
    pub fn task_project_backfill(&self, project: &str) -> ApiResult<TaskProjectBackfillOutcome> {
        let p = self
            .store
            .get_project(project)?
            .ok_or_else(|| StoreError::NotFound("project".into(), project.to_string()))?;
        let mut outcome = TaskProjectBackfillOutcome::default();
        for t in self.store.list_tasks()? {
            if t.project_id == Some(p.id) {
                outcome.already_bound.push(t.id);
                continue;
            }
            if t.state.is_terminal() {
                outcome.skipped_terminal.push(t.id);
                continue;
            }
            if t.project_id.is_some() {
                outcome.skipped_other_project.push(t.id);
                continue;
            }
            let set = self.task_project_set(TaskProjectSetReq {
                id: t.id.to_string(),
                project: p.id.to_string(),
                expected_lock: None,
            })?;
            outcome.bound.push(set.task.id);
        }
        Ok(outcome)
    }

    // ---- task: workflow dispatch (the former CLI `op()` helper) -----------

    /// Load task + artifacts, run one core workflow command, commit with
    /// optimistic locking — the single serialization point the old CLI op()
    /// was. Every named workflow operation delegates here (REQ-MUT-05 AC3).
    pub fn task_op(&self, id: &str, op: WorkflowOp) -> ApiResult<MutationResult> {
        match op {
            WorkflowOp::Start => {
                self.run_op(id, |t, _a| workflow::start(t, &self.actor, Utc::now()))
            }
            WorkflowOp::Review { verdict, summary } => self.run_op(id, move |t, _a| {
                workflow::review_recorded(t, verdict, summary.as_deref(), &self.actor, Utc::now())
            }),
            WorkflowOp::Approve { rationale } => self.run_op(id, move |t, a| {
                // The approve gate reads the CURRENT spec exactly as the old
                // closure did — same guard, same refusal text.
                if a.iter().any(|x| {
                    x.kind == ArtifactKind::RequirementsSpec && x.status == ArtifactStatus::Current
                }) {
                    workflow::approve_requirements(t, rationale.as_deref(), &self.actor, Utc::now())
                } else {
                    Err(devai_core::CoreError::guard(
                        "no CURRENT REQUIREMENTS_SPEC artifact to approve",
                    ))
                }
            }),
            WorkflowOp::Reject { rationale } => self.run_op(id, move |t, _a| {
                workflow::reject_requirements(t, rationale.as_deref(), &self.actor, Utc::now())
            }),
            WorkflowOp::Accept { rationale } => {
                // REQ-DEP-03: the acceptance gate consults the stored
                // dependency graph and the override decisions before the
                // core command runs — the loads keep the old position.
                let task = self.store.resolve_task(id)?;
                let deps = self.store.task_dependencies(task.id)?;
                let decisions = self.store.list_decisions(task.id)?;
                self.run_op(id, move |t, a| {
                    workflow::accept(
                        t,
                        a,
                        &deps,
                        &decisions,
                        rationale.as_deref(),
                        &self.actor,
                        Utc::now(),
                    )
                })
            }
            WorkflowOp::Return { rationale } => self.run_op(id, move |t, a| {
                workflow::return_to_rework(t, a, rationale.as_deref(), &self.actor, Utc::now())
            }),
            WorkflowOp::Pause { reason } => self.run_op(id, move |t, _a| {
                workflow::pause(t, reason.as_deref(), &self.actor, Utc::now())
            }),
            WorkflowOp::Resume => {
                let task = self.store.resolve_task(id)?;
                let deps = self.store.task_dependencies(task.id)?;
                let decisions = self.store.list_decisions(task.id)?;
                self.run_op(id, move |t, _a| {
                    workflow::resume(t, &deps, &decisions, &self.actor, Utc::now())
                })
            }
            WorkflowOp::Block { reason } => self.run_op(id, move |t, _a| {
                workflow::block(t, reason.as_deref(), &self.actor, Utc::now())
            }),
            WorkflowOp::Cancel { reason } => self.run_op(id, move |t, _a| {
                workflow::cancel(t, reason.as_deref(), &self.actor, Utc::now())
            }),
            WorkflowOp::Fail { reason } => self.run_op(id, move |t, _a| {
                workflow::fail(t, reason.as_deref(), &self.actor, Utc::now())
            }),
        }
    }

    fn run_op<F>(&self, id: &str, f: F) -> ApiResult<MutationResult>
    where
        F: FnOnce(&Task, &[Artifact]) -> devai_core::CoreResult<Mutation>,
    {
        let task = self.store.resolve_task(id)?;
        let artifacts = self.store.artifacts(task.id)?;
        let m = f(&task, &artifacts)?;
        let staled = m
            .artifact_status
            .iter()
            .filter(|(_, s)| *s == ArtifactStatus::Stale)
            .count();
        let superseded = m
            .artifact_status
            .iter()
            .filter(|(_, s)| *s == ArtifactStatus::Superseded)
            .count();
        self.store.commit(task.lock_version, &m)?;
        Ok(MutationResult {
            task: m.task.clone(),
            previous_state: task.state,
            event_kinds: m.events.iter().map(|e| e.kind.to_string()).collect(),
            staled,
            superseded,
            allowed_operations: workflow::allowed_ops(&m.task)
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
        })
    }

    pub fn task_start(&self, id: &str) -> ApiResult<MutationResult> {
        self.task_op(id, WorkflowOp::Start)
    }
    pub fn task_review(
        &self,
        id: &str,
        verdict: Verdict,
        summary: Option<String>,
    ) -> ApiResult<MutationResult> {
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
            .find(|x| {
                x.kind == ArtifactKind::RequirementsSpec && x.status == ArtifactStatus::Current
            })
            .map(|x| x.id.to_string())
            .unwrap_or_default();
        let mutation = self.task_op(
            id,
            WorkflowOp::Approve {
                rationale: rationale.clone(),
            },
        )?;
        let decision = Decision {
            id: new_id(),
            task_id: task.id,
            kind: decision_kind::REQUIREMENTS_APPROVAL.into(),
            verdict: Verdict::Approved,
            actor: self.actor.clone(),
            rationale,
            refs: if spec_ref.is_empty() {
                Vec::new()
            } else {
                vec![spec_ref]
            },
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
        // The gate renderings read the presented quality report — the same
        // status-aware selection (REQ-005, REQ-QH-05/06) the UI panel uses:
        // the stored human form when present, the §2.6 text otherwise, plus
        // the freshness marker when the report is no longer the last word.
        let presented = match report.gate {
            Gate::WaitingAccept | Gate::ReportFresh => {
                self.presented_quality_report(report.task_id)?
            }
            _ => None,
        };
        let gate_report_text = presented.as_ref().map(|p| p.text.clone());
        let gate_report_freshness = presented.map(|p| p.freshness);
        let current_quality_report = match report.gate {
            Gate::ReportFresh => self.store.artifacts(report.task_id)?.into_iter().find(|x| {
                x.kind == ArtifactKind::QualityReport && x.status == ArtifactStatus::Current
            }),
            _ => None,
        };
        // REQ-HP-05 (ratified OQ-HP-1): the paragraph half of the gate
        // presentation — approval/review reads spec + review, acceptance
        // reads the implementation-report. Stored fields, verbatim; absence
        // (a legacy @1/@2 CURRENT, a non-JSON body) renders as NO paragraph,
        // never as a broken gate (REQ-HP-06 non-retroactivity in the render
        // half too).
        let mut gate_human_paragraphs: Vec<(String, String)> = Vec::new();
        match report.gate {
            Gate::WaitingApproval => {
                for (kind, label) in [
                    (ArtifactKind::RequirementsSpec, "requirements-specification"),
                    (ArtifactKind::ReviewReport, "review-report"),
                ] {
                    if let Some(p) = self.stored_human_paragraph(report.task_id, kind, label)? {
                        gate_human_paragraphs.push(p);
                    }
                }
            }
            Gate::WaitingAccept | Gate::ReportFresh => {
                if let Some(p) = self.stored_human_paragraph(
                    report.task_id,
                    ArtifactKind::ImplementationReport,
                    "implementation-report",
                )? {
                    gate_human_paragraphs.push(p);
                }
            }
            _ => {}
        }
        let t = self.store.get_task(&report.task_id.to_string())?;
        Ok(RunOutcome {
            report,
            current_quality_report,
            gate_report_text,
            gate_report_freshness,
            gate_human_paragraphs,
            allowed_operations: workflow::allowed_ops(&t)
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
        })
    }

    pub fn task_revise(&self, req: ReviseReq) -> ApiResult<ReviseOutcome> {
        let task = self.store.resolve_task(&req.id)?;
        let _a = self.publish_row_internal(
            task.id,
            ArtifactKind::RequirementsSpec,
            &req.name,
            req.content,
            None,
            None,
            vec![],
            None,
            DataCategory::Artifact,
        )?;
        let after = self.store.artifacts(task.id)?;
        let m = workflow::requirements_published(
            &task,
            &after,
            req.reason.as_deref(),
            &self.actor,
            Utc::now(),
        )?;
        self.store.commit(task.lock_version, &m)?;
        Ok(ReviseOutcome {
            task: m.task.clone(),
            staled_ids: m
                .artifact_status
                .iter()
                .filter(|(_, s)| *s == ArtifactStatus::Stale)
                .map(|(i, _)| i.to_string())
                .collect(),
            allowed_operations: workflow::allowed_ops(&m.task)
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
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
            if let Some(spec) = self
                .store
                .artifacts(task.id)?
                .iter()
                .find(|a| {
                    a.kind == ArtifactKind::RequirementsSpec && a.status == ArtifactStatus::Current
                })
                .map(|a| a.id)
            {
                depends_ids.push(spec);
            }
        }
        let execution_id = match req.execution.as_deref() {
            Some(e) => {
                // Ratified OQ-4: publication into a closed row is a LOUD
                // refusal — «зомби не пишет в закрытую строку».
                let ex = self.store.get_execution(e)?;
                if ex.status != ExecutionStatus::Running {
                    return Err(ApiError::refused(
                        "execution_not_running",
                        format!(
                            "execution {} is {}, not RUNNING — artifact publication requires the producing execution to be RUNNING (a closed row stays closed: work that continues starts a new execution)",
                            ex.id,
                            ex.status.as_str()
                        ),
                    ));
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
        let artifact = self.publish_row_internal(
            task.id,
            req.kind,
            &req.name,
            req.content,
            contract,
            execution_id,
            depends_ids,
            created_by.as_deref(),
            req.category,
        )?;
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
                let m =
                    workflow::requirements_published(&task, &arts, None, &self.actor, Utc::now())?;
                self.store.commit(task.lock_version, &m)?;
                Ok(AdvanceOutcome {
                    from: task.state,
                    to: m.task.state,
                    rework_gate: false,
                })
            }
            ArtifactKind::ReviewReport => {
                let v = match req.verdict.as_deref() {
                    Some(s) => Self::parse_verdict(s)?,
                    None => {
                        return Err(ApiError::refused(
                            "bad_request",
                            "publishing REVIEW_REPORT requires --verdict approved|needs_revision",
                        ));
                    }
                };
                let m = workflow::review_recorded(&task, v, None, &self.actor, Utc::now())?;
                self.store.commit(task.lock_version, &m)?;
                Ok(AdvanceOutcome {
                    from: task.state,
                    to: m.task.state,
                    rework_gate: false,
                })
            }
            ArtifactKind::ImplementationReport => {
                // The manual publication crosses the SAME core command as
                // the engine — including the empty-rework gate.
                let arts = self.store.artifacts(task.id)?;
                let facts = devai_runtime::gate::collect_implementation_report_facts(
                    &self.store,
                    task.id,
                    &a,
                    &arts,
                    None,
                )?;
                let m = workflow::implementation_reported(&task, &facts, &self.actor, Utc::now())?;
                self.store.commit(task.lock_version, &m)?;
                Ok(AdvanceOutcome {
                    from: task.state,
                    to: m.task.state,
                    rework_gate: m.task.state == TaskState::WaitingForInput,
                })
            }
            // Fail-closed (QA R5): the kind carries no advance. The refusal
            // lists the advancing kinds DERIVED from the core predicate —
            // never a hand-copied set that could drift (QA R4).
            other => Err(ApiError::refused(
                "artifact_kind_not_advancing",
                format!(
                    "{} carries no workflow advance — the advancing kinds are {} (the row alone lands via `artifact.publish_row`; publish through `artifact.publish` to get the row and its advance in one call)",
                    other.as_str(),
                    ArtifactKind::ALL
                        .iter()
                        .filter(|k| k.advances_state())
                        .map(|k| k.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ),
            )),
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
            Some(self.artifact_advance(AdvanceReq {
                id: publish.task.id.to_string(),
                artifact_id: publish.artifact.id.to_string(),
                kind,
                verdict,
            })?)
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
        Verdict::from_token(s).ok_or_else(|| {
            ApiError::refused(
                "unknown_variant",
                format!(
                    "unknown verdict `{}` ({})",
                    s.to_ascii_lowercase(),
                    Verdict::token_hint()
                ),
            )
        })
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
                return Err(ApiError::ContractUnregistered {
                    contract: c.to_string(),
                });
            };
            // Gate parity (ratified OQ-FG-05, REQ-FG-08): the manual path
            // runs the SAME contract gate as the engine loop; enforcement
            // is the API's, the presentation (warning + full list) the
            // consumer's. REQ-HP-02 closes the last door: the paragraph
            // halves (presence via `required`, edibility via the
            // deterministic detector) live in this one call, and it reads
            // the project's loaded `humanize.yaml` fresh — config stays
            // the source of truth, a broken template is a LOUD refusal,
            // never a silent factory fallback on the manual path.
            let humanize = self.humanize_policy()?;
            let violations = devai_runtime::artifact_violations(
                String::from_utf8_lossy(&content).as_ref(),
                Some(schema.as_str()),
                &humanize,
            )?;
            if !violations.is_empty() {
                return Err(ApiError::ContractRejected {
                    contract: c.to_string(),
                    violations,
                });
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
        Ok(self.store.link_task_dependency(
            task.id,
            depends_on.id,
            req.kind,
            &self.actor,
            Utc::now(),
        )?)
    }

    pub fn task_unlink(&self, req: DependReq) -> ApiResult<UnlinkResult> {
        let task = self.store.resolve_task(&req.id)?;
        let depends_on = self.store.resolve_task(&req.on)?;
        // RATIFIED DECISION OQ7: edges are IMMUTABLE — removal is a
        // SUPERSEDED marker; the row stays as history forever.
        let edge = self.store.supersede_task_dependency(
            task.id,
            depends_on.id,
            req.kind,
            &self.actor,
            Utc::now(),
        )?;
        Ok(UnlinkResult {
            edge,
            superseded_by: self.actor.clone(),
        })
    }

    pub fn task_override(
        &self,
        id: &str,
        blocker: Vec<String>,
        rationale: Option<String>,
    ) -> ApiResult<OverrideResult> {
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
                return Err(ApiError::refused(
                    "bad_request",
                    format!(
                        "task {} has no open blocking dependencies — nothing to override",
                        short_id(&task.id)
                    ),
                ));
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
        self.store.insert_event(
            Some(task.id),
            event_kind::TASK_DEPENDENCY_OVERRIDDEN,
            &self.actor,
            serde_json::json!({ "blockers": blockers }),
        )?;
        Ok(OverrideResult {
            task,
            override_blockers: blockers,
        })
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
        Ok(MergeResult {
            task_id: task.id,
            branch: req.branch,
            workspace: req.workspace,
            head_before: req.head_before,
            head_after: req.head_after,
        })
    }

    /// The EMPTY-REWORK GATE escape (REQ-RG-03), human-only like the merge
    /// gate: records (or reuses) the pair-bound `rework_noop_ratified`
    /// decision, releases the gate's blocking question, and runs the
    /// withheld REWORK → QA transition through the same core command —
    /// which now finds the ratification and cites its decision id (AC3).
    /// R4 QA (cl/short-units): the phases live in the private units below;
    /// the act SEQUENCE and every refusal text are unchanged (this is
    /// layout, not domain — statements moved verbatim).
    pub fn task_ratify_empty_rework(
        &self,
        id: &str,
        rationale: Option<String>,
    ) -> ApiResult<RatifyResult> {
        let task = self.store.resolve_task(id)?;
        Self::ratify_gate_state(&task)?;
        let (current, trip, prev_report) = self.ratify_current_trip(&task)?;
        let current_id_s = current.id.to_string();
        let (decision, recorded_now) =
            self.ratify_decision_row(task.id, &prev_report, &current_id_s, rationale)?;
        let cur = self.ratify_release_question(&task, &trip, &decision)?;
        let m = self.ratify_withheld_passage(&cur, &current, &decision)?;
        self.store.commit(cur.lock_version, &m)?;
        Ok(RatifyResult {
            task: cur,
            decision,
            recorded_now,
            prev_report,
            current_report: current_id_s,
            state: m.task.state,
            allowed_operations: workflow::allowed_ops(&m.task)
                .into_iter()
                .map(|s| s.to_string())
                .collect(),
        })
    }

    /// One refusal in the gate's error family (code `empty_rework_gate`).
    fn empty_rework_refused(message: impl Into<String>) -> ApiError {
        ApiError::refused("empty_rework_gate", message)
    }

    /// Phase 1 — the state guard: only the gate hold (WAITING_FOR_INPUT
    /// returning to REWORK) or the released REWORK itself may ratify.
    fn ratify_gate_state(task: &Task) -> Result<(), ApiError> {
        match task.state {
            // The gate hold is WAITING_FOR_INPUT returning to REWORK. If
            // the operator answered the question first (back to REWORK, no
            // ratification) — or a previous run died between the two acts
            // — ratifying from REWORK converges here too.
            TaskState::WaitingForInput if task.return_state == Some(TaskState::Rework) => {}
            TaskState::Rework => {}
            s => {
                return Err(Self::empty_rework_refused(format!(
                    "ratify-empty-rework applies to the empty-rework gate (REWORK, or WAITING_FOR_INPUT returning to REWORK); the task is in {}",
                    s.as_str()
                )));
            }
        }
        Ok(())
    }

    /// Phase 2 — the CURRENT report and the gate trip that binds it: the
    /// newest implementation report must have a recorded
    /// `qa.rework_gate_tripped` event, and that trip must name a previous
    /// report (only the trip of the LATEST report is ratifiable, REQ-RG-03
    /// AC4). Returns (current report, trip payload, previous report id).
    fn ratify_current_trip(&self, task: &Task) -> ApiResult<(Artifact, serde_json::Value, String)> {
        let arts = self.store.artifacts(task.id)?;
        let Some(current) = arts
            .iter()
            .filter(|a| a.kind == ArtifactKind::ImplementationReport)
            .max_by_key(|a| a.seq)
            .cloned()
        else {
            return Err(Self::empty_rework_refused(
                "the task has no implementation report — nothing to ratify",
            ));
        };
        let events = self.store.all_events_for_task(task.id)?;
        let current_id_s = current.id.to_string();
        let trip = events
            .iter()
            .rev()
            .find(|e| e.kind == event_kind::QA_REWORK_GATE_TRIPPED && e.payload.get(json_field::CURRENT_REPORT).and_then(|v| v.as_str()) == Some(current_id_s.as_str()))
            .map(|e| e.payload.clone())
            .ok_or_else(|| Self::empty_rework_refused(format!(
                "no empty-rework gate recorded for report {} — the gate fires when a REWORK publication names the unchanged commit; if the latest report is newer, let the engine publish it from REWORK first",
                short_id(&current.id)
            )))?;
        let prev_report = trip
            .get(json_field::PREV_REPORT)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                ApiError::refused(
                    "corrupt",
                    "the gate trip event names no previous report — history corruption",
                )
            })?
            .to_string();
        Ok((current, trip, prev_report))
    }

    /// Phase 3 — the ratification for exactly this pair, recorded once; a
    /// retry reuses the stored row (never a double-written judgment).
    fn ratify_decision_row(
        &self,
        task_id: Id,
        prev_report: &str,
        current_id: &str,
        rationale: Option<String>,
    ) -> ApiResult<(Decision, bool)> {
        let decisions = self.store.list_decisions(task_id)?;
        let existing = decisions
            .iter()
            .find(|d| {
                d.kind == decision_kind::REWORK_NOOP_RATIFIED
                    && d.refs.iter().any(|r| r == prev_report)
                    && d.refs.iter().any(|r| r == current_id)
            })
            .cloned();
        match existing {
            Some(d) => Ok((d, false)),
            None => {
                let d = Decision {
                    id: new_id(),
                    task_id,
                    kind: decision_kind::REWORK_NOOP_RATIFIED.into(),
                    verdict: Verdict::Approved,
                    actor: self.actor.clone(),
                    rationale,
                    refs: vec![prev_report.to_string(), current_id.to_string()],
                    created_at: Utc::now(),
                };
                self.store.record_decision(&d)?;
                Ok((d, true))
            }
        }
    }

    /// Phase 4 — release the blocking gate question while the hold is still
    /// up. Returns the task as it stands after the release, or unchanged
    /// when there was no hold to release (the operator answered first).
    fn ratify_release_question(
        &self,
        task: &Task,
        trip: &serde_json::Value,
        decision: &Decision,
    ) -> ApiResult<Task> {
        let mut cur = task.clone();
        if cur.state == TaskState::WaitingForInput {
            let q_ref = trip
                .get(json_field::QUESTION_ID)
                .and_then(|v| v.as_str())
                .unwrap_or_default();
            let q = self.store.get_question(q_ref)?;
            if q.status == QuestionStatus::Open {
                self.store.answer_question(
                    q.id,
                    &format!(
                        "operator ratified the empty rework (decision {})",
                        decision.id
                    ),
                    &self.actor,
                    Utc::now(),
                )?;
            }
            let m = workflow::answer_question(&cur, q.id, &self.actor, Utc::now())?;
            self.store.commit(cur.lock_version, &m)?;
            cur = self.store.get_task(&cur.id.to_string())?;
        }
        Ok(cur)
    }

    /// Phase 5 — run the withheld transition through the single
    /// serialization point; with the stored ratification in the facts, the
    /// core command admits REWORK → QA and puts `ratified_by` on the event.
    fn ratify_withheld_passage(
        &self,
        cur: &Task,
        current: &Artifact,
        decision: &Decision,
    ) -> ApiResult<Mutation> {
        let arts = self.store.artifacts(cur.id)?;
        let facts = devai_runtime::gate::collect_implementation_report_facts(
            &self.store,
            cur.id,
            current,
            &arts,
            Some(format!("empty rework ratified by decision {}", decision.id)),
        )?;
        let m = workflow::implementation_reported(cur, &facts, &self.actor, Utc::now())?;
        if m.task.state != TaskState::Qa {
            return Err(Self::empty_rework_refused(format!(
                "the withheld transition did not land in QA (state {}); decision {} stands — inspect `devai task events {}`",
                m.task.state.as_str(),
                decision.id,
                short_id(&cur.id)
            )));
        }
        Ok(m)
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
        Ok(QuestionAsked {
            question: q,
            parked: req.blocking && m.task.state != task.state,
        })
    }

    pub fn question_answer(&self, question_id: &str, text: &str) -> ApiResult<AnsweredOutcome> {
        let q = self.store.get_question(question_id)?;
        if q.status == QuestionStatus::Answered {
            return Err(ApiError::refused(
                "bad_request",
                format!("question {} is already answered", q.id),
            ));
        }
        let task = self.store.get_task(&q.task_id.to_string())?;
        self.store
            .answer_question(q.id, text, &self.actor, Utc::now())?;
        let m = workflow::answer_question(&task, q.id, &self.actor, Utc::now())?;
        self.store.commit(task.lock_version, &m)?;
        Ok(AnsweredOutcome {
            question_id: q.id,
            from_state: task.state,
            to_state: m.task.state,
        })
    }

    // ---- roles / agents / contracts ----------------------------------------

    /// `role.add`: version 0 in the definition means latest+1 for the slug,
    /// a nil id gets a fresh one, `created_at` is stamped here (exactly the
    /// old CLI defaulting, now behind the API).
    pub fn role_add(&self, mut r: RoleDefinition) -> ApiResult<RoleDefinition> {
        if r.version == 0 {
            r.version = self
                .store
                .get_role(&r.slug)?
                .map(|e| e.version + 1)
                .unwrap_or(1);
        }
        if r.id == Uuid::nil() {
            r.id = new_id();
        }
        r.created_at = Utc::now();
        self.store.put_role(&r)?;
        Ok(r)
    }

    pub fn agent_add(&self, req: AgentAddReq) -> ApiResult<AgentInstance> {
        let a = AgentInstance {
            id: new_id(),
            name: req.name.clone(),
            provider: req.provider,
            model: req.model,
            runtime: req.runtime,
            created_at: Utc::now(),
        };
        self.store.put_agent(&a)?;
        Ok(a)
    }

    pub fn contract_add(&self, req: ContractAddReq) -> ApiResult<ContractRegistration> {
        self.store
            .put_contract(&req.name, req.version, &req.schema)?;
        Ok(ContractRegistration {
            name: req.name,
            version: req.version,
        })
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
        self.exec_close(
            exec_id,
            ExecutionStatus::Completed,
            summary.as_deref(),
            None,
        )
    }

    pub fn exec_fail(&self, exec_id: &str, error: &str) -> ApiResult<RoleExecution> {
        self.exec_close(exec_id, ExecutionStatus::Failed, None, Some(error))
    }

    pub fn exec_cancel(&self, exec_id: &str) -> ApiResult<RoleExecution> {
        self.exec_close(exec_id, ExecutionStatus::Cancelled, None, None)
    }

    fn exec_close(
        &self,
        exec_id: &str,
        status: ExecutionStatus,
        summary: Option<&str>,
        error: Option<&str>,
    ) -> ApiResult<RoleExecution> {
        let e = self.store.get_execution(exec_id)?;
        if e.status != ExecutionStatus::Running {
            return Err(ApiError::refused(
                "execution_not_running",
                format!("execution {} is {}, not RUNNING", e.id, e.status.as_str()),
            ));
        }
        self.store
            .finish_execution(&e, status, summary, error, Utc::now())?;
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
        let blocker = self
            .store
            .list_executions(Some(task.id))?
            .into_iter()
            .find(|e| e.status == ExecutionStatus::Running);
        let Some(e) = blocker else { return Ok(()) };
        let owner = match (e.owner_pid, e.owner_host.as_deref()) {
            (Some(pid), Some(host)) => format!("pid {pid}@{host}"),
            (Some(pid), None) => format!("pid {pid} (host unrecorded)"),
            (None, _) => "owner unrecorded (row predates the v7 guard)".to_string(),
        };
        let verdict = match liveness::check_owner(
            e.owner_pid,
            e.owner_host.as_deref(),
            e.owner_process_start,
        ) {
            liveness::Liveness::Alive => format!(
                "a LIVE writer is in flight ({owner}) — wait for it to finish, or stop it explicitly"
            ),
            liveness::Liveness::Dead => format!(
                "an ORPHAN ({owner} is no longer running) — nobody is collecting this writer's work"
            ),
            liveness::Liveness::Unknown => {
                format!("a writer whose owner cannot be verified ({owner}) — never assumed dead")
            }
        };
        Err(ApiError::refused(
            "single_writer_busy",
            format!(
                "single-writer guard: task `{}` already has a RUNNING execution `{}` — {} ({}). The second writer is refused by design: no queueing, no overwrite. Resolve the open row first (a human decision, the process is not touched): `devai exec finish|fail|cancel {}`",
                task.id,
                e.id,
                verdict,
                short_id(&e.id),
                e.id
            ),
        ))
    }

    /// The project's humanization template, resolved the same way the engine
    /// resolves `DEVAI_CONFIG`/home config (REQ-HP-04): loaded per call, so
    /// a value edit lands on the very next publication without a rebuild,
    /// and a broken file refuses LOUD — the manual publish path never
    /// silently falls back to the factory while the YAML claims otherwise.
    fn humanize_policy(&self) -> ApiResult<devai_store::config::HumanizePolicy> {
        let cfg_dir = std::env::var_os("DEVAI_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.store.root().join("config"));
        Ok(devai_store::config::load_humanize_policy(
            &cfg_dir.join("workflow"),
        )?)
    }

    /// REQ-HP-05/07: the stored human paragraph of the CURRENT artifact of
    /// `kind`, verbatim — the gate renders exactly the bytes the detector
    /// accepted, no read-time retranslation. Honest `Ok(None)` cases: no
    /// CURRENT row of the kind, a legacy body under @1/@2 without the
    /// field, a non-JSON body (manual free-form artifacts stay publishable
    /// under no contract — they simply render no paragraph). A store
    /// failure propagates; the gate never fabricates prose.
    fn stored_human_paragraph(
        &self,
        task_id: Id,
        kind: ArtifactKind,
        label: &str,
    ) -> ApiResult<Option<(String, String)>> {
        let arts = self.store.artifacts(task_id)?;
        let Some(a) = arts
            .iter()
            .find(|a| a.kind == kind && a.status == ArtifactStatus::Current)
        else {
            return Ok(None);
        };
        let blob = self.store.read_blob(&a.sha256)?;
        let Ok(v) = serde_json::from_slice::<serde_json::Value>(&blob) else {
            return Ok(None);
        };
        let Some(paragraph) = v.get("human_paragraph").and_then(|t| t.as_str()) else {
            return Ok(None);
        };
        Ok(Some((format!("{label} v{}", a.seq), paragraph.to_string())))
    }

    /// The rendered quality report shown at the human gates, when one is
    /// present (the gate's viewing channel). `Ok(None)` is the honest «no
    /// quality report»; a store failure or a body that is not JSON at all
    /// propagates as `Err` with its machine code — never a silent «absence».
    ///
    /// REQ-QH-05/06: the rendering (stored human form, legacy flat text, or
    /// the damaged-form marker) and the freshness fact both come from
    /// `human_view::present` — raw rows in, stored fields out. When the shown
    /// report is no longer the last word (rework in flight, or the selected
    /// row is not CURRENT), the freshness marker is prepended so an outdated
    /// report never reads as the decision.
    pub fn quality_report_text(&self, task_id: Id) -> ApiResult<Option<String>> {
        Ok(self
            .presented_quality_report(task_id)?
            .map(|p| p.text_with_freshness()))
    }

    /// The presented report plus its freshness verdict — the same single
    /// read the CLI gates and the UI panel consume. Thin on purpose: the
    /// row selection, the rendering and the freshness marker live in
    /// `devai_runtime::human_view::present`.
    pub fn presented_quality_report(&self, task_id: Id) -> ApiResult<Option<PresentedReport>> {
        let task = self.store.get_task(&task_id.to_string())?;
        Ok(self.presented_quality(&task)?.map(|p| p.presented))
    }

    /// REQ-QH-06 / rework R2/R4: the ONE presentation pipeline. Both public
    /// readers (`presented_quality_report` for the gates, `task_report` for
    /// the UI) call this, so row selection, blob read, rendering, the
    /// distinguishable render source and the freshness fact can never drift
    /// between two hand-copied paths.
    ///
    /// `Ok(None)` is the honest «no quality report»; a blob that does not
    /// parse as JSON is a TYPED outcome of its own (`RenderSource::CorruptBody`
    /// with a marker naming the damaged artifact), never an empty success and
    /// never accidentally equal to «QA has not run».
    pub(crate) fn presented_quality(&self, task: &Task) -> ApiResult<Option<PresentedQuality>> {
        let arts = self.store.artifacts(task.id)?;
        let Some(a) = present::select_report(&arts) else {
            return Ok(None);
        };
        let blob = self.store.read_blob(&a.sha256)?;
        let execs = self.store.list_executions(Some(task.id))?;
        let freshness = present::freshness(task, a, &execs);
        let (body, text, source) = match serde_json::from_slice::<serde_json::Value>(&blob) {
            Ok(body) => {
                let rendered = present::render_stored_report(&body);
                (body, rendered.text, rendered.source)
            }
            Err(e) => (
                serde_json::Value::Null,
                present::corrupt_body_text(&short_id(&a.id), &e.to_string()),
                devai_runtime::human_view::present::RenderSource::CorruptBody,
            ),
        };
        Ok(Some(PresentedQuality {
            artifact: a.clone(),
            body,
            presented: PresentedReport {
                text,
                source,
                freshness,
            },
        }))
    }
}

/// Everything the two presentation readers need from ONE store read: the
/// artifact row that is shown, the parsed machine body, and the presented
/// text plus its freshness verdict.
pub(crate) struct PresentedQuality {
    pub artifact: Artifact,
    pub body: serde_json::Value,
    pub presented: PresentedReport,
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
        api.task_create(TaskCreateReq {
            title: title.into(),
            project_id: None,
            description: String::new(),
            parent: vec![],
            blocked_by: vec![],
            idempotency_key: None,
        })
        .expect("create")
        .task
        .id
    }

    // --- REQ-API-01 / REQ-API-02 (QA artifact 01a1acab-18b1, ratified
    // OQ-1): task_create atomicity and the lost-response retry. Stores
    // are checked by DIRECT reads — never through command stdout. ---

    /// A task with a HAND-PICKED id (deterministic prefix-collision setups
    /// must not depend on wall-clock luck — the flake class this task
    /// retires).
    fn task_with_id(id: &str, title: &str) -> Task {
        let mut t = Task::new(title, String::new(), None, Utc::now());
        t.id = Uuid::parse_str(id).expect("crafted fixture id");
        t
    }

    /// REQ-API-01 AC3 (regression): a create whose blocked_by ref is
    /// AMBIGUOUS refuses — and the store keeps NEITHER the task NOR any
    /// edge. Pre-fix, `create_task` inserted the task BEFORE the resolve
    /// loops, so this exact call left an orphan task plus a committed
    /// parent edge (the partially-successful shape).
    #[test]
    fn ambiguous_refusal_leaves_no_orphan_task_or_edges() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        // Two ids sharing the FULL canonical 13-char display prefix —
        // a genuine ambiguity resolve_task must (and does) refuse.
        let a = task_with_id("019e0000-0000-7000-8000-0000000000aa", "collision-a");
        let b = task_with_id("019e0000-0000-7000-8000-0000000000ab", "collision-b");
        api.store.create_task(&a).unwrap();
        api.store.create_task(&b).unwrap();
        let before = api.store.list_tasks().unwrap().len();

        let e = api
            .task_create(TaskCreateReq {
                title: "orphan-candidate".into(),
                project_id: None,
                description: String::new(),
                // the resolvable parent would have been linked BEFORE the
                // failing ref by the old per-loop order — it must be gone too
                parent: vec![a.id.to_string()],
                blocked_by: vec!["019e0000-0000".into()], // matches BOTH
                idempotency_key: None,
            })
            .expect_err("ambiguous ref must refuse");
        assert!(format!("{e}").contains("ambiguous"), "{e}");

        // DIRECT store reads: nothing landed at all.
        assert_eq!(api.store.list_tasks().unwrap().len(), before);
        assert!(
            !api.store
                .list_tasks()
                .unwrap()
                .iter()
                .any(|t| t.title == "orphan-candidate")
        );
        assert!(api.store.incoming_dependencies(a.id).unwrap().is_empty());
        assert!(api.store.incoming_dependencies(b.id).unwrap().is_empty());

        // REQ-API-02 AC1: an identical retry with resolvable refs succeeds
        // and leaves EXACTLY one new task plus exactly its edges — the
        // refused attempt contributed nothing to double.
        api.task_create(TaskCreateReq {
            title: "orphan-candidate".into(),
            project_id: None,
            description: String::new(),
            parent: vec![a.id.to_string()],
            blocked_by: vec![b.id.to_string()],
            idempotency_key: None,
        })
        .expect("retry after refused resolve must not be poisoned");
        let all = api.store.list_tasks().unwrap();
        assert_eq!(all.len(), before + 1);
        let child = all.iter().find(|t| t.title == "orphan-candidate").unwrap();
        assert_eq!(api.store.task_dependencies(child.id).unwrap().len(), 2);
    }

    /// REQ-API-01: a refusal raised by the store's own write-shape checks
    /// (duplicate ref WITHIN one request) also rolls the whole effect back.
    #[test]
    fn duplicate_ref_in_one_request_rolls_back_everything() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let p = fresh_task(&api, "dup-parent");
        let before = api.store.list_tasks().unwrap().len();
        let e = api
            .task_create(TaskCreateReq {
                title: "dup-child".into(),
                project_id: None,
                description: String::new(),
                parent: vec![],
                blocked_by: vec![p.to_string(), p.to_string()],
                idempotency_key: None,
            })
            .expect_err("duplicate edge is loud at write");
        assert!(format!("{e}").contains("already exists"), "{e}");
        assert_eq!(api.store.list_tasks().unwrap().len(), before);
    }

    /// REQ-API-02 / ratified OQ-1: a retry carrying the SAME idempotency
    /// key returns the committed task and its edges — direct store reads
    /// prove one task, one edge set. A keyless call keeps the
    /// every-call-creates-a-task semantics; a DIFFERENT key creates.
    #[test]
    fn idempotency_key_retry_never_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let parent = fresh_task(&api, "idem-parent");
        let req = |key: Option<&str>| TaskCreateReq {
            title: "idem-child".into(),
            project_id: None,
            description: String::new(),
            parent: vec![parent.to_string()],
            blocked_by: vec![],
            idempotency_key: key.map(str::to_string),
        };

        let first = api.task_create(req(Some("retry-1"))).unwrap();
        let again = api.task_create(req(Some("retry-1"))).unwrap();
        assert_eq!(first.task.id, again.task.id, "same key = same task");
        assert_eq!(again.linked.len(), 1, "and exactly its one parent edge");

        let children = || {
            api.store
                .list_tasks()
                .unwrap()
                .iter()
                .filter(|t| t.title == "idem-child")
                .count()
        };
        assert_eq!(children(), 1, "direct read: no duplicate task");
        assert_eq!(api.store.task_dependencies(first.task.id).unwrap().len(), 1);

        // A different key is a different command — it creates.
        let other = api.task_create(req(Some("retry-2"))).unwrap();
        assert_ne!(other.task.id, first.task.id);
        assert_eq!(children(), 2);
        // No key at all: the plain per-call semantics stay.
        api.task_create(req(None)).unwrap();
        api.task_create(req(None)).unwrap();
        assert_eq!(children(), 4);
    }

    /// R2 QA (input-validation): an empty/whitespace-only idempotency key
    /// is refused with the typed `InvalidInput` (machine code
    /// `invalid_input`) BEFORE any store touch — it must never return the
    /// task a previous call registered, and the repeated refusals leave
    /// the store identical (before == after, direct reads). Pre-guard the
    /// CLI passed `''` through: the first empty-keyed create registered a
    /// row under the shared key, the second returned it — exit 0.
    #[test]
    fn empty_idempotency_key_is_typed_refusal_with_zero_store_effect() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        // A REAL keyed create first: the shared-`''` trap would have let an
        // empty-keyed call resolve against the registry and return it.
        api.task_create(TaskCreateReq {
            title: "keyed-real".into(),
            project_id: None,
            description: String::new(),
            parent: vec![],
            blocked_by: vec![],
            idempotency_key: Some("real-key".into()),
        })
        .expect("non-empty key creates");
        let before = api.store.list_tasks().unwrap().len();

        let empty_req = |key: &str| TaskCreateReq {
            title: "keyed-empty".into(),
            project_id: None,
            description: String::new(),
            parent: vec![],
            blocked_by: vec![],
            idempotency_key: Some(key.into()),
        };
        // Repeat each degenerate key: every attempt is a refusal, not the
        // shared-key return of the first task.
        for key in ["", "   ", "\t"] {
            for attempt in 1..=2 {
                let e = api
                    .task_create(empty_req(key))
                    .expect_err("empty/whitespace key must refuse");
                assert_eq!(
                    e.code(),
                    ErrorCode("invalid_input"),
                    "{key:?} #{attempt}: {e}"
                );
                assert_eq!(
                    api.store.list_tasks().unwrap().len(),
                    before,
                    "refusal of {key:?} #{attempt} touched nothing"
                );
            }
        }
        // The refusal must not poison later REAL keyed calls either.
        api.task_create(TaskCreateReq {
            title: "keyed-after".into(),
            project_id: None,
            description: String::new(),
            parent: vec![],
            blocked_by: vec![],
            idempotency_key: Some("real-key-2".into()),
        })
        .expect("guard refuses degenerate keys only");
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
        assert_eq!(
            via_dispatch.allowed_operations,
            via_named.allowed_operations
        );
        assert_eq!(
            via_named.event_kinds,
            vec![event_kind::WORKFLOW_STARTED.to_string()]
        );
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
        assert!(
            format!("{e}").contains("no CURRENT REQUIREMENTS_SPEC artifact to approve"),
            "{e}"
        );
        assert!(api.task_decisions(&id).unwrap().is_empty());

        // Publish the spec through the compound operation, review it in,
        // then approve: mutation + decision row land together.
        let content = br#"{"requirements": "ok"}"#.to_vec();
        api.artifact_publish(PublishReq {
            id: id.clone(),
            kind: ArtifactKind::RequirementsSpec,
            name: "spec.json".into(),
            content,
            depends: vec![],
            contract: None,
            execution: None,
            category: DataCategory::Artifact,
            verdict: None,
        })
        .expect("publish spec");
        api.task_review(&id, Verdict::Approved, None).unwrap();
        let o = api.task_approve(&id, Some("ratified".into())).unwrap();
        assert_eq!(o.mutation.task.state, TaskState::Implementation);
        let ds = api.task_decisions(&id).unwrap();
        assert_eq!(ds.len(), 1);
        assert_eq!(ds[0].kind, decision_kind::REQUIREMENTS_APPROVAL);
        assert_eq!(ds[0].rationale.as_deref(), Some("ratified"));
        assert_eq!(
            ds[0].refs.len(),
            1,
            "the decision refs bind the CURRENT spec id"
        );
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
            .artifact_publish(PublishReq {
                id: a.to_string(),
                kind: ArtifactKind::RequirementsSpec,
                name: "s1".into(),
                content: content("s1"),
                depends: vec![],
                contract: None,
                execution: None,
                category: DataCategory::Artifact,
                verdict: None,
            })
            .unwrap();
        let row = api
            .artifact_publish_row(PublishReq {
                id: b.to_string(),
                kind: ArtifactKind::RequirementsSpec,
                name: "s2".into(),
                content: content("s2"),
                depends: vec![],
                contract: None,
                execution: None,
                category: DataCategory::Artifact,
                verdict: None,
            })
            .unwrap();
        let adv = api
            .artifact_advance(AdvanceReq {
                id: b.to_string(),
                artifact_id: row.artifact.id.to_string(),
                kind: ArtifactKind::RequirementsSpec,
                verdict: None,
            })
            .unwrap();

        assert_eq!(compound.publish.artifact.kind, row.artifact.kind);
        assert_eq!(compound.publish.artifact.seq, row.artifact.seq);
        assert_eq!(
            compound.publish.task.state,
            TaskState::Analysis,
            "compound advance ran too: the task moved"
        );
        // Both paths converge on the same transition facts.
        let after_compound = api.task_get(&a.to_string()).unwrap();
        assert_eq!(after_compound.state, TaskState::Review);
        assert_eq!(after_compound.requirements_seq, 1);
        let after_phased = api.task_get(&b.to_string()).unwrap();
        assert_eq!(after_phased.state, after_compound.state);
        assert_eq!(
            after_phased.requirements_seq,
            after_compound.requirements_seq
        );
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
        api.exec_start(ExecStartReq {
            id: id.clone(),
            role: "analyst".into(),
            agent: None,
            inputs: vec![],
        })
        .unwrap();

        let e = api
            .exec_start(ExecStartReq {
                id: id.clone(),
                role: "analyst".into(),
                agent: None,
                inputs: vec![],
            })
            .unwrap_err();
        assert_eq!(e.code(), ErrorCode("single_writer_busy"));
        let text = format!("{e}");
        assert!(
            text.starts_with(&format!(
                "single-writer guard: task `{t}` already has a RUNNING execution"
            )),
            "{text}"
        );
        assert!(text.contains("`devai exec finish|fail|cancel "), "{text}");
        // Only one RUNNING row exists — the refusal wrote nothing.
        assert_eq!(
            api.exec_list(Some(&id))
                .unwrap()
                .iter()
                .filter(|x| x.status == ExecutionStatus::Running)
                .count(),
            1
        );
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
            .artifact_publish_row(PublishReq {
                id: t.to_string(),
                kind: ArtifactKind::TestReport,
                name: "tests".into(),
                content: b"ok".to_vec(),
                depends: vec![],
                contract: None,
                execution: None,
                category: DataCategory::Artifact,
                verdict: None,
            })
            .unwrap();
        let e = api
            .artifact_advance(AdvanceReq {
                id: t.to_string(),
                artifact_id: row.artifact.id.to_string(),
                kind: ArtifactKind::TestReport,
                verdict: None,
            })
            .unwrap_err();
        assert_eq!(e.code(), ErrorCode("artifact_kind_not_advancing"));
        let text = format!("{e}");
        assert!(
            text.contains("TEST_REPORT carries no workflow advance"),
            "{text}"
        );
        assert!(
            text.contains("REQUIREMENTS_SPEC, REVIEW_REPORT, IMPLEMENTATION_REPORT"),
            "advancing set derived from the core predicate: {text}"
        );
        // The refusal wrote nothing: the task is where the row phase left it.
        assert_eq!(
            api.task_get(&t.to_string()).unwrap().state,
            TaskState::Analysis
        );
    }

    /// Rework R7: the API tests build the quality report through the REAL
    /// assembly pipeline (`evaluation::assemble` → `human_view::build`), not
    /// through a hand-written JSON form. A hand-built form hid the
    /// producer/consumer dictionary mismatch (MECHANICAL_PASS was emitted on
    /// every passed mechanical check but unknown to the form parser), because
    /// the fixture simply never contained one.
    const FORM_PROFILE_YAML: &str = r#"
version: 1
project: formtest
applies_to_roles: [quality-auditor]
error_types:
  correctness/state-transition: { desc: d, severity: catastrophic }
  process/untested-branch: { desc: d, severity: minor }
gates: []
evaluations:
  - id: correctness
    title: T
    checks:
      - id: mech/pass
        statement: mechanical probe passes
        detects: [process/untested-branch]
        verify: mechanical
        anchors: { "1.0": ok }
        reviewer: tool
      - id: corr/floor
        statement: state transition is checked
        detects: [correctness/state-transition]
        verify: rubric
        anchors: { "1.0": ok, "0.4": mid, "0.0": bad }
        reviewer: either
      - id: cl/y
        statement: style drift
        detects: [process/untested-branch]
        verify: rubric
        anchors: { "1.0": ok }
        reviewer: either
scoring:
  theta: 0.8
  floors: { catastrophic: 0.7, major: 0.4, minor: null }
  blocked_policy: { max_blocked_weight: 0.2 }
  weights:
    mech/pass: 0.10
    corr/floor: 0.20
    cl/y: 0.10
"#;

    /// One quality-report document produced by the platform's own assembler:
    /// a passed MECHANICAL check, a breached floor and a nuance, each with a
    /// stored one-line summary and a collapsed evidence layer.
    fn assembled_report_doc() -> serde_json::Value {
        let profile: devai_store::config::Profile =
            serde_yaml::from_str(FORM_PROFILE_YAML).expect("form fixture profile parses");
        let mut mech = std::collections::HashMap::new();
        mech.insert(
            "mech/pass".to_string(),
            devai_runtime::evaluation::ProbeOutcome {
                state: devai_runtime::evaluation::ProbeState::Pass,
                detail: "cargo test: 12 passed".into(),
                output: "12 passed".into(),
            },
        );
        let run = devai_runtime::evaluation::EvalRun {
            gates: vec![],
            mech,
            hybrid: std::collections::HashMap::new(),
            evidence_text: String::new(),
        };
        let assessment = serde_json::json!({
            "scores": { "corr/floor": 0.4, "cl/y": 0.9 },
            "findings": [{
                "check": "corr/floor",
                "text": "the guard is missing at src/a.rs:7",
                "evidence": ["fn step() { unchecked() }"]
            }]
        });
        let revision = WorkspaceRevision {
            git_head: None,
            dirty_digest: None,
            profile: None,
            roles: vec![],
        };
        devai_runtime::evaluation::assemble(
            &profile,
            &run,
            &assessment,
            "ex-form",
            &revision,
            &std::collections::BTreeMap::new(),
        )
        .json
    }

    /// REQ-QH-01/05/06: the presentation channel quotes the STORED human form
    /// (never re-assembles one), and a report that is no longer the last word
    /// is announced as stale — with the task phase — instead of being passed
    /// off as the current decision. Old bodies keep their stored flat text.
    #[test]
    fn the_stored_human_form_is_presented_and_rework_is_announced_as_stale() {
        let dir = tempfile::tempdir().unwrap();
        let api = seeded_api(dir.path());
        let t = fresh_task(&api, "form-presentation");

        let body = assembled_report_doc();
        let row = api
            .artifact_publish_row(PublishReq {
                id: t.to_string(),
                kind: ArtifactKind::QualityReport,
                name: "qr".into(),
                content: serde_json::to_vec(&body).unwrap(),
                depends: vec![],
                contract: Some("quality-report@5".into()),
                execution: None,
                category: DataCategory::Artifact,
                verdict: None,
            })
            .unwrap();

        let p = api.presented_quality_report(t).unwrap().unwrap();
        assert_eq!(p.source, present::RenderSource::HumanForm);
        assert!(
            p.text.contains("БЬЁТ ПО ВЕРДИКТУ") && p.text.contains("НЮАНСЫ"),
            "the stored form is what gets rendered, grouped by decision weight: {}",
            p.text
        );
        assert!(p.text.contains("важность 0.120"));
        // R7/R5: the passed mechanical check came through the real producer
        // and is PRESENTED — not an «unknown state» hole.
        assert!(p.text.contains("mech/pass"), "{}", p.text);
        assert!(
            !p.text.contains("не распознано"),
            "no producer state may fall through as unknown: {}",
            p.text
        );
        assert!(
            !p.text.contains("RAW-FLAT-TEXT-MARKER"),
            "the flat text is the fallback, not the channel, for @5 bodies"
        );
        assert!(!p.freshness.stale);
        assert_eq!(p.freshness.report_status, "CURRENT");
        // The UI view is the SAME read: same text, same source, same verdict.
        let view = api.task_report(&t.to_string()).unwrap();
        assert_eq!(view.report_text.as_deref(), Some(p.text.as_str()));
        assert_eq!(view.source, "human_form");

        // A pre-@5 body still renders its own stored text, byte for byte.
        let t2 = fresh_task(&api, "legacy");
        api.artifact_publish_row(PublishReq {
            id: t2.to_string(),
            kind: ArtifactKind::QualityReport,
            name: "qr".into(),
            content: br#"{"report_text":"LEGACY-FLAT-TEXT"}"#.to_vec(),
            depends: vec![],
            contract: None,
            execution: None,
            category: DataCategory::Artifact,
            verdict: None,
        })
        .unwrap();
        assert_eq!(
            api.quality_report_text(t2).unwrap().as_deref(),
            Some("LEGACY-FLAT-TEXT")
        );

        // Rework (ratified OQ-QH-3) flips the row CURRENT -> STALE; the
        // presentation must say so instead of showing it as the decision.
        let task = api.store.get_task(&t.to_string()).unwrap();
        let mut m = Mutation {
            task: task.clone(),
            events: vec![],
            artifact_status: vec![(row.artifact.id, ArtifactStatus::Stale)],
            questions: vec![],
        };
        m.task.state = TaskState::Rework;
        api.store.commit(task.lock_version, &m).unwrap();

        let p2 = api.presented_quality_report(t).unwrap().unwrap();
        assert!(p2.freshness.stale);
        assert_eq!(p2.freshness.report_status, "STALE");
        assert_eq!(p2.freshness.task_state, "REWORK");
        let text = api.quality_report_text(t).unwrap().unwrap();
        assert_eq!(
            text.lines().next().unwrap_or(""),
            "⚠ отчёт v1 УСТАРЕЛ — задача в REWORK: ре-аудит ещё не запущен",
            "the full formulation is pinned: no execution exists, so none is named"
        );
        assert!(text.contains("НЕ текущее решение"), "{text}");
        assert!(text.contains("НЮАНСЫ"), "the stored form is still readable");
        // The UI view carries the same fact, not a second derivation.
        let view = api.task_report(&t.to_string()).unwrap();
        assert_eq!(view.status, "STALE");
        assert!(view.freshness.expect("freshness is part of the view").stale);
    }

    /// R1/R5: the freshness marker names the STORE fact. A return whose last
    /// auditor run is the COMPLETED one that produced the report is NOT an
    /// unfinished re-audit (the old code invented one), «в работе» appears
    /// only with a live RUNNING row, and both formulations are pinned in full.
    #[test]
    fn freshness_marker_names_the_store_fact_not_an_invented_reaudit() {
        fn auditor_exec(task_id: Id, started: Timestamp) -> RoleExecution {
            RoleExecution {
                id: new_id(),
                task_id,
                role_slug: present::AUDITOR_ROLE.into(),
                role_version: 1,
                agent_name: None,
                status: ExecutionStatus::Running,
                inputs: vec![],
                summary: None,
                error: None,
                started_at: started,
                finished_at: None,
                workspace_revision: None,
                attempt: 1,
                parent_execution: None,
                failure_class: None,
                owner_pid: None,
                owner_host: None,
                owner_process_start: None,
            }
        }

        let dir = tempfile::tempdir().unwrap();
        let api = seeded_api(dir.path());
        let t = fresh_task(&api, "freshness-facts");
        let body = assembled_report_doc();
        let row = api
            .artifact_publish_row(PublishReq {
                id: t.to_string(),
                kind: ArtifactKind::QualityReport,
                name: "qr".into(),
                content: serde_json::to_vec(&body).unwrap(),
                depends: vec![],
                contract: Some("quality-report@5".into()),
                execution: None,
                category: DataCategory::Artifact,
                verdict: None,
            })
            .unwrap();

        // The COMPLETED auditor run that produced the report on screen.
        let started = chrono::DateTime::parse_from_rfc3339("2026-11-06T10:00:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let done = auditor_exec(t, started);
        api.store.start_execution(&done).unwrap();
        api.store
            .finish_execution(
                &done,
                ExecutionStatus::Completed,
                Some("report published"),
                None,
                started + chrono::Duration::minutes(5),
            )
            .unwrap();

        // Return to rework: the rejected report stops being CURRENT (OQ-QH-3).
        let task = api.store.get_task(&t.to_string()).unwrap();
        let mut m = Mutation {
            task: task.clone(),
            events: vec![],
            artifact_status: vec![(row.artifact.id, ArtifactStatus::Stale)],
            questions: vec![],
        };
        m.task.state = TaskState::Rework;
        api.store.commit(task.lock_version, &m).unwrap();

        let p = api.presented_quality_report(t).unwrap().unwrap();
        assert_eq!(
            p.freshness.marker,
            format!(
                "⚠ отчёт v1 УСТАРЕЛ — задача в REWORK: возврат без ре-аудита; последний прогон {} COMPLETED 2026-11-06 10:05:00Z",
                short_id(&done.id)
            ),
            "the completed run that produced the report is not an unfinished re-audit"
        );
        assert!(!p.freshness.marker.contains("в работе"));
        assert!(!p.freshness.marker.contains("не завершён"));
        // The text channel carries the same full marker, not a summary.
        assert_eq!(
            api.quality_report_text(t)
                .unwrap()
                .unwrap()
                .lines()
                .next()
                .unwrap_or(""),
            p.freshness.marker
        );

        // A live re-audit: only NOW is «в работе» a fact of the store.
        let running = auditor_exec(t, Utc::now());
        api.store.start_execution(&running).unwrap();
        let p2 = api.presented_quality_report(t).unwrap().unwrap();
        assert_eq!(
            p2.freshness.marker,
            format!(
                "⚠ отчёт v1 УСТАРЕЛ — задача в REWORK: готовится ре-аудит (execution {}, в работе)",
                short_id(&running.id)
            )
        );
        assert_eq!(
            p2.freshness.execution_status.as_deref(),
            Some("RUNNING"),
            "the view names the live execution, not a finished one"
        );
    }

    /// QA R2 / rework R4: «no report» (Ok(None)), «damaged form» and «the blob
    /// is not JSON at all» are three DISTINGUISHABLE outcomes. A corrupt
    /// CURRENT blob is presented with a marker naming the artifact and a
    /// distinct render source — never as absence of a report and never as the
    /// generic «assembled before @2» placeholder.
    #[test]
    fn quality_report_text_distinguishes_absence_from_corruption() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let t = fresh_task(&api, "gate-text");
        assert_eq!(
            api.quality_report_text(t).unwrap(),
            None,
            "no report yet — honest absence"
        );

        let row = api
            .artifact_publish_row(PublishReq {
                id: t.to_string(),
                kind: ArtifactKind::QualityReport,
                name: "qr".into(),
                content: br#"{"report_text":"all good"}"#.to_vec(),
                depends: vec![],
                contract: None,
                execution: None,
                category: DataCategory::Artifact,
                verdict: None,
            })
            .unwrap();
        assert_eq!(
            api.quality_report_text(t).unwrap().as_deref(),
            Some("all good")
        );

        // Corrupt the stored blob under the CURRENT row: the next read must
        // say so, not fall through to «no report» and not show the
        // pre-@2 placeholder.
        std::fs::write(
            api.store.blob_path(&row.artifact.sha256),
            b"not-json-at-all",
        )
        .unwrap();
        let p = api.presented_quality_report(t).unwrap().unwrap();
        assert_eq!(p.source, present::RenderSource::CorruptBody);
        assert!(p.text.contains("повреждён"), "{}", p.text);
        assert!(!p.text.contains("before @2"), "{}", p.text);
        let view = api.task_report(&t.to_string()).unwrap();
        assert_eq!(view.source, "corrupt_body");
        assert!(
            view.report_text.unwrap_or_default().contains("повреждён"),
            "the UI view carries the same marker"
        );
    }

    /// R3: a structurally damaged `human_view` (a valid JSON body whose form
    /// does not deserialize) is ANNOUNCED. The flat text may still be shown,
    /// but never silently as if it were the stored form.
    #[test]
    fn a_damaged_human_view_is_announced_not_silently_flattened() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let t = fresh_task(&api, "damaged-form");
        let body = serde_json::json!({
            "verdict": "RETURN",
            "recommendation_basis": "красные линии: x",
            "report_text": "FLAT-BODY-STILL-HERE",
            "human_view": { "version": "not-a-number", "headline": 42 },
            "profile": { "project": "p", "version": 1 },
            "evaluation": { "verdict": "FAIL", "score": 0.5, "theta": 0.8, "gates": [], "checks": [] },
        });
        api.artifact_publish_row(PublishReq {
            id: t.to_string(),
            kind: ArtifactKind::QualityReport,
            name: "qr".into(),
            content: serde_json::to_vec(&body).unwrap(),
            depends: vec![],
            contract: None,
            execution: None,
            category: DataCategory::Artifact,
            verdict: None,
        })
        .unwrap();

        let text = api.quality_report_text(t).unwrap().unwrap();
        assert!(
            text.contains("форма отчёта повреждена"),
            "the damaged form must be announced: {text}"
        );
        assert!(text.contains("FLAT-BODY-STILL-HERE"), "{text}");
    }

    /// REQ-MUT-03/04 + human Q2: `Api::open` is the only bootstrap entry —
    /// the home is created through the API and seed lands through the API.
    #[test]
    fn bootstrap_is_encapsulated() {
        let dir = tempfile::tempdir().unwrap();
        let home = dir.path().join("fresh-home");
        let api = api(&home);
        assert!(
            home.join("devai.db").exists(),
            "opening through the API creates the store"
        );
        let seeded = api.seed_config(None).unwrap();
        assert!(seeded > 0);
        assert!(
            !api.role_list().unwrap().is_empty(),
            "seeded roles are visible through the read plane"
        );
    }

    // ---- human paragraph (task 01a1acfe) ---------------------------------
    //
    // The manual `artifact publish` half of REQ-HP-02/03: the SAME gate as
    // the engine (one function, loaded project policy), rejection with the
    // FULL violation list and no side effects — the rejected body never
    // becomes an artifact, there is no failure_class naming and no re-prompt
    // (the manual path has no form loop; parity ratified OQ-FG-05).

    fn seeded_api(dir: &Path) -> Api {
        let api = api(dir);
        api.seed_config(None).unwrap();
        api
    }

    /// REQ-HP-01/02 (manual half): @3 publication without the paragraph is
    /// refused with the field named at its json path; nothing lands.
    #[test]
    fn manual_publish_refuses_a_spec_without_the_paragraph() {
        let dir = tempfile::tempdir().unwrap();
        let api = seeded_api(dir.path());
        let t = fresh_task(&api, "no-paragraph");
        let id = t.to_string();
        api.task_start(&id).unwrap();
        let e = api
            .artifact_publish_row(PublishReq {
                id: id.clone(),
                kind: ArtifactKind::RequirementsSpec,
                name: "spec.json".into(),
                content: br#"{"version":1,"requirements":[],"unknowns":[],"assumptions":[]}"#
                    .to_vec(),
                depends: vec![],
                contract: Some("requirements-specification@3".into()),
                execution: None,
                category: DataCategory::Artifact,
                verdict: None,
            })
            .unwrap_err();
        assert_eq!(e.code(), ErrorCode("contract_violation"));
        let text = format!("{e}");
        assert!(
            text.contains("$.human_paragraph: missing required key"),
            "the refusal names the paragraph: {text}"
        );
        // No failure-class taxonomy on the manual door (there is no loop to
        // classify — the CLI consumer prints `warning:` lines per cause).
        assert!(!text.contains("response_form"), "{text}");
        // And the refusal wrote NOTHING: the rejected body is not an artifact.
        assert_eq!(api.task_get(&id).unwrap().state, TaskState::Analysis);
        let arts = api.store.artifacts(t).unwrap();
        assert!(
            !arts
                .iter()
                .any(|a| a.kind == ArtifactKind::RequirementsSpec),
            "rejected bodies never become artifacts"
        );
    }

    /// REQ-HP-03 (manual half): an unreadable paragraph — «всё хорошо,
    /// работа выполнена» — is refused by the detector with the plain-words
    /// cause; the same body under the OLD @2 contract passes unchanged
    /// (REQ-HP-06 non-retroactivity on the publication door).
    #[test]
    fn manual_publish_detector_and_non_retroactivity() {
        let dir = tempfile::tempdir().unwrap();
        let api = seeded_api(dir.path());
        let t = fresh_task(&api, "chatter");
        let id = t.to_string();
        api.task_start(&id).unwrap();
        let chatter = serde_json::json!({
            "version": 1,
            "requirements": [{"id": "R1", "statement": "Требование."}],
            "unknowns": [],
            "assumptions": [],
            "human_paragraph": "всё хорошо, работа выполнена"
        });
        let e = api
            .artifact_publish_row(PublishReq {
                id: id.clone(),
                kind: ArtifactKind::RequirementsSpec,
                name: "spec.json".into(),
                content: chatter.to_string().into_bytes(),
                depends: vec![],
                contract: Some("requirements-specification@3".into()),
                execution: None,
                category: DataCategory::Artifact,
                verdict: None,
            })
            .unwrap_err();
        assert_eq!(e.code(), ErrorCode("contract_violation"));
        let text = format!("{e}");
        assert!(
            text.contains("below the configured minimum") || text.contains("explanation slot"),
            "detector cause in plain words: {text}"
        );
        // The SAME body is legal under @2 — no paragraph obligation there.
        api.artifact_publish_row(PublishReq {
            id: id.clone(),
            kind: ArtifactKind::RequirementsSpec,
            name: "spec-v2.json".into(),
            content: chatter.to_string().into_bytes(),
            depends: vec![],
            contract: Some("requirements-specification@2".into()),
            execution: None,
            category: DataCategory::Artifact,
            verdict: None,
        })
        .expect("REQ-HP-06: the obligation is not retroactive");
    }

    /// REQ-HP-03 AC5: publishing the platform-assembled quality-report never
    /// runs the detector on its report_text — the §2.6 render is exempt by
    /// construction (its schema has no paragraph field).
    #[test]
    fn quality_report_publish_never_meets_the_paragraph_detector() {
        let dir = tempfile::tempdir().unwrap();
        let api = seeded_api(dir.path());
        let t = fresh_task(&api, "qa-exempt");
        let id = t.to_string();
        api.task_start(&id).unwrap();
        // A report_text that would FAIL the detector as a role field —
        // publish must accept it untouched.
        let body = serde_json::json!({
            "verdict": "ACCEPT",
            "recommendation_basis": "all gates passed",
            "report_text": "ок",
            "profile": { "project": "p", "version": 1 },
            "workspace_revision": {
                "git_head": null, "dirty_digest": null, "profile": null, "roles": []
            },
            "evaluation": { "verdict": "PASS", "score": 0.9, "theta": 0.8, "gates": [], "checks": [] }
        });
        api.artifact_publish_row(PublishReq {
            id: id.clone(),
            kind: ArtifactKind::QualityReport,
            name: "qr.json".into(),
            content: body.to_string().into_bytes(),
            depends: vec![],
            contract: Some("quality-report@4".into()),
            execution: None,
            category: DataCategory::Artifact,
            verdict: None,
        })
        .expect("report_text is exempt from the detector (REQ-HP-03)");
    }

    /// REQ-HP-04 AC1 on the manual door: the project EDITS humanize.yaml
    /// (no rebuild) and the very next publication resolves the new values —
    /// the same paragraph rejected at the factory minimum passes the second
    /// the window is lowered in the config tree.
    #[test]
    fn manual_publish_reads_the_edited_template_without_rebuild() {
        let dir = tempfile::tempdir().unwrap();
        let api = seeded_api(dir.path());
        let t = fresh_task(&api, "config-window");
        let id = t.to_string();
        api.task_start(&id).unwrap();
        let body = serde_json::json!({
            "version": 1,
            "requirements": [{"id": "R1", "statement": "Требование."}],
            "unknowns": [],
            "assumptions": [],
            "human_paragraph": "абзац длиной ровно в проектную норму"
        })
        .to_string();
        let publish = |api: &Api, contract: &str| {
            api.artifact_publish_row(PublishReq {
                id: id.clone(),
                kind: ArtifactKind::RequirementsSpec,
                name: "spec.json".into(),
                content: body.clone().into_bytes(),
                depends: vec![],
                contract: Some(contract.into()),
                execution: None,
                category: DataCategory::Artifact,
                verdict: None,
            })
        };
        // Factory min 200: refused…
        let e = publish(&api, "requirements-specification@3").unwrap_err();
        assert!(
            format!("{e}").contains("below the configured minimum"),
            "{e}"
        );
        // …the project lowers the window in the FILE — no code, no rebuild…
        std::fs::write(
            dir.path()
                .join("config/workflow/humanize.yaml"),
            "humanize:\n  min_chars: 20\n  max_chars: 400\n  ngram: 2\n  overlap_max_ratio: 0.95\n  forbidden_markers: []\n  slots: []\n",
        )
        .unwrap();
        // …and the next publication accepts the same bytes.
        publish(&api, "requirements-specification@3").expect("config is the source of truth");
    }

    /// REQ-HP-05/07: `task_run` stopping at the approval gate carries the
    /// human paragraphs of the CURRENT spec and review — the stored field
    /// verbatim, read without any re-translation, while the machine report
    /// surface (`--json` renders `report`) keeps its exact shape.
    #[test]
    fn approval_gate_presents_the_stored_paragraphs_first() {
        let dir = tempfile::tempdir().unwrap();
        let api = seeded_api(dir.path());
        let o = api
            .task_run(RunReq {
                id: api
                    .task_create(TaskCreateReq {
                        title: "show-me".into(),
                        project_id: None,
                        description: "Показать абзац на гейте".into(),
                        idempotency_key: None,
                        parent: vec![],
                        blocked_by: vec![],
                    })
                    .unwrap()
                    .task
                    .id
                    .to_string(),
                agent: "mock".into(),
                workspace: None,
                force_reaudit: false,
            })
            .unwrap();
        assert_eq!(o.report.gate, Gate::WaitingApproval);
        let labels: Vec<&str> = o
            .gate_human_paragraphs
            .iter()
            .map(|(l, _)| l.as_str())
            .collect();
        assert_eq!(
            labels,
            vec!["requirements-specification v1", "review-report v1"],
            "spec first, review second — the reading order of the gate"
        );
        // REQ-HP-07: what the gate shows IS what the detector accepted and
        // what the store holds — byte-for-byte, no generation at read time.
        let arts = o.report.steps.clone();
        assert_eq!(arts.len(), 2, "analyst + reviewer ran");
        for (label, text) in &o.gate_human_paragraphs {
            let kind = if label.starts_with("requirements") {
                ArtifactKind::RequirementsSpec
            } else {
                ArtifactKind::ReviewReport
            };
            let a = api
                .store
                .artifacts(o.report.task_id)
                .unwrap()
                .into_iter()
                .find(|x| x.kind == kind && x.status == ArtifactStatus::Current)
                .unwrap();
            let body: serde_json::Value =
                serde_json::from_slice(&api.store.read_blob(&a.sha256).unwrap()).unwrap();
            assert_eq!(body["human_paragraph"].as_str().unwrap(), text.as_str());
        }
    }

    // --- REQ-BIND-02/03 (C5 D1): the project-binding plane ----------------

    fn devai_project(api: &Api) -> Project {
        api.project_add(ProjectAddReq {
            name: "devai".into(),
            description: String::new(),
            repos: vec![],
        })
        .expect("project add")
    }

    /// REQ-BIND-02 AC1/AC2/AC3: an effective set bumps the lock, moves the
    /// row and appends the event; re-applying the SAME project is a
    /// documented side-effect-free NO-OP; a stale `expected_lock` refuses
    /// BEFORE any write.
    #[test]
    fn task_project_set_binds_noops_on_replay_and_refuses_stale_lock() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let p = devai_project(&api);
        let t = fresh_task(&api, "to-bind");

        let o = api
            .task_project_set(TaskProjectSetReq {
                id: t.to_string(),
                project: p.name.clone(),
                expected_lock: None,
            })
            .expect("binding lands");
        assert!(o.changed);
        assert_eq!(o.task.project_id, Some(p.id));
        assert_eq!(o.task.lock_version, 2);

        // The event landed append-only with the vocabulary payload.
        let evs = api.task_events(&t.to_string(), None).unwrap();
        let set = evs
            .iter()
            .find(|e| e.kind == event_kind::TASK_PROJECT_SET)
            .expect("task.project.set event");
        assert_eq!(
            set.payload["project_id"].as_str(),
            Some(p.id.to_string().as_str())
        );
        assert!(set.payload["previous_project_id"].is_null());

        // Idempotent re-apply: same project → `changed=false`, NO lock bump,
        // NO new event.
        let o2 = api
            .task_project_set(TaskProjectSetReq {
                id: t.to_string(),
                project: p.id.to_string(),
                expected_lock: None,
            })
            .expect("replay succeeds");
        assert!(!o2.changed);
        assert_eq!(o2.task.lock_version, 2);
        let evs2 = api.task_events(&t.to_string(), None).unwrap();
        assert_eq!(evs2.len(), evs.len(), "the replay wrote nothing");

        // Stale `expected_lock` refuses typed `conflict` — and the row stays.
        let err = api
            .task_project_set(TaskProjectSetReq {
                id: t.to_string(),
                project: p.name.clone(),
                expected_lock: Some(1),
            })
            .expect_err("a stale lock must refuse");
        assert_eq!(err.code(), ErrorCode("conflict"));
        assert_eq!(api.store.get_task(&t.to_string()).unwrap().lock_version, 2);
    }

    /// REQ-BIND-02 AC1: both refusal families are TYPED — an unknown task
    /// ref and an unknown project ref each carry the machine code
    /// `not_found`; nothing is written on either path.
    #[test]
    fn task_project_set_refusals_are_typed() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let p = devai_project(&api);
        let t = fresh_task(&api, "typed");

        let err = api
            .task_project_set(TaskProjectSetReq {
                id: "no-such-task".into(),
                project: p.name.clone(),
                expected_lock: None,
            })
            .expect_err("unknown task refuses");
        assert_eq!(err.code(), ErrorCode("not_found"));
        assert!(format!("{err}").contains("task"), "{err}");

        let err = api
            .task_project_set(TaskProjectSetReq {
                id: t.to_string(),
                project: "no-such-project".into(),
                expected_lock: None,
            })
            .expect_err("unknown project refuses");
        assert_eq!(err.code(), ErrorCode("not_found"));
        assert!(format!("{err}").contains("project"), "{err}");

        // Neither refusal moved the row.
        let stored = api.store.get_task(&t.to_string()).unwrap();
        assert_eq!(stored.project_id, None);
        assert_eq!(stored.lock_version, 1);
    }

    /// REQ-BIND-03 under the ratified OQ-BIND-2 rule: the backfill binds
    /// every UNBOUND NON-TERMINAL task, leaves terminal history at its old
    /// address, never re-binds another project's task, and is idempotent on
    /// re-application. Each binding is one `task.project.set` event.
    #[test]
    fn task_project_backfill_applies_the_ratified_rule_idempotently() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let p = devai_project(&api);
        let other = api
            .project_add(ProjectAddReq {
                name: "other".into(),
                description: String::new(),
                repos: vec![],
            })
            .unwrap();

        let free = fresh_task(&api, "unbound-draft");
        let owned = fresh_task(&api, "owned-by-other");
        api.task_project_set(TaskProjectSetReq {
            id: owned.to_string(),
            project: other.id.to_string(),
            expected_lock: None,
        })
        .unwrap();
        let closed = fresh_task(&api, "terminal-history");
        // Park it terminal through the workflow plane (CANCELLED).
        api.task_op(&closed.to_string(), WorkflowOp::Cancel { reason: None })
            .unwrap();
        let home = fresh_task(&api, "already-devai");
        api.task_project_set(TaskProjectSetReq {
            id: home.to_string(),
            project: p.id.to_string(),
            expected_lock: None,
        })
        .unwrap();

        let o = api.task_project_backfill("devai").expect("backfill runs");
        assert_eq!(o.bound, vec![free]);
        assert_eq!(o.already_bound, vec![home]);
        assert_eq!(o.skipped_terminal, vec![closed]);
        assert_eq!(o.skipped_other_project, vec![owned]);

        let stored = api.store.get_task(&free.to_string()).unwrap();
        assert_eq!(stored.project_id, Some(p.id));
        assert_eq!(stored.lock_version, 2);
        let evs = api.task_events(&free.to_string(), None).unwrap();
        assert_eq!(
            evs.iter()
                .filter(|e| e.kind == event_kind::TASK_PROJECT_SET)
                .count(),
            1
        );
        // Terminal history untouched, another project's task not re-bound.
        assert_eq!(
            api.store.get_task(&closed.to_string()).unwrap().project_id,
            None
        );
        assert_eq!(
            api.store.get_task(&owned.to_string()).unwrap().project_id,
            Some(other.id)
        );

        // Re-application: no candidates left, nothing changes anywhere.
        let o2 = api.task_project_backfill("devai").unwrap();
        assert!(o2.bound.is_empty());
        assert_eq!(o2.already_bound.len(), 2);
        assert_eq!(
            api.store.get_task(&free.to_string()).unwrap().lock_version,
            2
        );

        // Unknown project refuses typed, before touching a row.
        let err = api
            .task_project_backfill("no-such")
            .expect_err("unknown project refuses");
        assert_eq!(err.code(), ErrorCode("not_found"));
    }
}
