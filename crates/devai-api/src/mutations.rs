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

use chrono::{DateTime, Utc};
use std::path::{Path, PathBuf};

use devai_core::types::*;
use devai_core::workflow::{self, Mutation};
use devai_runtime::human_view::present::{self, PresentedReport, ReportFreshness};
use devai_runtime::launch::{self, LaunchOutcome, LaunchRequest};
use devai_runtime::reclaim;
use devai_runtime::{Engine, Gate, RunReport, liveness};
use devai_store::config::ProcessPolicy;
use devai_store::{ArtifactDraft, StoreError, bundle, guess_media_type, seed};
use serde::Serialize;
use uuid::Uuid;

use crate::inbox::pick_spec_artifact;
use crate::{Api, ApiError, ApiResult, SessionKind};

/// The stored facts of ONE attempt-limit trip, parsed STRICTLY (rework R4):
/// the answer writes them into a decision and a reset event, so a corrupt trip
/// must refuse loudly instead of recording an empty role and zero attempts as
/// if they were true.
struct AttemptTrip {
    role: String,
    attempts: u32,
    limit: u32,
}

impl AttemptTrip {
    fn parse(trip: &serde_json::Value) -> ApiResult<AttemptTrip> {
        Ok(AttemptTrip {
            role: Self::text(trip, json_field::ROLE)?,
            attempts: Self::count(trip, json_field::ATTEMPT)?,
            limit: Self::count(trip, json_field::LIMIT)?,
        })
    }

    fn text(trip: &serde_json::Value, key: &'static str) -> ApiResult<String> {
        trip.get(key)
            .and_then(|v| v.as_str())
            .map(str::to_string)
            .filter(|value| !value.is_empty())
            .ok_or_else(|| corrupt_stop_fact(key))
    }

    fn count(trip: &serde_json::Value, key: &'static str) -> ApiResult<u32> {
        trip.get(key)
            .and_then(|v| v.as_u64())
            .and_then(|n| u32::try_from(n).ok())
            .ok_or_else(|| corrupt_stop_fact(key))
    }
}

/// A trip event missing one of its facts is refused by name: the decision and
/// the reset event stay truthful or are not written at all.
fn corrupt_stop_fact(key: &str) -> ApiError {
    ApiError::refused(
        "corrupt_stop_fact",
        format!("attempt-limit trip event has no usable `{key}` — the stop fact cannot be trusted"),
    )
}

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

/// `task.rename` (ratified OQ-TN-1) — set/replace the human signboard of
/// an existing task directly. `id` is a full id or a unique prefix, resolved
/// through `task.get` semantics first; `name` is the new signboard (window-
/// checked against the declarative task-name template before any write);
/// `expected_lock` is the OPTIONAL optimistic lock the writer added last —
/// a stale value refuses typed `conflict` before any write.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskRenameReq {
    pub id: String,
    pub name: String,
    pub expected_lock: Option<u64>,
}

/// The outcome of `task.rename`: the task row AS STORED after the call plus
/// the accepted (trimmed) name; `changed=false` exactly when the task
/// already carried it and nothing was written.
#[derive(Debug, Clone, serde::Serialize)]
pub struct TaskRenameOutcome {
    pub task: Task,
    pub human_name: String,
    pub changed: bool,
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
    /// Человек проходит путь заново с названной стадии.
    Rewind {
        to: String,
        reason: String,
    },
    /// Человек переписал изначальную постановку задачи.
    Restate {
        title: Option<String>,
        description: Option<String>,
        reason: String,
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

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
    /// The run this outcome belongs to (REQ-AR-04/07). A `--wait` caller
    /// reads it back from the stored evidence; `None` on the inline path.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
}

/// The typed outcome of an ASYNC launch (REQ-AR-04): the run number, the
/// execution the caller can quote, and which of the three states happened.
/// `--wait` is a separate caller concern; this type never waits.
#[derive(Serialize, Clone, Debug)]
pub struct RunStartOutcome {
    /// The run number — an id minted by the launch itself (`task_run`,
    /// `new_id()`), NOT the id of any execution: the row that carries it is
    /// a different id, minted later by the store when the runner opens it.
    pub run_id: String,
    /// The execution handle: the first execution of this launch, or — for
    /// `already_running` — the execution that already owns the task
    /// (REQ-AR-06: the refusal names it and touches nothing).
    pub execution_id: Option<String>,
    pub outcome: RunLaunch,
    /// Why the launch was refused («нельзя» carries a reason, REQ-AR-04).
    pub reason: Option<String>,
    /// The task state the launch was asked in.
    pub state: String,
    /// The platform's ONE sentence for this outcome (REQ-LV-05 AC3): the
    /// window's «перезапустить» prints it verbatim and the terminal prints it
    /// before its own ids — nobody re-words the typed outcome.
    pub line: String,
}

#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RunLaunch {
    Started,
    AlreadyRunning,
    Refused,
}

/// The supervisor's request: one detached runner, one run.
#[derive(Debug, Clone)]
pub struct SupervisedRunReq {
    pub task_id: String,
    pub agent: String,
    pub workspace: Option<PathBuf>,
    pub force_reaudit: bool,
    pub run_id: String,
    /// Where the caller expects the handshake (plain file, atomic rename).
    pub ack: PathBuf,
}

/// Потолок сериализованной спеки (байты). Решение оператора 09.11: держим
/// жёстко в публикации, а не только пожеланием в промпте роли.
pub const MAX_SPEC_BYTES: usize = 20_000;

/// Проверка потолка спеки, вынесенная из публикации: чистая, чтобы её можно
/// было проверить тестом без сборки дома. Другие виды артефактов не трогает.
fn spec_size_guard(kind: ArtifactKind, len: usize) -> ApiResult<()> {
    if kind == ArtifactKind::RequirementsSpec && len > MAX_SPEC_BYTES {
        return Err(ApiError::InvalidInput {
            field: "spec_size",
            message: format!(
                "спека {len} байт при потолке {MAX_SPEC_BYTES} байт: сократите прозу и повторы в критериях — \
                 платформа передаёт спеку роли одним аргументом, и большая спека не запускает роль"
            ),
        });
    }
    Ok(())
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
    /// The RUN this row belongs to (REQ-AR-04/07). A detached runner passes
    /// the number its caller minted; a manual `exec start` passes `None` and
    /// the row becomes its own run (its id).
    pub run_id: Option<String>,
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
        // task. The typed refusal happens BEFORE the first store touch.
        Self::refuse_empty_creation_key(&req)?;
        // REQ-API-02 (ratified OQ-1): a retry carrying the key of an ALREADY
        // COMMITTED create returns the registered task and its stored ACTIVE
        // edges — the lost-response retry never duplicates the effect.
        if let Some(done) = self.creation_by_key(&req)? {
            return Ok(done);
        }
        // REQ-API-01 (the QA-01a1acab-18b1 fix): EVERY ref is resolved
        // BEFORE anything is written, so a late refusal leaves no ORPHAN
        // task and the retry cannot double the effect.
        let deps = self.creation_dependencies(&req)?;
        self.insert_task_with_edges(req, &deps)
    }

    /// The typed refusal of an empty/whitespace-only idempotency key — it
    /// can neither read nor write, so a refusal leaves the store identical.
    fn refuse_empty_creation_key(req: &TaskCreateReq) -> ApiResult<()> {
        match req.idempotency_key.as_deref() {
            Some(key) if key.trim().is_empty() => Err(ApiError::InvalidInput {
                field: "idempotency_key",
                message: "must not be empty or whitespace-only".into(),
            }),
            _ => Ok(()),
        }
    }

    /// The already-committed create under this key, if any. The registry row
    /// lands inside the creation transaction, so finding it is proof the
    /// first create committed.
    fn creation_by_key(&self, req: &TaskCreateReq) -> ApiResult<Option<TaskCreated>> {
        let Some(key) = req.idempotency_key.as_deref() else {
            return Ok(None);
        };
        match self.store.task_by_creation_key(key)? {
            Some(existing) => Ok(Some(TaskCreated {
                linked: self.store.task_dependencies(existing.id)?,
                task: existing,
            })),
            None => Ok(None),
        }
    }

    /// REQ-DEP-07: a derived task records WHY it exists at the moment it is
    /// created. Resolution order stays parents-then-blockers, so the first
    /// failing ref produces the identical error text of the old path.
    fn creation_dependencies(&self, req: &TaskCreateReq) -> ApiResult<Vec<(Task, DependencyKind)>> {
        let mut deps: Vec<(Task, DependencyKind)> = Vec::new();
        for p in &req.parent {
            deps.push((self.store.resolve_task(p)?, DependencyKind::Parent));
        }
        for b in &req.blocked_by {
            deps.push((self.store.resolve_task(b)?, DependencyKind::Blocks));
        }
        Ok(deps)
    }

    /// The task, its edges and the key registration land in ONE store
    /// transaction (REQ-API-01: all effects of this call, or none), so
    /// `TaskCreated` can never describe an edge absent from the store.
    fn insert_task_with_edges(
        &self,
        req: TaskCreateReq,
        deps: &[(Task, DependencyKind)],
    ) -> ApiResult<TaskCreated> {
        let t = Task::new(req.title, req.description, req.project_id, Utc::now());
        let edges: Vec<(&Task, DependencyKind)> = deps.iter().map(|(t, k)| (t, *k)).collect();
        let linked = self.store.create_task_with_dependencies(
            &t,
            &edges,
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
    ///
    /// The PROCESS DECLARATION is handed to every command: the command names
    /// the condition, the declaration names the target (REQ-AR-09). One load
    /// per op — a broken/foreign declaration is a typed refusal before any
    /// mutation runs.
    pub fn task_op(&self, id: &str, op: WorkflowOp) -> ApiResult<MutationResult> {
        let process = self.process_policy()?;
        let p = &process;
        match op {
            WorkflowOp::Start => {
                self.run_op(id, |t, _a| workflow::start(p, t, &self.actor, Utc::now()))
            }
            WorkflowOp::Review { verdict, summary } => self.run_op(id, move |t, _a| {
                if verdict == Verdict::Approved {
                    workflow::review_recorded(
                        p,
                        t,
                        verdict,
                        summary.as_deref(),
                        &self.actor,
                        Utc::now(),
                    )
                } else {
                    // Отказ ревью — вопрос человеку, а не следующий круг.
                    workflow::park_review_rejection(
                        p,
                        t,
                        new_id(),
                        verdict,
                        summary.as_deref(),
                        &self.actor,
                        Utc::now(),
                    )
                }
            }),
            WorkflowOp::Approve { rationale } => self.op_approve(id, rationale),
            WorkflowOp::Reject { rationale } => self.run_op(id, move |t, _a| {
                workflow::reject_requirements(p, t, rationale.as_deref(), &self.actor, Utc::now())
            }),
            WorkflowOp::Accept { rationale } => self.op_accept(id, rationale),
            WorkflowOp::Return { rationale } => self.run_op(id, move |t, a| {
                workflow::return_to_rework(p, t, a, rationale.as_deref(), &self.actor, Utc::now())
            }),
            WorkflowOp::Rewind { to, reason } => self.run_op(id, move |t, a| {
                let target = TaskState::parse(&to)
                    .map_err(|_| devai_core::CoreError::guard(format!("unknown stage `{to}`")))?;
                workflow::rewind(p, t, target, &reason, a, &self.actor, Utc::now())
            }),
            WorkflowOp::Restate {
                title,
                description,
                reason,
            } => self.run_op(id, move |t, a| {
                workflow::restate(
                    p,
                    t,
                    title.as_deref(),
                    description.as_deref(),
                    &reason,
                    a,
                    &self.actor,
                    Utc::now(),
                )
            }),
            WorkflowOp::Pause { reason } => self.run_op(id, move |t, _a| {
                workflow::pause(p, t, reason.as_deref(), &self.actor, Utc::now())
            }),
            WorkflowOp::Resume => self.op_resume(id),
            WorkflowOp::Block { reason } => self.run_op(id, move |t, _a| {
                workflow::block(p, t, reason.as_deref(), &self.actor, Utc::now())
            }),
            WorkflowOp::Cancel { reason } => self.run_op(id, move |t, _a| {
                workflow::cancel(p, t, reason.as_deref(), &self.actor, Utc::now())
            }),
            WorkflowOp::Fail { reason } => self.run_op(id, move |t, _a| {
                workflow::fail(p, t, reason.as_deref(), &self.actor, Utc::now())
            }),
        }
    }

    /// APPROVAL: the gate reads the CURRENT spec exactly as the old closure
    /// did — same guard, same refusal text.
    fn op_approve(&self, id: &str, rationale: Option<String>) -> ApiResult<MutationResult> {
        let process = self.process_policy()?;
        self.run_op(id, move |t, a| {
            if a.iter().any(|x| {
                x.kind == ArtifactKind::RequirementsSpec && x.status == ArtifactStatus::Current
            }) {
                workflow::approve_requirements(
                    &process,
                    t,
                    rationale.as_deref(),
                    &self.actor,
                    Utc::now(),
                )
            } else {
                Err(devai_core::CoreError::guard(
                    "no CURRENT REQUIREMENTS_SPEC artifact to approve",
                ))
            }
        })
    }

    /// ACCEPT: REQ-DEP-03 — the acceptance gate consults the stored
    /// dependency graph and the override decisions before the core command
    /// runs; the loads keep the old position.
    fn op_accept(&self, id: &str, rationale: Option<String>) -> ApiResult<MutationResult> {
        let task = self.store.resolve_task(id)?;
        let deps = self.store.task_dependencies(task.id)?;
        let decisions = self.store.list_decisions(task.id)?;
        let process = self.process_policy()?;
        self.run_op(id, move |t, a| {
            workflow::accept(
                &process,
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

    /// RESUME: the same stored dependency/decision loads the acceptance gate
    /// performs, in the old order.
    fn op_resume(&self, id: &str) -> ApiResult<MutationResult> {
        let task = self.store.resolve_task(id)?;
        let deps = self.store.task_dependencies(task.id)?;
        let decisions = self.store.list_decisions(task.id)?;
        let process = self.process_policy()?;
        self.run_op(id, move |t, _a| {
            workflow::resume(&process, t, &deps, &decisions, &self.actor, Utc::now())
        })
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
        // REQ-AR-11/12: entering a state a ROLE executes starts that run —
        // decided by the platform from the declaration, after the human act
        // is safely stored (a failed launch never undoes the decision).
        self.autostart_if_role_state(&m.task);
        // The operations this answer lists name the DECLARED executor of the
        // resulting state (REQ-AR-09 AC1) — read from the declaration here.
        let process = self.process_policy()?;
        Ok(MutationResult {
            task: m.task.clone(),
            previous_state: task.state,
            event_kinds: m.events.iter().map(|e| e.kind.to_string()).collect(),
            staled,
            superseded,
            allowed_operations: workflow::allowed_ops(&process, &m.task),
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
    /// The id of the CURRENT REQUIREMENTS_SPEC, or `None`. ONE selector —
    /// `inbox::pick_spec_artifact` — shared by the approve and reject refs
    /// (rework R6: the same lookup block used to be copied at both sites).
    fn current_spec_ref(&self, task_id: Id) -> ApiResult<Option<String>> {
        Ok(pick_spec_artifact(&self.store.artifacts(task_id)?)
            .filter(|(_, current)| *current)
            .map(|(artifact, _)| artifact.id.to_string()))
    }

    pub fn task_reject(&self, id: &str, rationale: Option<String>) -> ApiResult<MutationResult> {
        // REQ-SIGN-05 AC3 (ASM-13): the rejection is a stored DECISION row,
        // not only an event — the same act the CLI `task reject` performs.
        // refs cite the CURRENT spec when one exists; the APPROVAL→ANALYSIS
        // transition itself is untouched.
        let task = self.store.resolve_task(id)?;
        // ONE selector for "the CURRENT spec" (rework R6): the lookup used to
        // be copied here and in task_approve.
        let spec_ref = self.current_spec_ref(task.id)?;
        let mutation = self.task_op(
            id,
            WorkflowOp::Reject {
                rationale: rationale.clone(),
            },
        )?;
        let decision = Decision {
            id: new_id(),
            task_id: task.id,
            kind: decision_kind::REQUIREMENTS_REJECTION.into(),
            verdict: Verdict::Rejected,
            actor: self.actor.clone(),
            rationale,
            refs: spec_ref.into_iter().collect(),
            created_at: Utc::now(),
            payload: serde_json::Value::Null,
        };
        self.store.record_decision(&decision)?;
        Ok(mutation)
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
    /// Пройти путь заново с названной стадии (только человек): результаты этой
    /// стадии и всех следующих помечаются STALE, причина ложится в журнал.
    pub fn task_rewind(&self, id: &str, to: &str, reason: &str) -> ApiResult<MutationResult> {
        self.task_op(
            id,
            WorkflowOp::Rewind {
                to: to.to_string(),
                reason: reason.to_string(),
            },
        )
    }
    /// Переписать изначальную постановку задачи (только человек). Правка
    /// начатого пути возвращает его в STATEMENT.
    pub fn task_restate(
        &self,
        id: &str,
        title: Option<String>,
        description: Option<String>,
        reason: &str,
    ) -> ApiResult<MutationResult> {
        self.task_op(
            id,
            WorkflowOp::Restate {
                title,
                description,
                reason: reason.to_string(),
            },
        )
    }
    pub fn task_fail(&self, id: &str, reason: Option<String>) -> ApiResult<MutationResult> {
        self.task_op(id, WorkflowOp::Fail { reason })
    }

    /// APPROVAL → IMPLEMENTATION plus the `requirements_approval` decision
    /// row (the rationale becomes auditable data, precedent 01a1a0b5-0fcf).
    /// The spec-ref load and the refusal positions mirror the old handler.
    pub fn task_approve(&self, id: &str, rationale: Option<String>) -> ApiResult<ApproveOutcome> {
        let task = self.store.resolve_task(id)?;
        // The SAME selector the reject path uses (rework R6).
        let spec_ref = self.current_spec_ref(task.id)?.unwrap_or_default();
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
            payload: serde_json::Value::Null,
        };
        self.store.record_decision(&decision)?;
        Ok(ApproveOutcome { mutation, decision })
    }

    // ---- task: run / revise ------------------------------------------------

    pub fn task_run_inline(&self, req: RunReq) -> ApiResult<RunOutcome> {
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
        self.run_view(report, None)
    }

    /// The rendering half of a settled run: the gate presentation (stored
    /// quality report, freshness, human paragraphs) plus the allowed
    /// operations. Shared by the inline drive and the `--wait` read-back of a
    /// detached run, so both paths show the operator the SAME gate view.
    pub(crate) fn run_view(
        &self,
        report: RunReport,
        run_id: Option<String>,
    ) -> ApiResult<RunOutcome> {
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
        let current_quality_report = self.fresh_quality_report(&report)?;
        let gate_human_paragraphs = self.gate_paragraphs(&report)?;
        let t = self.store.get_task(&report.task_id.to_string())?;
        Ok(RunOutcome {
            report,
            current_quality_report,
            gate_report_text,
            gate_report_freshness,
            gate_human_paragraphs,
            allowed_operations: workflow::allowed_ops(&self.process_policy()?, &t),
            run_id,
        })
    }

    /// The CURRENT quality report the FRESH gate presents (REQ-005): the row
    /// the gate names, read from the store — never re-derived from the report.
    fn fresh_quality_report(&self, report: &RunReport) -> ApiResult<Option<Artifact>> {
        if report.gate != Gate::ReportFresh {
            return Ok(None);
        }
        Ok(self
            .store
            .artifacts(report.task_id)?
            .into_iter()
            .find(|x| x.kind == ArtifactKind::QualityReport && x.status == ArtifactStatus::Current))
    }

    /// The paragraph half of the gate presentation (REQ-HP-05, ratified
    /// OQ-HP-1): approval/review reads spec + review, acceptance reads the
    /// implementation-report. Stored fields, verbatim; absence (a legacy
    /// @1/@2 CURRENT, a non-JSON body) renders as NO paragraph, never as a
    /// broken gate (REQ-HP-06 non-retroactivity in the render half too).
    fn gate_paragraphs(&self, report: &RunReport) -> ApiResult<Vec<(String, String)>> {
        let mut out: Vec<(String, String)> = Vec::new();
        match report.gate {
            Gate::WaitingApproval => {
                for (kind, label) in [
                    (ArtifactKind::RequirementsSpec, "requirements-specification"),
                    (ArtifactKind::ReviewReport, "review-report"),
                ] {
                    if let Some(p) = self.stored_human_paragraph(report.task_id, kind, label)? {
                        out.push(p);
                    }
                }
            }
            Gate::WaitingAccept | Gate::ReportFresh => {
                if let Some(p) = self.stored_human_paragraph(
                    report.task_id,
                    ArtifactKind::ImplementationReport,
                    "implementation-report",
                )? {
                    out.push(p);
                }
            }
            _ => {}
        }
        Ok(out)
    }

    // ---- asynchronous launch (REQ-AR-01..08) -------------------------------

    /// `task run` — the ASYNC launch (REQ-AR-04): mint a run number, start a
    /// detached platform runner, and return the handle as soon as the runner
    /// has opened the execution row. The caller never waits for a role, and
    /// the role never dies with the caller (REQ-AR-01).
    ///
    /// Outcomes are typed, not exceptions: `started` (with the execution id),
    /// `already_running` (with the id of the execution that owns the task —
    /// REQ-AR-06, and the first run is not touched) and `refused` (a reason —
    /// «нельзя», e.g. the state is executed by a human). A spawn/handshake
    /// failure is NOT one of them: nothing was started, it is recorded as a
    /// launch failure event (REQ-AR-13) and returned as a typed refusal.
    pub fn task_run(&self, req: RunReq) -> ApiResult<RunStartOutcome> {
        let run_id = new_id().to_string();
        let task = self.store.resolve_task(&req.id)?;
        // REQ-AR-05/08: a provably dead owner must not hold the slot while a
        // new launch asks for it — the platform closes it first.
        reclaim::reclaim(&self.store, Some(task.id))?;
        let task = self.store.resolve_task(&req.id)?;
        self.launch_now(&task, &req, run_id)
    }

    /// The launch proper, after the dead-owner sweep. Every outcome is TYPED
    /// (started / already running with the owner's id / refused with a
    /// reason); a launch that did not happen is an EVENT with the consecutive
    /// counter (REQ-AR-04/06/13), never silence.
    fn launch_now(&self, task: &Task, req: &RunReq, run_id: String) -> ApiResult<RunStartOutcome> {
        let state = task.state.as_str().to_string();
        let answer = |execution_id: Option<String>, outcome: RunLaunch, reason: Option<String>| {
            let line = crate::launch_line(outcome, execution_id.as_deref(), reason.as_deref());
            RunStartOutcome {
                run_id: run_id.clone(),
                execution_id,
                outcome,
                reason,
                state: state.clone(),
                line,
            }
        };
        if let Some(reason) = self.state_launch_refusal(task)? {
            return Ok(answer(None, RunLaunch::Refused, Some(reason)));
        }
        let launched = launch::launch(&self.launch_request(task, req, &run_id));
        self.settle_launch(task, launched, answer)
    }

    /// One place where a launch outcome becomes the caller's typed answer. A
    /// refusal that came back FROM the runner means the launch was attempted
    /// and did not happen (no executable agent, a tripped brake): REQ-AR-13
    /// makes it an EVENT with the counter, never silence. The check inside
    /// `task_run` (a human-executed state) is a typed «нельзя» — nothing was
    /// tried, so nothing is recorded as a failure.
    fn settle_launch<F>(
        &self,
        task: &Task,
        launched: Result<LaunchOutcome, devai_runtime::LaunchError>,
        answer: F,
    ) -> ApiResult<RunStartOutcome>
    where
        F: Fn(Option<String>, RunLaunch, Option<String>) -> RunStartOutcome,
    {
        match launched {
            Ok(LaunchOutcome::Started { execution_id, .. }) => {
                Ok(answer(Some(execution_id), RunLaunch::Started, None))
            }
            Ok(LaunchOutcome::AlreadyRunning { execution_id, .. }) => Ok(answer(
                Some(execution_id),
                RunLaunch::AlreadyRunning,
                Some(
                    "one owner per task: the first run keeps running and was not touched"
                        .to_string(),
                ),
            )),
            Ok(LaunchOutcome::Refused { reason }) => {
                // REQ-AL-05: a refusal that came WITH a park (the attempt
                // limit stopped the task) is the stop itself, not a failed
                // launch — counting it here would speak a second language
                // about one event and could reach the consecutive brake.
                let now = self.store.get_task(&task.id.to_string())?;
                if now.state != task.state && now.state.is_hold() {
                    return Ok(answer(None, RunLaunch::Refused, Some(reason)));
                }
                self.record_launch_failure(task, failure_class::PLATFORM, &reason)?;
                Ok(answer(None, RunLaunch::Refused, Some(reason)))
            }
            Err(e) => {
                self.record_launch_failure(
                    task,
                    crate::launch_brakes::launch_failure_class(&e),
                    &e.to_string(),
                )?;
                Err(ApiError::refused(
                    "launch_failed",
                    format!("launch failed: {e}"),
                ))
            }
        }
    }

    /// The one launch request this call describes: the home, the task, the run
    /// number and the caller's choices. Platform facts (actor, isolation) come
    /// from THIS process, never the caller.
    fn launch_request(&self, task: &Task, req: &RunReq, run_id: &str) -> LaunchRequest {
        LaunchRequest {
            home: self.home.clone(),
            task_id: task.id.to_string(),
            agent: req.agent.clone(),
            workspace: req.workspace.clone(),
            force_reaudit: req.force_reaudit,
            actor: self.actor.clone(),
            run_id: run_id.to_string(),
            // The runner THIS handle resolved; `None` keeps the platform's own
            // resolution (an explicit seam, not a global env read at the door).
            runner: self.runner.clone(),
        }
    }

    /// The detached runner's entry (the CLI's hidden `__supervise` command):
    /// DRAFT bookkeeping, brakes, the execution row (owned by THIS process),
    /// the handshake, then the engine drive of the whole chain. Every refusal
    /// travels through the ack file, so the caller that already returned still
    /// gets no phantom handle (REQ-AR-08).
    pub fn task_run_supervised(&self, req: &SupervisedRunReq) -> ApiResult<Option<RunReport>> {
        match self.supervised_inner(req) {
            Ok(report) => Ok(report),
            Err(e) => {
                // The caller may have already given up (timeout); writing the
                // ack is best effort — the log keeps the reason either way.
                let _ = launch::write_ack(&req.ack, &format!("refused {e}"));
                Err(e)
            }
        }
    }

    fn supervised_inner(&self, req: &SupervisedRunReq) -> ApiResult<Option<RunReport>> {
        let task = self.draft_booking(req)?;
        let policy = self.process_policy()?;
        let Some(role) = self.admit_launch(&task, &policy, req)? else {
            return Ok(None);
        };
        let Some(started) = self.open_owned_execution(&task, &role, req)? else {
            return Ok(None);
        };
        let report = self.drive_chain(req, &policy, &started)?;
        self.settle_open_row(&started, report.gate)?;
        self.persist_outcome(&report, req)?;
        Ok(Some(report))
    }

    /// DRAFT → ANALYSIS is bookkeeping, not a role execution (the same step the
    /// inline engine took): the runner performs it before it can open a stage
    /// row. The address comes from the declaration like every other move.
    fn draft_booking(&self, req: &SupervisedRunReq) -> ApiResult<Task> {
        let task = self.store.resolve_task(&req.task_id)?;
        if task.state != TaskState::Draft {
            return Ok(task);
        }
        let process = self.process_policy()?;
        let m = workflow::start(&process, &task, &self.actor, Utc::now())?;
        self.store.commit(task.lock_version, &m)?;
        Ok(self.store.get_task(&task.id.to_string())?)
    }

    /// Open the row THIS process owns and hand the caller the handle — only
    /// now, when a live row exists (REQ-AR-08). A second chain is refused with
    /// the first execution's id and the first run is not touched (REQ-AR-06).
    fn open_owned_execution(
        &self,
        task: &Task,
        role: &str,
        req: &SupervisedRunReq,
    ) -> ApiResult<Option<ExecStarted>> {
        let Some(started) = self.claim_execution_row(task, role, req)? else {
            return Ok(None);
        };
        self.hand_over_handle(req, &started)?;
        self.record_launch_event(task, role, req, &started)?;
        Ok(Some(started))
    }

    /// The store's single-writer index is the enforcement point; its refusal
    /// already names the owning execution, so it becomes the caller's answer.
    fn claim_execution_row(
        &self,
        task: &Task,
        role: &str,
        req: &SupervisedRunReq,
    ) -> ApiResult<Option<ExecStarted>> {
        match self.exec_start_in(
            ExecStartReq {
                id: task.id.to_string(),
                role: role.to_string(),
                agent: (!req.agent.is_empty()).then(|| req.agent.clone()),
                inputs: Vec::new(),
                run_id: Some(req.run_id.clone()),
            },
            req.workspace.as_deref(),
        ) {
            Ok(s) => Ok(Some(s)),
            Err(ApiError::Store(StoreError::SingleWriterBusy { existing_id, .. })) => {
                let _ = launch::write_ack(&req.ack, &format!("busy {} {existing_id}", req.run_id));
                Ok(None)
            }
            Err(e) => Err(e),
        }
    }

    /// Drive the chain on the row this process opened (adoption), NOT a second
    /// one. An engine failure never leaves the row RUNNING: it is closed with
    /// the platform class and the declaration's park rule decides what follows
    /// (REQ-AR-05/09).
    fn drive_chain(
        &self,
        req: &SupervisedRunReq,
        policy: &ProcessPolicy,
        started: &ExecStarted,
    ) -> ApiResult<RunReport> {
        let mut engine = Engine::new(&self.store)
            .with_adopted_execution(started.execution.clone())
            .with_force_reaudit(req.force_reaudit);
        if let Some(w) = req.workspace.as_deref() {
            engine = engine.with_workspace(w);
        }
        match engine.advance(&req.task_id, &req.agent, &self.actor) {
            Ok(report) => Ok(report),
            Err(e) => {
                self.close_failed_run(started, policy, &e)?;
                Err(e.into())
            }
        }
    }

    /// The process declaration in force (REQ-AR-09/16): one loader, the same
    /// one the engine and `devai init` use. A broken declaration is a typed
    /// refusal here, never an implicit fallback. `pub(crate)`: the read doors
    /// in `lib.rs` (the QA gate, the active snapshot) ask the SAME declaration
    /// — the one role/stage authority (rework R1 of task 01a1b7dc).
    pub(crate) fn process_policy(&self) -> ApiResult<ProcessPolicy> {
        devai_store::config::load_process_policy(&self.config_dir().join("workflow"))
            .map_err(ApiError::from)
    }

    /// REQ-PK-03: the declared words of the stop surface — one load per read,
    /// the same merge-scan the other workflow policies use. A broken or
    /// incomplete declaration is a LOUD config error, never a silent default.
    pub(crate) fn park_reasons(&self) -> ApiResult<devai_store::config::ParkReasons> {
        devai_store::config::load_park_reasons(&self.config_dir().join("workflow"))
            .map_err(ApiError::from)
    }

    /// REQ-AR-11/12: a state that the declaration says a role executes REQUIRES
    /// a run — after a human decision the platform starts it itself, without a
    /// second command. Called after every committed workflow mutation; a state
    /// executed by a human or by nobody launches nothing.
    ///
    /// A failed auto-launch never fails the human act that triggered it: the
    /// rejection/approval is already stored, and the failure is recorded by
    /// `task_run` as a launch event with its counter (REQ-AR-13).
    fn autostart_if_role_state(&self, task: &Task) {
        if !self.autolaunch {
            return;
        }
        let Ok(policy) = self.process_policy() else {
            return;
        };
        if policy.role_for(task.state.as_str()).is_none() {
            return;
        }
        let _ = self.task_run(RunReq {
            id: task.id.to_string(),
            agent: String::new(),
            workspace: None,
            force_reaudit: false,
        });
    }

    /// The quality report a decision gate presents: the stored human form when
    /// present, the §2.6 text otherwise — the same status-aware selection
    /// (REQ-005, REQ-QH-05/06) the UI panel reads.
    fn presented_for_gate(&self, report: &RunReport) -> ApiResult<Option<PresentedReport>> {
        match report.gate {
            Gate::WaitingAccept | Gate::ReportFresh => {
                self.presented_quality_report(report.task_id)
            }
            _ => Ok(None),
        }
    }

    /// The paragraph half of the gate presentation (REQ-HP-05, ratified
    /// OQ-HP-1): approval/review reads spec + review, acceptance reads the
    /// implementation-report. Stored fields, verbatim; absence (a legacy @1/@2
    /// CURRENT, a non-JSON body) renders as NO paragraph, never as a broken
    /// gate (REQ-HP-06 non-retroactivity in the render half too).
    fn gate_human_paragraphs(&self, task: Id, gate: Gate) -> ApiResult<Vec<(String, String)>> {
        let mut paragraphs: Vec<(String, String)> = Vec::new();
        let wanted: &[(ArtifactKind, &str)] = match gate {
            Gate::WaitingApproval => &[
                (ArtifactKind::RequirementsSpec, "requirements-specification"),
                (ArtifactKind::ReviewReport, "review-report"),
            ],
            Gate::WaitingAccept | Gate::ReportFresh => {
                &[(ArtifactKind::ImplementationReport, "implementation-report")]
            }
            _ => &[],
        };
        for (kind, label) in wanted {
            if let Some(p) = self.stored_human_paragraph(task, *kind, label)? {
                paragraphs.push(p);
            }
        }
        Ok(paragraphs)
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
        let spec_name = self.current_spec_human_name(&after)?;
        let process = self.process_policy()?;
        let m = workflow::requirements_published(
            &process,
            &task,
            &after,
            req.reason.as_deref(),
            spec_name.as_deref(),
            &self.actor,
            Utc::now(),
        )?;
        self.store.commit(task.lock_version, &m)?;
        // REQ-AR-11: publishing a revised requirements spec lands in REVIEW —
        // a role state, so the platform starts the reviewer itself.
        self.autostart_if_role_state(&m.task);
        Ok(ReviseOutcome {
            task: m.task.clone(),
            staled_ids: m
                .artifact_status
                .iter()
                .filter(|(_, s)| *s == ArtifactStatus::Stale)
                .map(|(i, _)| i.to_string())
                .collect(),
            allowed_operations: workflow::allowed_ops(&process, &m.task),
        })
    }

    // ---- artifacts ---------------------------------------------------------

    /// Publish the artifact row through the contract gate — WITHOUT the
    /// workflow advance. Exposed beside `artifact_publish` so a consumer
    /// that renders between the two effects (the CLI prints the artifact
    /// row before a failing advance aborts) keeps byte parity (REQ-MUT-06).
    pub fn artifact_publish_row(&self, req: PublishReq) -> ApiResult<PublishedRow> {
        let task = self.store.resolve_task(&req.id)?;
        let depends_ids = self.publication_dependencies(&task, &req)?;
        let execution_id = self.publication_execution(req.execution.as_deref())?;
        let created_by = execution_id.map(|id| format!("execution:{}", id));
        // The contract reference parses AFTER the execution check — the
        // exact position the pre-cycle-2 CLI held (error-order parity).
        let contract = match req.contract.as_deref() {
            Some(c) => Some(ContractRef::parse(c).map_err(devai_core::CoreError::from)?),
            None => None,
        };
        // Отчёт проверки называет ПРОВЕРЕННУЮ версию спеки: пара
        // «спека vN → проверка → вердикт» иначе не восстановима (review-report@4).
        // Ставит платформа: роль не может знать seq будущего артефакта.
        let content = if req.kind == ArtifactKind::ReviewReport {
            self.stamp_review_spec(task.id, req.content)?
        } else {
            req.content
        };
        let artifact = self.publish_row_internal(
            task.id,
            req.kind,
            &req.name,
            content,
            contract,
            execution_id,
            depends_ids,
            created_by.as_deref(),
            req.category,
        )?;
        Ok(PublishedRow { artifact, task })
    }

    /// Put the reviewed specification into a REVIEW_REPORT body: `spec_seq`
    /// and `spec_artifact` of the task's CURRENT spec. Absent spec → the body
    /// is published exactly as written (nothing is invented). A body that is
    /// not one JSON object is refused rather than silently left unstamped.
    fn stamp_review_spec(&self, task_id: Id, content: Vec<u8>) -> ApiResult<Vec<u8>> {
        let spec = self.store.artifacts(task_id)?.into_iter().find(|a| {
            a.kind == ArtifactKind::RequirementsSpec && a.status == ArtifactStatus::Current
        });
        let Some(spec) = spec else {
            return Ok(content);
        };
        let mut body: serde_json::Value = serde_json::from_slice(&content).map_err(|e| {
            ApiError::refused(
                "malformed_body",
                format!("review report is not valid JSON, cannot name the spec: {e}"),
            )
        })?;
        let Some(obj) = body.as_object_mut() else {
            return Err(ApiError::refused(
                "malformed_body",
                "review report must be one JSON object — nothing to stamp".to_string(),
            ));
        };
        obj.insert("spec_seq".to_string(), serde_json::json!(spec.seq));
        obj.insert(
            "spec_artifact".to_string(),
            serde_json::json!(spec.id.to_string()),
        );
        serde_json::to_vec_pretty(&body).map_err(|e| {
            ApiError::refused(
                "malformed_body",
                format!("cannot serialize stamped review report: {e}"),
            )
        })
    }

    /// The explicit dependencies of a publication, plus the implicit one:
    /// downward role results are children of the CURRENT approved
    /// specification (a later requirements revision marks them STALE through
    /// the dependency graph — unless deps were given).
    fn publication_dependencies(&self, task: &Task, req: &PublishReq) -> ApiResult<Vec<Id>> {
        let mut depends_ids: Vec<Id> = Vec::new();
        for p in &req.depends {
            depends_ids.push(self.store.get_artifact(p)?.id);
        }
        if req.depends.is_empty() && req.kind.is_downstream_of_spec() {
            let spec = self
                .store
                .artifacts(task.id)?
                .iter()
                .find(|a| {
                    a.kind == ArtifactKind::RequirementsSpec && a.status == ArtifactStatus::Current
                })
                .map(|a| a.id);
            depends_ids.extend(spec);
        }
        Ok(depends_ids)
    }

    /// The producing execution of a publication. Ratified OQ-4: publication
    /// into a closed row is a LOUD refusal — «зомби не пишет в закрытую
    /// строку».
    fn publication_execution(&self, execution: Option<&str>) -> ApiResult<Option<Id>> {
        let Some(e) = execution else {
            return Ok(None);
        };
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
        Ok(Some(ex.id))
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
            ArtifactKind::StatementReview => self.advance_statement(&task, &a),
            ArtifactKind::RequirementsSpec => self.advance_spec(&task),
            ArtifactKind::ReviewReport => self.advance_review(&task, req.verdict.as_deref()),
            ArtifactKind::ImplementationReport => self.advance_implementation(&task, &a),
            // Fail-closed (QA R5): the kind carries no advance. The refusal
            // lists the advancing kinds DERIVED from the core predicate —
            // never a hand-copied set that could drift (QA R4).
            other => Err(not_advancing_refusal(other)),
        }
    }

    /// Test support (doc-hidden): a statement-review VERDICT recorded and
    /// advanced, so a fixture can reach ANALYSIS without running the checker
    /// role. Production publishes the same artifact from the role's reply —
    /// this is the same command path, not a shortcut around a gate.
    #[doc(hidden)]
    pub fn statement_approve_for_tests(&self, id: &str) -> ApiResult<()> {
        let task = self.store.resolve_task(id)?;
        if task.state != TaskState::Statement {
            return Ok(());
        }
        let body = r#"{"verdict":"APPROVED","human_paragraph":"statement checked (test fixture)"}"#
            .as_bytes();
        let published = self.artifact_publish(PublishReq {
            id: task.id.to_string(),
            kind: ArtifactKind::StatementReview,
            name: "statement-review.json".into(),
            content: body.to_vec(),
            depends: vec![],
            contract: None,
            execution: None,
            category: DataCategory::Artifact,
            verdict: None,
        })?;
        // The compound publish may already have walked the edge; the second
        // call is then a no-op for the fixture, never an error.
        let _ = self.artifact_advance(AdvanceReq {
            id: task.id.to_string(),
            artifact_id: published.publish.artifact.id.to_string(),
            kind: ArtifactKind::StatementReview,
            verdict: None,
        });
        Ok(())
    }

    /// STATEMENT_REVIEW advance: the checker's verdict opens the analysis.
    fn advance_statement(&self, task: &Task, a: &Artifact) -> ApiResult<AdvanceOutcome> {
        // The checker may refuse to open the analysis: his verdict says
        // QUESTIONS, and then the questions — not the transition — are what
        // the human owes. A verdict the schema does not name falls through to
        // the approve edge and is refused by the core, never guessed here.
        let raw = self.store.read_artifact_body(a)?;
        let body: serde_json::Value =
            serde_json::from_slice(&raw).unwrap_or(serde_json::Value::Null);
        let verdict = body
            .get("verdict")
            .and_then(|v| v.as_str())
            .unwrap_or_default();
        if verdict.eq_ignore_ascii_case("QUESTIONS") {
            let text = workflow::statement_questions_text(&body);
            let q = Question {
                id: new_id(),
                task_id: task.id,
                execution_id: a.execution_id,
                text,
                context: body
                    .get("human_paragraph")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string()),
                options: vec![],
                blocking: true,
                status: QuestionStatus::Open,
                answer: None,
                answered_by: None,
                answered_at: None,
                created_at: Utc::now(),
            };
            self.store.create_question(&q)?;
            let process = self.process_policy()?;
            let m = workflow::raise_question(&process, task, q.id, true, &self.actor, Utc::now())?;
            self.store.commit(task.lock_version, &m)?;
            return Ok(AdvanceOutcome {
                from: task.state,
                to: m.task.state,
                rework_gate: false,
            });
        }
        let process = self.process_policy()?;
        let m = workflow::statement_approved(&process, task, &self.actor, Utc::now())?;
        self.store.commit(task.lock_version, &m)?;
        self.autostart_if_role_state(&m.task);
        Ok(AdvanceOutcome {
            from: task.state,
            to: m.task.state,
            rework_gate: false,
        })
    }

    /// REQUIREMENTS_SPEC advance: the core `requirements_published` command
    /// over the task's stored artifacts.
    fn advance_spec(&self, task: &Task) -> ApiResult<AdvanceOutcome> {
        let arts = self.store.artifacts(task.id)?;
        let name = self.current_spec_human_name(&arts)?;
        let process = self.process_policy()?;
        let m = workflow::requirements_published(
            &process,
            task,
            &arts,
            None,
            name.as_deref(),
            &self.actor,
            Utc::now(),
        )?;
        self.store.commit(task.lock_version, &m)?;
        Ok(AdvanceOutcome {
            from: task.state,
            to: m.task.state,
            rework_gate: false,
        })
    }

    /// REVIEW_REPORT advance: the verdict is required and parsed before the
    /// core command runs.
    fn advance_review(&self, task: &Task, verdict: Option<&str>) -> ApiResult<AdvanceOutcome> {
        let Some(s) = verdict else {
            return Err(ApiError::refused(
                "bad_request",
                "publishing REVIEW_REPORT requires --verdict approved|needs_revision",
            ));
        };
        let v = Self::parse_verdict(s)?;
        let process = self.process_policy()?;
        // Отказ ревью не запускает следующий круг сам: задача паркуется с
        // вопросом человеку (решение оператора 12.11).
        let m = if v == Verdict::Approved {
            workflow::review_recorded(&process, task, v, None, &self.actor, Utc::now())?
        } else {
            workflow::park_review_rejection(
                &process,
                task,
                new_id(),
                v,
                None,
                &self.actor,
                Utc::now(),
            )?
        };
        self.store.commit(task.lock_version, &m)?;
        Ok(AdvanceOutcome {
            from: task.state,
            to: m.task.state,
            rework_gate: false,
        })
    }

    /// IMPLEMENTATION_REPORT advance: the manual publication crosses the SAME
    /// core command as the engine — including the empty-rework gate.
    fn advance_implementation(&self, task: &Task, a: &Artifact) -> ApiResult<AdvanceOutcome> {
        let arts = self.store.artifacts(task.id)?;
        let facts = devai_runtime::gate::collect_implementation_report_facts(
            &self.store,
            task.id,
            a,
            &arts,
            None,
        )?;
        let process = self.process_policy()?;
        let m = workflow::implementation_reported(&process, task, &facts, &self.actor, Utc::now())?;
        self.store.commit(task.lock_version, &m)?;
        Ok(AdvanceOutcome {
            from: task.state,
            to: m.task.state,
            rework_gate: m.task.state == TaskState::WaitingForInput,
        })
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
        // Потолок спеки (решение оператора 09.11): промпт роли передаётся роли
        // ОДНИМ аргументом командной строки, а у Linux на аргумент лимит 128 КБ.
        // Спека в 57 КБ + профиль + ревью + отчёт превышали его и роняли запуск
        // агента («Argument list too long»). Жёсткий предел держит и смысл в
        // разумных рамках: раздутая спека и есть источник противоречий.
        spec_size_guard(kind, content.len())?;
        if let Some(c) = &contract {
            self.contract_gate(c, &content)?;
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

    /// The manual publication's contract gate (ratified OQ-FG-05, REQ-FG-08):
    /// an unregistered contract is a typed refusal; a registered one runs the
    /// SAME gate the engine loop runs, so enforcement is the API's and the
    /// presentation (warning + full list) the consumer's. REQ-HP-02 closes
    /// the last door: the paragraph halves (presence via `required`,
    /// edibility via the deterministic detector) live in this one call, and
    /// it reads the project's loaded `humanize.yaml` fresh — config stays the
    /// source of truth, a broken template is a LOUD refusal, never a silent
    /// factory fallback on the manual path.
    /// REQ-TN (ratified OQ-TN-1): set or replace the human signboard of a
    /// task DIRECTLY — the human door to the name, without a rework cycle
    /// (the morning the operator renamed eleven active tasks by hand proved
    /// the need). Mirrors `task.project_set`: refs resolve first, the name
    /// is validated against the SAME declarative window the publication
    /// gate uses (REQ-TN-05 AC1 «и на ручном публикации» — one rule, two
    /// doors), a stale `expected_lock` refuses typed `conflict`, an
    /// effective change writes row + `task.human_name.set` event in one
    /// transaction. Re-applying the name the task already carries is a
    /// side-effect-free NO-OP (`changed=false`), like the project binding.
    /// The workflow state is never touched — a rename signs, it does not
    /// re-open.
    pub fn task_rename(&self, req: TaskRenameReq) -> ApiResult<TaskRenameOutcome> {
        let task = self.store.resolve_task(&req.id)?;
        if req.expected_lock.is_some_and(|l| l != task.lock_version) {
            return Err(StoreError::Conflict {
                what: format!("task {}", task.id),
                expected: req.expected_lock.unwrap(),
                found: task.lock_version,
            }
            .into());
        }
        let policy = self.task_name_policy()?;
        let name = devai_runtime::taskname::validate_human_name(&req.name, &policy).map_err(
            |violations| {
                ApiError::refused(
                    "contract_violation",
                    format!(
                        "the human task name violates config/workflow/task-name.yaml: {}",
                        violations.join("; ")
                    ),
                )
            },
        )?;
        if task.human_name.as_deref() == Some(name.as_str()) {
            return Ok(TaskRenameOutcome {
                task,
                human_name: name,
                changed: false,
            });
        }
        let updated =
            self.store
                .set_task_human_name(&task, Some(name.as_str()), &self.actor, Utc::now())?;
        Ok(TaskRenameOutcome {
            human_name: name,
            task: updated,
            changed: true,
        })
    }

    /// The typed name read of the CURRENT spec (QA R1, rel/error-context;
    /// body read, QA R2: this is the private unit the advance dispatches
    /// to, not inline facade machinery). The blob is the content-addressed
    /// body of the row just published, read with the template's field. A
    /// CURRENT spec blob that does not read back, or reads back as
    /// non-JSON, is a store-integrity failure — the advance refuses LOUD
    /// (`spec_body_unreadable`) instead of folding the failure into «body
    /// without a name» and silently keeping the previous signboard.
    /// «Readable but name-less» (the @1..@3 legacy shape) stays the ONE
    /// legal absence branch: such a publication says nothing about the
    /// signboard (REQ-TN-02 AC3).
    fn current_spec_human_name(&self, arts: &[Artifact]) -> ApiResult<Option<String>> {
        let field = self.task_name_policy()?.field;
        let Some(x) = arts
            .iter()
            .filter(|x| {
                x.kind == ArtifactKind::RequirementsSpec && x.status == ArtifactStatus::Current
            })
            .max_by_key(|x| x.seq)
        else {
            // No CURRENT spec at all: `requirements_published` keeps its
            // own guard and refuses loudly at the transition below.
            return Ok(None);
        };
        let body = self.store.read_blob(&x.sha256).map_err(|e| {
            ApiError::refused(
                "spec_body_unreadable",
                format!(
                    "CURRENT spec {} blob {} does not read back: {e} — the advance refuses instead of silently keeping the stored signboard; repair the store or republish the specification",
                    x.id, x.sha256
                ),
            )
        })?;
        match workflow::read_human_name_from_spec(&body, &field) {
            workflow::SpecNameRead::Named(v) => Ok(Some(v)),
            workflow::SpecNameRead::Absent => Ok(None),
            workflow::SpecNameRead::Corrupt => Err(ApiError::refused(
                "spec_body_unreadable",
                format!(
                    "CURRENT spec {} blob {} reads back as bytes that are not parseable JSON — the advance refuses instead of silently keeping the stored signboard; the row and its blob are inconsistent, republish the specification",
                    x.id, x.sha256
                ),
            )),
        }
    }

    /// The project's task-name template, resolved exactly the way the
    /// humanization one is (REQ-TN-05 AC2): loaded per call, so a window
    /// edit lands on the very next gate without a rebuild, and a broken
    /// file refuses LOUD — the manual paths never silently fall back to
    /// the factory while the YAML claims otherwise.
    fn task_name_policy(&self) -> ApiResult<devai_store::config::TaskNamePolicy> {
        let cfg_dir = std::env::var_os("DEVAI_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| self.store.root().join("config"));
        Ok(devai_store::config::load_task_name_policy(
            &cfg_dir.join("workflow"),
        )?)
    }

    fn contract_gate(&self, c: &ContractRef, content: &[u8]) -> ApiResult<()> {
        let schema = self.store.get_contract(&c.name, c.version)?;
        let Some(schema) = schema else {
            return Err(ApiError::ContractUnregistered {
                contract: c.to_string(),
            });
        };
        let humanize = self.humanize_policy()?;
        let task_name = self.task_name_policy()?;
        let violations = devai_runtime::artifact_violations(
            String::from_utf8_lossy(content).as_ref(),
            Some(schema.as_str()),
            &humanize,
            &task_name,
        )?;
        if violations.is_empty() {
            return Ok(());
        }
        Err(ApiError::ContractRejected {
            contract: c.to_string(),
            violations,
        })
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
        let task = self.store.resolve_task(id)?;
        let blockers = self.override_blockers(&task, &blocker)?;
        self.record_override(&task, &blockers, rationale)?;
        Ok(OverrideResult {
            task,
            override_blockers: blockers,
        })
    }

    /// The blocker set an override covers, as FULL task ids: the explicit
    /// refs, else exactly the blockers OPEN right now (computed from stored
    /// data, so a NEW blocker is never covered by an old decision).
    fn override_blockers(&self, task: &Task, requested: &[String]) -> ApiResult<Vec<String>> {
        if !requested.is_empty() {
            let mut ids: Vec<String> = Vec::new();
            for b in requested {
                ids.push(self.store.resolve_task(b)?.id.to_string());
            }
            return Ok(ids);
        }
        let deps = self.store.task_dependencies(task.id)?;
        let blockers = workflow::open_blockers(task, &deps);
        if blockers.is_empty() {
            return Err(ApiError::refused(
                "bad_request",
                format!(
                    "task {} has no open blocking dependencies — nothing to override",
                    short_id(&task.id)
                ),
            ));
        }
        Ok(blockers)
    }

    /// The stored decision row plus the append-only event of one override.
    fn record_override(
        &self,
        task: &Task,
        blockers: &[String],
        rationale: Option<String>,
    ) -> ApiResult<()> {
        self.store.record_decision(&Decision {
            id: new_id(),
            task_id: task.id,
            kind: decision_kind::DEPENDENCY_OVERRIDE.into(),
            verdict: Verdict::Approved,
            actor: self.actor.clone(),
            rationale,
            refs: blockers.to_vec(),
            created_at: Utc::now(),
            payload: serde_json::Value::Null,
        })?;
        self.store.insert_event(
            Some(task.id),
            event_kind::TASK_DEPENDENCY_OVERRIDDEN,
            &self.actor,
            serde_json::json!({ "blockers": blockers }),
        )?;
        Ok(())
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
            payload: serde_json::Value::Null,
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
        // REQ-AR-11: the ratified passage lands in QA — a role state, so the
        // auditor's run starts here, without a second command.
        self.autostart_if_role_state(&m.task);
        Ok(RatifyResult {
            task: cur,
            decision,
            recorded_now,
            prev_report,
            current_report: current_id_s,
            state: m.task.state,
            allowed_operations: workflow::allowed_ops(&self.process_policy()?, &m.task),
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
                    payload: serde_json::Value::Null,
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
            let process = self.process_policy()?;
            let m = workflow::answer_question(&process, &cur, q.id, &self.actor, Utc::now())?;
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
        let process = self.process_policy()?;
        let m = workflow::implementation_reported(&process, cur, &facts, &self.actor, Utc::now())?;
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
        let process = self.process_policy()?;
        let m =
            workflow::raise_question(&process, &task, q.id, q.blocking, &self.actor, Utc::now())?;
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
        // REQ-AL-06: the answer to the attempt-limit question is the ONLY
        // reset of the counter. It is read from the STORED trip event (not
        // from the question text), resolved to one of the two stored options,
        // recorded as a decision row and acted on — «продолжить» resumes the
        // remembered stage, «остановить» leaves the task in human custody.
        if let Some(trip) = self.attempt_limit_trip(&task, q.id)? {
            return self.answer_attempt_limit(&q, &task, text, &trip);
        }
        // Вопрос «начинать ли новый круг после отказа ревью»: «остановить»
        // уводит дело в удержание у человека, «продолжить» идёт обычным путём
        // (снять удержание, вернуть в ANALYSIS, автозапуск).
        if self.review_return_gate(&task, q.id)? {
            if attempt_limit_answer::parse(text) == Some(attempt_limit_answer::STOP) {
                let process = self.process_policy()?;
                let m = workflow::answer_review_return_stop(
                    &process,
                    &task,
                    q.id,
                    &self.actor,
                    Utc::now(),
                )?;
                self.store
                    .answer_question(q.id, text, &self.actor, Utc::now())?;
                self.store.commit(task.lock_version, &m)?;
                return Ok(AnsweredOutcome {
                    question_id: q.id,
                    from_state: task.state,
                    to_state: m.task.state,
                });
            }
        }
        self.store
            .answer_question(q.id, text, &self.actor, Utc::now())?;
        let process = self.process_policy()?;
        let mut m = workflow::answer_question(&process, &task, q.id, &self.actor, Utc::now())?;
        // Ответ на вопрос ПОСТАНОВКИ становится частью постановки: иначе
        // проверяющий снова видит тот же текст и спрашивает то же самое
        // (прецедент 01a1df87 — пять одинаковых кругов по одному сценарию).
        Self::absorb_answer_into_statement(&mut m, &task, &q, text);
        self.store.commit(task.lock_version, &m)?;
        // REQ-AR-11: the answer releases the hold and returns the task to the
        // stage that asked — a role state, so the platform starts that run
        // itself (no second command).
        self.autostart_if_role_state(&m.task);
        Ok(AnsweredOutcome {
            question_id: q.id,
            from_state: task.state,
            to_state: m.task.state,
        })
    }

    /// Ответ человека на вопрос, поднятый из постановки, ДОПИСЫВАЕТСЯ в текст
    /// постановки: вопрос задан про этот текст, ответ на него — уточнение этого
    /// текста. Так следующий круг судит полную постановку, а не ту же самую
    /// с пропущенными ответами. Повтор того же ответа не дублируется.
    fn absorb_answer_into_statement(m: &mut Mutation, task: &Task, q: &Question, text: &str) {
        if task.return_state != Some(TaskState::Statement) {
            return;
        }
        let answer = text.trim();
        if answer.is_empty() || m.task.business_description.contains(answer) {
            return;
        }
        const HEAD: &str = "\n\n— уточнения человека (ответы на вопросы постановки):\n";
        let mut body = m.task.business_description.clone();
        if !body.contains(HEAD.trim()) {
            body.push_str(HEAD);
        }
        body.push_str(&format!("• {answer}\n"));
        m.task.business_description = body;
        m.events.push(EventDraft {
            kind: devai_core::types::event_kind::TASK_RESTATED,
            actor: "platform".to_string(),
            payload: serde_json::json!({
                "reason": "ответ человека на вопрос постановки вписан в постановку",
                "question_id": q.id.to_string(),
            }),
        });
    }

    /// Поднят ли `question_id` гейтом «ревью вернуло спеку» — по СОБЫТИЮ, а не
    /// по тексту вопроса: ответ читается из данных, не из формулировки.
    fn review_return_gate(&self, task: &Task, question_id: Id) -> ApiResult<bool> {
        let qid = question_id.to_string();
        Ok(self
            .store
            .events(task.id, None)?
            .into_iter()
            .rev()
            .any(|ev| {
                ev.kind == event_kind::QUESTION_RAISED
                    && ev.payload.get(json_field::GATE).and_then(|v| v.as_str())
                        == Some(workflow::GATE_REVIEW_RETURN)
                    && ev
                        .payload
                        .get(json_field::QUESTION_ID)
                        .and_then(|v| v.as_str())
                        == Some(qid.as_str())
            }))
    }

    /// The stored attempt-limit trip that raised `question_id`, if any: the
    /// event carries role, count and limit, so the answer path needs no
    /// parsing of free text (REQ-AL-05/06).
    fn attempt_limit_trip(
        &self,
        task: &Task,
        question_id: Id,
    ) -> ApiResult<Option<serde_json::Value>> {
        let qid = question_id.to_string();
        Ok(self
            .store
            .events(task.id, None)?
            .into_iter()
            .rev()
            .find(|ev| {
                ev.kind == event_kind::ATTEMPT_LIMIT_TRIPPED
                    && ev
                        .payload
                        .get(json_field::QUESTION_ID)
                        .and_then(|v| v.as_str())
                        == Some(qid.as_str())
            })
            .map(|ev| ev.payload))
    }

    /// Apply the human answer to the attempt-limit stop (REQ-AL-06). Both
    /// answers write a decision row (who, when, role, attempts, limit, answer)
    /// and an append-only event; only «продолжить» is a reset boundary.
    fn answer_attempt_limit(
        &self,
        q: &Question,
        task: &Task,
        text: &str,
        trip: &serde_json::Value,
    ) -> ApiResult<AnsweredOutcome> {
        // The accepted words and the question's stored options are ONE
        // dictionary (`attempt_limit_answer`): the answer is data, not a
        // sentence the operator has to phrase perfectly.
        let Some(choice) = attempt_limit_answer::parse(text) else {
            let stored = attempt_limit_answer::ALL
                .iter()
                .map(|option| format!("`{option}`"))
                .collect::<Vec<_>>()
                .join(" or ");
            return Err(ApiError::refused(
                "bad_request",
                format!(
                    "question {} takes one of its stored options: {stored} (got `{}`)",
                    q.id, text
                ),
            ));
        };
        // A corrupt trip event must NOT be read as an empty role and zero
        // attempts: the recorded decision would then lie (rework RL/R4).
        let trip = AttemptTrip::parse(trip)?;
        let now = Utc::now();
        let decision = self.attempt_limit_decision(task, q, &trip, choice, now);
        self.store.answer_question(q.id, text, &self.actor, now)?;
        let process = self.process_policy()?;
        let m = self.attempt_limit_mutation(&process, task, q, &trip, choice, &decision, now)?;
        self.store.record_decision(&decision)?;
        self.store.commit(task.lock_version, &m)?;
        if choice == attempt_limit_answer::CONTINUE {
            // The human said «продолжить» — the interrupted stage starts
            // again (REQ-AL-06 AC1: no second command).
            self.autostart_if_role_state(&m.task);
        }
        Ok(AnsweredOutcome {
            question_id: q.id,
            from_state: task.state,
            to_state: m.task.state,
        })
    }

    /// The decision row the answer leaves behind: who, when, role, attempts,
    /// limit and the answer — one stored fact, no free text.
    fn attempt_limit_decision(
        &self,
        task: &Task,
        q: &Question,
        trip: &AttemptTrip,
        choice: &str,
        now: DateTime<Utc>,
    ) -> Decision {
        Decision {
            id: new_id(),
            task_id: task.id,
            kind: decision_kind::ATTEMPT_LIMIT_ANSWER.to_string(),
            verdict: Verdict::Approved,
            actor: self.actor.clone(),
            rationale: None,
            refs: vec![q.id.to_string()],
            created_at: now,
            payload: serde_json::json!({
                (json_field::ROLE): &trip.role,
                (json_field::ATTEMPT): trip.attempts,
                (json_field::LIMIT): trip.limit,
                (json_field::ANSWER): choice,
            }),
        }
    }

    /// The state mutation ONE answer produces: «продолжить» releases the hold
    /// and writes the append-only reset event, «остановить» moves the task to
    /// the declared human-custody hold keeping its remembered stage.
    fn attempt_limit_mutation(
        &self,
        process: &ProcessPolicy,
        task: &Task,
        q: &Question,
        trip: &AttemptTrip,
        choice: &str,
        decision: &Decision,
        now: DateTime<Utc>,
    ) -> ApiResult<Mutation> {
        if choice != attempt_limit_answer::CONTINUE {
            return Ok(workflow::answer_attempt_limit_stop(
                process,
                task,
                q.id,
                &trip.role,
                trip.attempts,
                trip.limit,
                &self.actor,
                now,
            )?);
        }
        let mut m = workflow::answer_question(process, task, q.id, &self.actor, now)?;
        m.events.push(EventDraft {
            kind: event_kind::ATTEMPT_LIMIT_RESET,
            actor: self.actor.clone(),
            payload: serde_json::json!({
                (json_field::QUESTION_ID): q.id.to_string(),
                (json_field::ROLE): &trip.role,
                (json_field::ATTEMPT): trip.attempts,
                (json_field::LIMIT): trip.limit,
                (json_field::ANSWER): choice,
                (json_field::DECISION): decision.id.to_string(),
            }),
        });
        Ok(m)
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
            reasoning: String::new(),
            created_at: Utc::now(),
            active: false,
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
        self.exec_start_in(req, None)
    }

    /// The same start with the RUN's workspace known: a detached runner was
    /// launched with `--workspace`, so the revision stamped on the row must be
    /// THAT workspace's (REQ-001/REQ-006: platform-side, not the caller's cwd).
    fn exec_start_in(&self, req: ExecStartReq, workspace: Option<&Path>) -> ApiResult<ExecStarted> {
        let task = self.store.resolve_task(&req.id)?;
        // The declaration is the only role/stage authority (REQ-AR-09 AC1):
        // the guard reads who executes the state FROM process.yaml, so a home
        // that changes the executor changes who may start here.
        let process = self.process_policy()?;
        workflow::check_execution_allowed(&process, &req.role, &task)?;
        // REQ-SW-02 AC2: the labeled early refusal of a second RUNNING row
        // — before the index would reject it, in the old order (check,
        // then guard).
        self.guard_running_writer(&task)?;
        let version = self
            .store
            .get_role(&req.role)?
            .map(|r| r.version)
            .unwrap_or(1);
        let inputs = self.execution_inputs(&req.inputs)?;
        let e = self.new_execution_row(req, task.id, version, inputs, workspace)?;
        self.store.start_execution(&e)?;
        Ok(ExecStarted { execution: e, task })
    }

    /// The stored ids of the declared inputs, in the caller's order.
    fn execution_inputs(&self, inputs: &[String]) -> ApiResult<Vec<String>> {
        let mut ids: Vec<String> = Vec::new();
        for p in inputs {
            ids.push(self.store.get_artifact(p)?.id.to_string());
        }
        Ok(ids)
    }

    /// The RUNNING row a manual start opens: the workspace revision is
    /// computed platform-side (REQ-001/REQ-006 — never supplied by the
    /// caller) and the owner columns are captured platform-side (REQ-SW-03:
    /// this process is the one opening the row, so the orphan detect,
    /// REQ-SW-04, finds exactly that when it exits with the row RUNNING).
    fn new_execution_row(
        &self,
        req: ExecStartReq,
        task_id: Id,
        role_version: u32,
        inputs: Vec<String>,
        workspace: Option<&Path>,
    ) -> ApiResult<RoleExecution> {
        let revision = self.workspace_revision(workspace)?;
        let (owner_pid, owner_host, owner_process_start) = liveness::current_owner().into_columns();
        let run_id = Self::row_run_id(req.run_id.as_deref())?;
        let stamp = self.bundle_stamp(req.agent.as_ref())?;
        Ok(RoleExecution {
            id: new_id(),
            task_id,
            role_slug: req.role,
            role_version,
            agent_name: req.agent,
            status: ExecutionStatus::Running,
            inputs,
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
            bundle: stamp,
            run_id,
        })
    }

    /// The workspace revision the row is bound to: computed PLATFORM-side, in
    /// the workspace the run actually evaluates (REQ-001/REQ-006) — never
    /// supplied by the caller.
    fn workspace_revision(&self, workspace: Option<&Path>) -> ApiResult<WorkspaceRevision> {
        let engine = match workspace {
            Some(w) => Engine::new(&self.store).with_workspace(w),
            None => Engine::new(&self.store),
        };
        Ok(engine.current_revision()?)
    }

    /// REQ-AR-04/07: the run identity of this row. A detached runner hands in
    /// the number its caller minted (the whole chain shares it); a manual start
    /// IS its own run — its id is the run number.
    fn row_run_id(raw: Option<&str>) -> ApiResult<Option<Uuid>> {
        match raw {
            Some(raw) => Ok(Some(Uuid::parse_str(raw).map_err(|_| {
                ApiError::refused("invalid_input", format!("run_id `{raw}` is not a uuid"))
            })?)),
            None => Ok(None),
        }
    }

    /// REQ-MS-05: the run stamp, taken from the agent RECORD at start — the
    /// manual path reads the same row the engine does, so a manual row answers
    /// «what did this run use?» exactly like an engine row. No record → no
    /// stamp («связка не записана»), never a guessed identity.
    fn bundle_stamp(&self, agent: Option<&String>) -> ApiResult<Option<ExecutionBundle>> {
        Ok(self
            .store
            .list_agents()?
            .into_iter()
            .find(|a| Some(&a.name) == agent)
            .map(|a| ExecutionBundle::new(&a.provider, &a.runtime, &a.model, &a.reasoning)))
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

    /// Остановка прогона человеком. Строка закрывается с ПРИЧИНОЙ: кто и что
    /// сказал. Раньше она закрывалась пустой (`summary` и `error` — None), и
    /// карточка честно не могла показать ничего, кроме статуса (прецедент
    /// «только статус, а в консоли есть текст»).
    pub fn exec_cancel(&self, exec_id: &str, reason: Option<&str>) -> ApiResult<RoleExecution> {
        let why = reason.map(str::trim).filter(|r| !r.is_empty());
        let summary = match why {
            Some(r) => format!("остановлено человеком ({}) · причина: {r}", self.actor),
            None => format!("остановлено человеком ({}) · без причины", self.actor),
        };
        self.exec_close(exec_id, ExecutionStatus::Cancelled, Some(&summary), None)
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
        // Возвращаем строку КАК ОНА ЗАПИСАНА, а не снимок до записи: иначе
        // вызывающий видел старый статус и пустую причину.
        Ok(self.store.get_execution(&e.id.to_string())?)
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
        let blob = self.store.read_artifact_body(a)?;
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
        let events = self.store.events(task.id, None)?;
        let cause = present::stale_cause(&events, a.id);
        let freshness = present::freshness(task, a, &execs, cause.as_ref());
        let (body, text, source, first_layer) =
            match serde_json::from_slice::<serde_json::Value>(&blob) {
                Ok(body) => {
                    let rendered = present::render_stored_report(&body);
                    (body, rendered.text, rendered.source, rendered.first_layer)
                }
                Err(e) => (
                    serde_json::Value::Null,
                    present::corrupt_body_text(&short_id(&a.id), &e.to_string()),
                    devai_runtime::human_view::present::RenderSource::CorruptBody,
                    None,
                ),
            };
        Ok(Some(PresentedQuality {
            artifact: a.clone(),
            body,
            presented: PresentedReport {
                text,
                source,
                freshness,
                first_layer,
            },
        }))
    }
}

// ---------------------------------------------------------------------------
// profile rules ratification (task 01a1bacf, RV1..RV9)
// ---------------------------------------------------------------------------

/// `profile.ratify` — the HUMAN act that gives a project's rules edition
/// force. `items` is the explicit covered list; `all` covers every item of
/// the current edition (the decision still stores the full explicit list).
/// `rationale` is mandatory: a ratification is a stated human ground, never
/// a bare button.
#[derive(Debug, Clone)]
pub struct ProfileRatifyReq {
    pub task: String,
    pub project: String,
    pub items: Vec<String>,
    pub all: bool,
    pub rationale: Option<String>,
}

/// `profile.revoke` — the HUMAN act that takes an APPROVED ratification out
/// of force. The rules file is not touched (RV6).
#[derive(Debug, Clone)]
pub struct ProfileRevokeReq {
    pub decision: String,
    pub rationale: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProfileRatifyOutcome {
    pub task_id: String,
    pub decision: Decision,
    /// false when the SAME edition + item set already carried a live
    /// approval — the command then reports that decision instead of
    /// recording a second one (RV1 AC6).
    pub recorded_now: bool,
    pub project_id: String,
    pub file: String,
    pub edition: String,
    pub items: Vec<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ProfileRevokeOutcome {
    pub decision: Decision,
    pub revoked: Decision,
    pub project_id: String,
    pub file: String,
    pub edition: String,
    pub items: Vec<String>,
}

/// State of ONE item of a project's rules file (RV9).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProfileStateItem {
    pub target_id: String,
    pub in_force: bool,
    pub decision: Option<String>,
    pub actor: Option<String>,
    /// WHY an item is out of force, in the RV8 vocabulary.
    pub reason: Option<String>,
}

/// The read-only view of a project's rules state (RV9): the current edition
/// fingerprint and every item's force state. Nothing is written; the
/// fingerprint is computed the same way the ratification computes it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ProfileState {
    pub project_id: String,
    pub project_name: String,
    pub file: String,
    pub edition: String,
    pub items: Vec<ProfileStateItem>,
}

impl Api {
    /// Resolve the project and its rules file, refusing loudly when either
    /// does not exist (RV1 AC5 / RV7 AC2): the refusal names the project
    /// and the expected file.
    fn profile_rules_target(&self, project: &str) -> ApiResult<(Project, PathBuf)> {
        let p = self.store.get_project(project)?.ok_or_else(|| {
            ApiError::refused(
                "bad_request",
                format!(
                    "project `{project}` is not registered — a profile ratification names an EXISTING project"
                ),
            )
        })?;
        let path = devai_store::config::overlay_path(&self.config_dir(), &p.id.to_string());
        if !path.is_file() {
            return Err(ApiError::refused(
                "bad_request",
                format!(
                    "project `{}` has no rules file at {} — nothing to ratify (the file is the project rules overlay)",
                    p.name,
                    path.display()
                ),
            ));
        }
        Ok((p, path))
    }

    fn load_profile_overlay(
        &self,
        project: &Project,
        path: &Path,
    ) -> ApiResult<(String, devai_store::config::ProfileOverlay)> {
        let (edition, overlay) =
            devai_store::config::overlay_edition_at(&self.config_dir(), &project.id.to_string())?;
        if overlay.project_id != project.id.to_string() {
            return Err(ApiError::refused(
                "bad_request",
                format!(
                    "{}: header project_id `{}` does not belong to project `{}` ({}) — the file must be the project's own rules overlay",
                    path.display(),
                    overlay.project_id,
                    project.name,
                    project.id
                ),
            ));
        }
        Ok((edition, overlay))
    }

    pub fn profile_state(&self, project: &str) -> ApiResult<ProfileState> {
        let (p, path) = self.profile_rules_target(project)?;
        let (edition, overlay) = self.load_profile_overlay(&p, &path)?;
        let index = self.store.ratification_index()?;
        let forces = devai_store::config::item_forces(&overlay, &edition, &index);
        let mut items: Vec<ProfileStateItem> = overlay
            .items
            .iter()
            .map(|item| {
                let state = forces.get(&item.target_id).cloned().unwrap_or(
                    devai_store::config::ItemForce {
                        force: None,
                        reason: Some("нератифицирован".into()),
                    },
                );
                ProfileStateItem {
                    target_id: item.target_id.clone(),
                    in_force: state.force.is_some(),
                    decision: state.force.as_ref().map(|(d, _)| d.clone()),
                    actor: state.force.as_ref().map(|(_, a)| a.clone()),
                    reason: state.reason,
                }
            })
            .collect();
        items.sort_by(|a, b| a.target_id.cmp(&b.target_id));
        Ok(ProfileState {
            project_id: p.id.to_string(),
            project_name: p.name,
            file: devai_store::config::overlay_rel_path(&p.id.to_string()),
            edition,
            items,
        })
    }

    /// The mandatory human reason of ONE act (RV1/RV6): a single rule for both
    /// doors, so ratify and revoke cannot drift in strictness or wording.
    fn human_reason(rationale: Option<&str>, act: &str) -> ApiResult<String> {
        rationale
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .ok_or_else(|| {
                ApiError::refused(
                    "bad_request",
                    format!("`--rationale` is required — a {act} records WHY the operator took it"),
                )
            })
    }

    /// RV4 AC2: both acts are HUMAN-ONLY. The decision is taken HERE, from the
    /// session kind the door declared — not from the caller's text and not
    /// from the transport. A role/engine run may read the rules state, never
    /// ratify or revoke them, exactly like `task.ratify_empty_rework`.
    fn require_human_session(&self, act: &str) -> ApiResult<()> {
        if self.session == SessionKind::Human {
            return Ok(());
        }
        Err(ApiError::refused(
            "human_only",
            format!(
                "{act} is a HUMAN-ONLY act: the calling session is a role/engine run, not the operator's own process. No engine, role or agent path may create a profile_ratification decision — the operator runs it"
            ),
        ))
    }

    /// The item list a ratification covers (RV1 AC1/AC3/AC4): the explicit
    /// list, or EVERY item of the current edition — the decision stores the
    /// full list either way. The list is sorted and de-duplicated, so the
    /// same set always produces the same decision (the RV1 AC6 reuse key).
    fn ratification_items(
        req: &ProfileRatifyReq,
        project: &Project,
        overlay: &devai_store::config::ProfileOverlay,
    ) -> ApiResult<Vec<String>> {
        let mut items: Vec<String> = if req.all {
            overlay.items.iter().map(|i| i.target_id.clone()).collect()
        } else {
            Self::require_known_items(req, project, overlay)?;
            req.items.clone()
        };
        items.sort();
        items.dedup();
        if items.is_empty() {
            return Err(ApiError::refused(
                "bad_request",
                format!(
                    "project `{}` has no rules items — nothing to ratify",
                    project.name
                ),
            ));
        }
        Ok(items)
    }

    /// A ratification covers items of the CURRENT edition only (RV1 AC4): an
    /// unknown id refuses BY NAME, so nothing is ever ratified "for later".
    fn require_known_items(
        req: &ProfileRatifyReq,
        project: &Project,
        overlay: &devai_store::config::ProfileOverlay,
    ) -> ApiResult<()> {
        if req.items.is_empty() {
            return Err(ApiError::refused(
                "bad_request",
                "no items named — pass `--items id[,id...]` or `--all` so the decision states WHAT it covers",
            ));
        }
        for id in &req.items {
            if !overlay.items.iter().any(|i| &i.target_id == id) {
                return Err(ApiError::refused(
                    "bad_request",
                    format!(
                        "item `{id}` is not in the current edition of project `{}` — a ratification cannot cover an unknown or future item",
                        project.name
                    ),
                ));
            }
        }
        Ok(())
    }

    /// Record the ratification row, or return the live one for exactly
    /// (project, edition, item set) — the reuse key of RV1 AC6. One decision
    /// per act: a repeat never mints a second row.
    fn record_ratification(
        &self,
        task: Id,
        project_id: &str,
        file: &str,
        edition: &str,
        items: &[String],
        rationale: String,
    ) -> ApiResult<(Decision, bool)> {
        let rows = self.store.profile_ratification_decisions()?;
        let index = devai_store::config::ratification_index(&rows)?;
        let same_act = |d: &&Decision| {
            index.get(&d.id.to_string()).is_some_and(|r| {
                r.live
                    && r.payload.project_id == project_id
                    && r.payload.edition == edition
                    && r.payload.items.as_slice() == items
            })
        };
        if let Some(stored) = rows.iter().find(same_act) {
            return Ok((stored.clone(), false));
        }
        let decision = self.new_ratification(task, project_id, file, edition, items, rationale);
        self.store.record_decision(&decision)?;
        Ok((decision, true))
    }

    /// The ratification row itself: APPROVED, with the platform-computed
    /// edition and the full covered list in BOTH `refs` (the historic read
    /// surface) and `payload` (the typed binding the force rule reads).
    fn new_ratification(
        &self,
        task: Id,
        project_id: &str,
        file: &str,
        edition: &str,
        items: &[String],
        rationale: String,
    ) -> Decision {
        Decision {
            id: new_id(),
            task_id: task,
            kind: decision_kind::PROFILE_RATIFICATION.into(),
            verdict: Verdict::Approved,
            actor: self.actor.clone(),
            rationale: Some(rationale),
            refs: items.to_vec(),
            created_at: Utc::now(),
            payload: serde_json::to_value(ProfileRatification {
                project_id: project_id.to_string(),
                file: file.to_string(),
                edition: edition.to_string(),
                items: items.to_vec(),
                revokes: None,
            })
            .unwrap_or(serde_json::Value::Null),
        }
    }

    /// The pointer half of the act (RV3). A failure here refuses naming the
    /// file AND the edition: the decision row stays, the items stay OUT of
    /// force, and a repeated call finishes the act instead of recording a
    /// second decision.
    fn write_ratification_pointers(
        &self,
        path: &Path,
        edition: &str,
        decision: &Decision,
        items: &[String],
    ) -> ApiResult<()> {
        devai_store::config::set_ratification_pointers(path, &decision.id.to_string(), items)
            .map_err(|e| {
                ApiError::refused(
                    "profile_rules_unwritable",
                    format!(
                        "ratification decision {} was recorded, but the pointers did not land in {} (edition {edition}): {e}. The items stay OUT of force; re-run the same command to finish the act",
                        decision.id,
                        path.display()
                    ),
                )
            })
    }

    pub fn profile_ratify(&self, req: ProfileRatifyReq) -> ApiResult<ProfileRatifyOutcome> {
        self.require_human_session("profile.ratify")?;
        let rationale = Self::human_reason(req.rationale.as_deref(), "ratification")?;
        let task = self.store.resolve_task(&req.task)?;
        let (p, path) = self.profile_rules_target(&req.project)?;
        let (edition, overlay) = self.load_profile_overlay(&p, &path)?;
        let items = Self::ratification_items(&req, &p, &overlay)?;
        let project_id = p.id.to_string();
        let file = devai_store::config::overlay_rel_path(&project_id);
        let (decision, recorded_now) =
            self.record_ratification(task.id, &project_id, &file, &edition, &items, rationale)?;
        self.write_ratification_pointers(&path, &edition, &decision, &items)?;
        self.record_profile_event(event_kind::PROFILE_RATIFIED, task.id, &decision, &file)?;
        Ok(ProfileRatifyOutcome {
            task_id: task.id.to_string(),
            decision,
            recorded_now,
            project_id,
            file,
            edition,
            items,
        })
    }

    /// The stored row a revocation names, with its typed edition binding.
    /// Another kind, a non-APPROVED row or a payload without a binding
    /// refuses: only a live ratification can be taken out of force.
    fn ratification_to_revoke(&self, id: &str) -> ApiResult<(Decision, ProfileRatification)> {
        let revoked = self.store.get_decision(id)?;
        if revoked.kind != decision_kind::PROFILE_RATIFICATION
            || revoked.verdict != Verdict::Approved
        {
            return Err(ApiError::refused(
                "bad_request",
                format!(
                    "decision {} is not an APPROVED profile_ratification — only a live ratification can be revoked",
                    revoked.id
                ),
            ));
        }
        let payload: ProfileRatification = serde_json::from_value(revoked.payload.clone())
            .map_err(|e| {
                ApiError::refused(
                    "bad_request",
                    format!(
                        "decision {} carries no edition binding ({e}) — it cannot be revoked as a rules ratification",
                        revoked.id
                    ),
                )
            })?;
        Ok((revoked, payload))
    }

    /// RV6: one revocation per ratification. A second one refuses instead of
    /// appending a duplicate act to the history (append-only ≠ repetitive).
    fn ensure_not_revoked(&self, id: &str) -> ApiResult<()> {
        let already = self
            .store
            .profile_ratification_decisions()?
            .into_iter()
            .any(|d| {
                d.verdict != Verdict::Approved
                    && d.payload.get(json_field::REVOKES).and_then(|v| v.as_str()) == Some(id)
            });
        if already {
            return Err(ApiError::refused(
                "bad_request",
                format!(
                    "ratification {id} is already revoked — no second revocation row is written"
                ),
            ));
        }
        Ok(())
    }

    /// The revocation row: a NEW decision citing the revoked one — the
    /// APPROVED row is never rewritten, the rules file is never touched.
    fn revocation_decision(
        &self,
        revoked: &Decision,
        payload: &ProfileRatification,
        rationale: String,
    ) -> Decision {
        Decision {
            id: new_id(),
            task_id: revoked.task_id,
            kind: decision_kind::PROFILE_RATIFICATION.into(),
            verdict: Verdict::Rejected,
            actor: self.actor.clone(),
            rationale: Some(rationale),
            refs: vec![revoked.id.to_string()],
            created_at: Utc::now(),
            payload: serde_json::to_value(ProfileRatification {
                project_id: payload.project_id.clone(),
                file: payload.file.clone(),
                edition: payload.edition.clone(),
                items: payload.items.clone(),
                revokes: Some(revoked.id.to_string()),
            })
            .unwrap_or(serde_json::Value::Null),
        }
    }

    pub fn profile_revoke(&self, req: ProfileRevokeReq) -> ApiResult<ProfileRevokeOutcome> {
        self.require_human_session("profile.revoke")?;
        let rationale = Self::human_reason(req.rationale.as_deref(), "revocation")?;
        let (revoked, payload) = self.ratification_to_revoke(&req.decision)?;
        self.ensure_not_revoked(&revoked.id.to_string())?;
        let revocation = self.revocation_decision(&revoked, &payload, rationale);
        self.store.record_decision(&revocation)?;
        self.record_profile_event(
            event_kind::PROFILE_REVOKED,
            revoked.task_id,
            &revocation,
            &payload.file,
        )?;
        Ok(ProfileRevokeOutcome {
            decision: revocation,
            revoked,
            project_id: payload.project_id,
            file: payload.file,
            edition: payload.edition,
            items: payload.items,
        })
    }

    /// Append the human act to the task history (RV5). Append-only: one
    /// event per decision row; a repeated call for a decision that already
    /// has its event writes nothing.
    fn record_profile_event(
        &self,
        kind: &str,
        task_id: Id,
        decision: &Decision,
        file: &str,
    ) -> ApiResult<()> {
        let already = self.store.events(task_id, None)?.iter().any(|e| {
            e.kind == kind
                && e.payload.get(json_field::DECISION).and_then(|v| v.as_str())
                    == Some(decision.id.to_string().as_str())
        });
        if already {
            return Ok(());
        }
        self.store.insert_event(
            Some(task_id),
            kind,
            &self.actor,
            serde_json::json!({
                "decision": decision.id.to_string(),
                "project_id": decision.payload.get("project_id"),
                "file": file,
                "edition": decision.payload.get("edition"),
                "items": &decision.refs,
                "rationale": decision.rationale,
                "revokes": decision.payload.get("revokes"),
            }),
        )?;
        Ok(())
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
// free helpers
// ---------------------------------------------------------------------------

/// The typed refusal of a standalone `artifact.advance` for a kind that
/// carries no workflow advance (QA R5). The listed advancing kinds are
/// DERIVED from the core predicate — never a hand-copied set (QA R4).
fn not_advancing_refusal(kind: ArtifactKind) -> ApiError {
    ApiError::refused(
        "artifact_kind_not_advancing",
        format!(
            "{} carries no workflow advance — the advancing kinds are {} (the row alone lands via `artifact.publish_row`; publish through `artifact.publish` to get the row and its advance in one call)",
            kind.as_str(),
            ArtifactKind::ALL
                .iter()
                .filter(|k| k.advances_state())
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    )
}

// ---------------------------------------------------------------------------
// tests (REQ-MUT-05 AC3: dispatch == dedicated; REQ-MUT-06: effect parity)
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    /// Решение оператора 09.11: потолок спеки держит ПЛАТФОРМА. Проверяем, что
    /// превышение отклоняется типизированно, а граница и другие виды проходят.
    #[test]
    fn spec_size_cap_is_enforced_at_the_boundary() {
        use crate::mutations::{MAX_SPEC_BYTES, spec_size_guard};
        use devai_core::types::ArtifactKind;

        assert!(spec_size_guard(ArtifactKind::RequirementsSpec, MAX_SPEC_BYTES).is_ok());
        let err = spec_size_guard(ArtifactKind::RequirementsSpec, MAX_SPEC_BYTES + 1)
            .expect_err("спека сверх потолка обязана отклоняться");
        assert!(
            err.to_string().contains("spec_size"),
            "отказ типизирован полем spec_size: {err}"
        );
        // Другие артефакты потолком не ограничены.
        assert!(spec_size_guard(ArtifactKind::ImplementationReport, MAX_SPEC_BYTES * 10).is_ok());
    }
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

    /// Живое дерево показывает НЕЗАКОММИЧЕННЫЕ правки: именно их окно не
    /// видело, пока роль работает (закоммиченный changeset у идущего прогона пуст).
    #[test]
    fn workspace_live_reads_the_worktree_as_it_is_now() {
        let dir = tempfile::TempDir::new().unwrap();
        let api = api(dir.path());
        let id = fresh_task(&api, "живое дерево");

        // рабочий каталог задачи в доме
        let wt = dir.path().join("worktrees").join(id.to_string());
        std::fs::create_dir_all(&wt).unwrap();
        let git = |args: &[&str]| {
            let out = std::process::Command::new("git")
                .arg("-C")
                .arg(&wt)
                .args(args)
                .output()
                .unwrap();
            assert!(out.status.success(), "git {args:?}: {:?}", out);
        };
        git(&["init", "-q"]);
        std::fs::write(wt.join("a.txt"), "первая строка\n").unwrap();
        git(&["add", "."]);
        git(&["-c", "user.email=t@t", "-c", "user.name=t", "commit", "-qm", "init"]);
        // роль правила файл, но ещё не закоммитила
        std::fs::write(wt.join("a.txt"), "первая строка\nвторая строка\n").unwrap();

        let live = api.workspace_live(&id.to_string()).unwrap();
        assert_eq!(live.dirty, 1, "{live:?}");
        assert_eq!(live.files[0].path, "a.txt");
        assert_eq!(live.files[0].status, "M");
        assert_eq!(live.files[0].added, 1);

        let d = api.workspace_live_diff(&id.to_string(), "a.txt").unwrap();
        assert!(d.diff.contains("+вторая строка"), "{}", d.diff);
    }

    /// Остановка прогона человеком ПИШЕТ причину в строку: раньше строка
    /// закрывалась пустой, и карточка могла показать только статус.
    #[test]
    fn cancelled_execution_records_who_and_why() {
        let dir = tempfile::TempDir::new().unwrap();
        let api = api(dir.path());
        let id = fresh_task(&api, "остановить прогон");
        let task = api.task_start(&id.to_string()).unwrap().task;
        assert_eq!(task.state, TaskState::Statement);

        let started = api
            .exec_start(crate::mutations::ExecStartReq {
                id: id.to_string(),
                role: "statement-reviewer".into(),
                agent: None,
                inputs: vec![],
                run_id: None,
            })
            .unwrap();
        let exec_id = started.execution.id.to_string();
        let row = api.exec_cancel(&exec_id, Some("проверяю вручную")).unwrap();
        assert!(row
            .summary
            .as_deref()
            .unwrap_or("")
            .contains("проверяю вручную"));

        let read = api.exec_get(&exec_id).unwrap();
        let summary = read.summary.unwrap_or_default();
        assert!(summary.contains("остановлено человеком"), "{summary}");
        assert!(summary.contains("проверяю вручную"), "{summary}");
        assert!(summary.contains("api-tester"), "{summary}");
    }

    /// Ответ на вопрос постановки ДОПИСЫВАЕТСЯ в постановку: следующий круг
    /// судит полный текст, а не тот же самый (прецедент 01a1df87 — пять
    /// одинаковых кругов, потому что ответы никуда не попадали).
    #[test]
    fn answer_to_statement_question_is_written_into_the_statement() {
        let dir = tempfile::TempDir::new().unwrap();
        let api = api(dir.path());
        let id = fresh_task(&api, "промотка списка");
        let task = api.task_start(&id.to_string()).unwrap().task;
        assert_eq!(task.state, TaskState::Statement);

        let asked = api
            .question_ask(crate::mutations::AskReq {
                id: id.to_string(),
                text: "кто получит результат?".into(),
                context: None,
                options: vec![],
                blocking: true,
                execution: None,
            })
            .unwrap();

        let out = api
            .question_answer(
                &asked.question.id.to_string(),
                "любой оператор интерфейса devai",
            )
            .unwrap();
        assert_eq!(out.to_state, TaskState::Statement);

        let after = api.task_get(&id.to_string()).unwrap();
        assert!(
            after.business_description.contains("уточнения человека"),
            "постановка должна нести вписанный ответ: {}",
            after.business_description
        );
        assert!(
            after
                .business_description
                .contains("любой оператор интерфейса devai")
        );
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
        api.statement_approve_for_tests(&id).unwrap();

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

    /// The shared precondition of the two signature acts: a task in APPROVAL
    /// with a published CURRENT spec. Returns the task id and the spec id.
    fn approval_with_current_spec(api: &Api, title: &str) -> (String, String) {
        let t = fresh_task(api, title);
        let id = t.to_string();
        api.task_start(&id).unwrap();
        api.statement_approve_for_tests(&id).unwrap();
        let published = api.artifact_publish(spec_publish(&t, "s1")).unwrap();
        api.task_review(&id, Verdict::Approved, None).unwrap();
        assert_eq!(
            api.task_get(&id).unwrap().state,
            TaskState::Approval,
            "the fixture must sit on the APPROVAL gate"
        );
        (id, published.publish.artifact.id.to_string())
    }

    /// REQ-SIGN-05 AC3 + rework R1: reject writes ONE
    /// `requirements_rejection` row — verdict, actor, rationale and the
    /// CURRENT spec ref — beside the transition, and touches no artifact.
    /// Before this test the reject write path had no coverage at all; only
    /// the backfill dogfood ever produced such a row.
    #[test]
    fn reject_carries_one_decision_row_and_leaves_artifacts_alone() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let (id, spec_id) = approval_with_current_spec(&api, "reject-me");
        let task = api.task_get(&id).unwrap();
        let arts_before = api.store.artifacts(task.id).unwrap();

        let outcome = api
            .task_reject(&id, Some("спеку надо переписать".into()))
            .unwrap();
        assert_eq!(outcome.task.state, TaskState::Analysis);
        assert_eq!(
            outcome.event_kinds,
            vec![event_kind::REQUIREMENTS_REJECTED.to_string()]
        );

        let decisions = api.task_decisions(&id).unwrap();
        assert_eq!(decisions.len(), 1, "exactly one rejection row");
        let row = &decisions[0];
        assert_eq!(row.kind, decision_kind::REQUIREMENTS_REJECTION);
        assert_eq!(row.verdict, Verdict::Rejected);
        assert_eq!(row.actor, "api-tester");
        assert_eq!(row.rationale.as_deref(), Some("спеку надо переписать"));
        assert_eq!(row.refs, vec![spec_id], "refs cite the CURRENT spec");
        assert_eq!(row.task_id, task.id);

        // No artifact was staled, superseded or rewritten by the rejection.
        let arts_after = api.store.artifacts(task.id).unwrap();
        assert_eq!(arts_before.len(), arts_after.len());
        for (before, after) in arts_before.iter().zip(arts_after.iter()) {
            assert_eq!(before.id, after.id);
            assert_eq!(before.status, after.status);
            assert_eq!(before.sha256, after.sha256);
            assert_eq!(before.seq, after.seq);
        }
    }

    /// REQ-SIGN-06 / rework R6: signing twice is refused and writes nothing —
    /// no second transition event, no second decision row.
    #[test]
    fn a_second_approve_is_refused_and_adds_no_rows() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let (id, _) = approval_with_current_spec(&api, "approve-twice");
        api.task_approve(&id, Some("первая".into())).unwrap();
        let decisions = api.task_decisions(&id).unwrap().len();
        let events = api.task_events(&id, None).unwrap().len();

        let err = api.task_approve(&id, Some("вторая".into())).unwrap_err();
        assert_eq!(err.code(), ErrorCode("invalid_transition"), "{err}");
        assert_eq!(
            api.task_get(&id).unwrap().state,
            TaskState::Implementation,
            "the refusal leaves the task where the first signature put it"
        );
        assert_eq!(api.task_decisions(&id).unwrap().len(), decisions);
        assert_eq!(api.task_events(&id, None).unwrap().len(), events);
    }

    /// The same idempotency for the reject path: the second reject is
    /// refused (the task left APPROVAL) and no row or event is added.
    #[test]
    fn a_second_reject_is_refused_and_adds_no_rows() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let (id, _) = approval_with_current_spec(&api, "reject-twice");
        api.task_reject(&id, Some("первый".into())).unwrap();
        let decisions = api.task_decisions(&id).unwrap().len();
        let events = api.task_events(&id, None).unwrap().len();

        let err = api.task_reject(&id, Some("второй".into())).unwrap_err();
        assert_eq!(err.code(), ErrorCode("invalid_transition"), "{err}");
        assert_eq!(
            api.task_get(&id).unwrap().state,
            TaskState::Analysis,
            "the refusal leaves the task where the first rejection put it"
        );
        assert_eq!(api.task_decisions(&id).unwrap().len(), decisions);
        assert_eq!(api.task_events(&id, None).unwrap().len(), events);
    }

    /// Rework R1 (idempotency, both acts): the transition runs BEFORE the
    /// decision row, so a repeat is refused at the gate and leaves the FIRST
    /// stored row untouched — same rationale, actor and refs, no added event.
    #[test]
    fn a_repeated_signature_act_keeps_the_first_stored_row() {
        for reject in [false, true] {
            let dir = tempfile::tempdir().unwrap();
            let api = api(dir.path());
            let (id, spec_id) = approval_with_current_spec(&api, "repeat-once");
            let first_landed = if reject {
                api.task_reject(&id, Some("первый".into())).is_ok()
            } else {
                api.task_approve(&id, Some("первый".into())).is_ok()
            };
            assert!(first_landed, "the first act must land");
            let decisions_before = api.task_decisions(&id).unwrap().len();
            let events_before = api.task_events(&id, None).unwrap().len();

            let refused = if reject {
                api.task_reject(&id, Some("второй".into()))
                    .map(|_| ())
                    .unwrap_err()
            } else {
                api.task_approve(&id, Some("второй".into()))
                    .map(|_| ())
                    .unwrap_err()
            };
            assert_eq!(refused.code(), ErrorCode("invalid_transition"), "{refused}");
            assert_eq!(
                api.task_decisions(&id).unwrap().len(),
                decisions_before,
                "a repeat adds no decision row"
            );
            assert_eq!(
                api.task_events(&id, None).unwrap().len(),
                events_before,
                "a repeat adds no event"
            );
            let row = &api.task_decisions(&id).unwrap()[0];
            assert_eq!(
                row.rationale.as_deref(),
                Some("первый"),
                "the first act's rationale survives the repeated call"
            );
            assert_eq!(row.refs, vec![spec_id], "the first act's refs survive");
        }
    }

    /// A publish request of the CURRENT spec kind — the shape both publish
    /// paths of the parity test take.
    fn spec_publish(id: &Id, name: &str) -> PublishReq {
        // Bodies are JSON, as any gated spec body is: the phased advance
        // door re-reads the CURRENT body TYPED — a non-JSON blob is a
        // corruption it refuses loudly, not the legal name-less body (the
        // name-less JSON body stays the Absent branch on both paths).
        let content = format!(r#"{{"requirements": "{name}"}}"#).into_bytes();
        PublishReq {
            id: id.to_string(),
            kind: ArtifactKind::RequirementsSpec,
            name: name.into(),
            content,
            depends: vec![],
            contract: None,
            execution: None,
            category: DataCategory::Artifact,
            verdict: None,
        }
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
        api.statement_approve_for_tests(&a.to_string()).unwrap();
        api.task_start(&b.to_string()).unwrap();
        api.statement_approve_for_tests(&b.to_string()).unwrap();

        let compound = api.artifact_publish(spec_publish(&a, "s1")).unwrap();
        let row = api.artifact_publish_row(spec_publish(&b, "s2")).unwrap();
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
        api.statement_approve_for_tests(&id).unwrap();
        api.exec_start(ExecStartReq {
            id: id.clone(),
            role: "requirements-analyst".into(),
            agent: None,
            inputs: vec![],
            run_id: None,
        })
        .unwrap();

        let e = api
            .exec_start(ExecStartReq {
                id: id.clone(),
                role: "requirements-analyst".into(),
                agent: None,
                inputs: vec![],
                run_id: None,
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
                .filter(|x| x.execution.status == ExecutionStatus::Running)
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
        // The refusal wrote nothing: the task is where the row phase left it
        // (the fixture stands on the statement stage — the first role state).
        assert_eq!(
            api.task_get(&t.to_string()).unwrap().state,
            TaskState::Statement
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
        devai_runtime::evaluation::assemble(
            &form_profile(),
            &form_eval_run(),
            &form_assessment(),
            "ex-form",
            &unbound_revision(),
            &std::collections::BTreeMap::new(),
        )
        .json
    }

    fn form_profile() -> devai_store::config::Profile {
        serde_yaml::from_str(FORM_PROFILE_YAML).expect("form fixture profile parses")
    }

    /// The mechanical probe results the assembler consumes: one passed check.
    fn form_eval_run() -> devai_runtime::evaluation::EvalRun {
        let mut mech = std::collections::HashMap::new();
        mech.insert(
            "mech/pass".to_string(),
            devai_runtime::evaluation::ProbeOutcome {
                state: devai_runtime::evaluation::ProbeState::Pass,
                detail: "cargo test: 12 passed".into(),
                output: "12 passed".into(),
            },
        );
        devai_runtime::evaluation::EvalRun {
            gates: vec![],
            mech,
            hybrid: std::collections::HashMap::new(),
            evidence_text: String::new(),
        }
    }

    /// The auditor's assessment answer: one breached floor and one nuance.
    fn form_assessment() -> serde_json::Value {
        serde_json::json!({
            "scores": { "corr/floor": 0.4, "cl/y": 0.9 },
            "findings": [{
                "check": "corr/floor",
                "text": "the guard is missing at src/a.rs:7",
                "evidence": ["fn step() { unchecked() }"]
            }]
        })
    }

    fn unbound_revision() -> WorkspaceRevision {
        WorkspaceRevision {
            git_head: None,
            dirty_digest: None,
            profile: None,
            roles: vec![],
        }
    }

    /// Publish the assembler-produced @5 document and return its artifact row.
    fn publish_form_report(api: &Api, t: Id) -> Artifact {
        api.artifact_publish_row(PublishReq {
            id: t.to_string(),
            kind: ArtifactKind::QualityReport,
            name: "qr".into(),
            content: serde_json::to_vec(&assembled_report_doc()).unwrap(),
            depends: vec![],
            contract: Some("quality-report@5".into()),
            execution: None,
            category: DataCategory::Artifact,
            verdict: None,
        })
        .unwrap()
        .artifact
    }

    /// The ratified return as the store records it: task → REWORK and the
    /// rejected report CURRENT → STALE (OQ-QH-3, a data change).
    fn return_to_rework_state(api: &Api, t: Id, artifact: Id) {
        let task = api.store.get_task(&t.to_string()).unwrap();
        let mut m = Mutation {
            task: task.clone(),
            events: vec![],
            artifact_status: vec![(artifact, ArtifactStatus::Stale)],
            questions: vec![],
        };
        m.task.state = TaskState::Rework;
        api.store.commit(task.lock_version, &m).unwrap();
    }

    /// One auditor execution row in the shape the freshness fact reads. The
    /// row is left RUNNING; a caller that needs a finished run closes it with
    /// `finish_execution`.
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
            bundle: None,
            run_id: None,
        }
    }

    /// REQ-QH-01/06: the panel quotes the STORED human form (it never
    /// re-assembles one), and the UI view is the same read — same text,
    /// same source token.
    #[test]
    fn the_stored_human_form_is_what_both_channels_present() {
        let dir = tempfile::tempdir().unwrap();
        let api = seeded_api(dir.path());
        let t = fresh_task(&api, "form-presentation");
        publish_form_report(&api, t);

        let p = api.presented_quality_report(t).unwrap().unwrap();
        assert_eq!(p.source, present::RenderSource::HumanForm);
        assert!(
            p.text.contains("БЬЁТ ПО ВЕРДИКТУ") && p.text.contains("важность 0.120"),
            "the stored form is rendered, grouped by decision weight: {}",
            p.text
        );
        // R7/R5 + R4 (decision of 07.11): the passed mechanical check comes
        // through the real producer and is NOT an «unknown state» hole; a 1.0
        // with no decision link is the norm, so it is counted, not listed.
        assert!(!p.text.contains("mech/pass"), "{}", p.text);
        assert!(p.text.contains("пройдено без замечаний: 1"), "{}", p.text);
        assert!(!p.text.contains("не распознано"), "{}", p.text);
        assert!(
            !p.text.contains("RAW-FLAT-TEXT-MARKER"),
            "the flat text is the fallback, not the channel, for @5 bodies"
        );
        assert!(!p.freshness.stale);
        assert_eq!(p.freshness.report_status, "CURRENT");
    }

    /// The UI view carries the FIRST LAYER of the same read — the panel does
    /// not derive it, and both channels carry one and the same layer
    /// (REQ-SAY-01..06, rework R1 wiring).
    #[test]
    fn the_ui_view_carries_the_first_layer_of_the_same_read() {
        let dir = tempfile::tempdir().unwrap();
        let api = seeded_api(dir.path());
        let t = fresh_task(&api, "form-first-layer");
        publish_form_report(&api, t);

        let p = api.presented_quality_report(t).unwrap().unwrap();
        let view = api.task_report(&t.to_string()).unwrap();
        assert_eq!(view.report_text.as_deref(), Some(p.text.as_str()));
        assert_eq!(view.source, "human_form");
        let first = view
            .first_layer
            .as_ref()
            .expect("a readable stored form reaches the UI with its first layer");
        assert_eq!(Some(first), p.first_layer.as_ref(), "one read, one layer");
        assert!(
            first.score_line.starts_with("сохранённый балл работы: 0."),
            "the saved score travels beside the assessment: {}",
            first.score_line
        );
        assert!(
            first.findings.iter().any(|f| f.site == "src/a.rs:7"),
            "the breached floor reaches the findings with its site: {:?}",
            first.findings
        );
        assert!(
            first
                .findings
                .iter()
                .all(|f| !f.what.is_empty() && !f.stake.is_empty()),
            "every finding is a phrase with its stake"
        );
    }

    /// REQ-QH-06 fallback: a pre-@5 body still renders its own stored text,
    /// byte for byte — no form is invented for it.
    #[test]
    fn a_legacy_body_still_renders_its_own_flat_text() {
        let dir = tempfile::tempdir().unwrap();
        let api = seeded_api(dir.path());
        let t = fresh_task(&api, "legacy");
        api.artifact_publish_row(PublishReq {
            id: t.to_string(),
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
            api.quality_report_text(t).unwrap().as_deref(),
            Some("LEGACY-FLAT-TEXT")
        );
        assert_eq!(
            api.presented_quality_report(t).unwrap().unwrap().source,
            present::RenderSource::FlatText
        );
        // A legacy report has no human form, so it has NO first layer: the
        // viewer is never handed an invented one (rework R1 wiring).
        assert!(
            api.task_report(&t.to_string())
                .unwrap()
                .first_layer
                .is_none(),
            "the new layer is not invented for a body that never had a form"
        );
    }

    /// REQ-QH-05: after a return the stored report is announced as stale —
    /// with the task phase and the stored status — instead of being shown as
    /// the decision; both channels carry the same fact.
    #[test]
    fn rework_announces_the_stored_report_as_stale() {
        let dir = tempfile::tempdir().unwrap();
        let api = seeded_api(dir.path());
        let t = fresh_task(&api, "rework-stale");
        let artifact = publish_form_report(&api, t);
        return_to_rework_state(&api, t, artifact.id);

        let p = api.presented_quality_report(t).unwrap().unwrap();
        assert!(p.freshness.stale);
        assert_eq!(p.freshness.report_status, "STALE");
        assert_eq!(p.freshness.task_state, "REWORK");
        let text = api.quality_report_text(t).unwrap().unwrap();
        assert_eq!(
            text.lines().next().unwrap_or(""),
            "⚠ отчёт v1 УСТАРЕЛ — задача в REWORK: ре-аудит ещё не запущен",
            "the full formulation is pinned: no execution exists, so none is named"
        );
        assert!(text.contains("НЕ текущее решение"), "{text}");
        assert!(
            text.contains("БЬЁТ ПО ВЕРДИКТУ"),
            "the stored form is still readable: {text}"
        );

        let view = api.task_report(&t.to_string()).unwrap();
        assert_eq!(view.status, "STALE");
        assert!(view.freshness.expect("freshness is part of the view").stale);
    }

    /// Fixture for the freshness facts below: publish a form report, then
    /// return the task to rework after a COMPLETED auditor run that produced
    /// that very report. Returns the completed run.
    fn completed_audit_then_rework(api: &Api, t: Id) -> RoleExecution {
        let artifact = publish_form_report(api, t);
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
        return_to_rework_state(api, t, artifact.id);
        done
    }

    /// The first line of the CLI text channel — the freshness marker.
    fn reported_marker(api: &Api, t: Id) -> String {
        api.quality_report_text(t)
            .unwrap()
            .unwrap()
            .lines()
            .next()
            .unwrap_or("")
            .to_string()
    }

    /// R1/R5: the freshness marker names the STORE fact. A return whose last
    /// auditor run is the COMPLETED one that produced the report is NOT an
    /// unfinished re-audit (the old code invented one).
    #[test]
    fn a_completed_auditor_run_is_not_an_unfinished_reaudit() {
        let dir = tempfile::tempdir().unwrap();
        let api = seeded_api(dir.path());
        let t = fresh_task(&api, "freshness-completed");
        let done = completed_audit_then_rework(&api, t);

        let p = api.presented_quality_report(t).unwrap().unwrap();
        assert_eq!(
            p.freshness.marker,
            format!(
                "⚠ отчёт v1 УСТАРЕЛ — задача в REWORK: новый прогон аудитора не идёт; последний прогон {} COMPLETED 2026-11-06 10:05:00Z",
                short_id(&done.id)
            ),
            "the completed run that produced the report is not an unfinished re-audit, and no cause is claimed without a staled event"
        );
        assert!(!p.freshness.marker.contains("в работе"));
        assert!(!p.freshness.marker.contains("не завершён"));
        // The text channel carries the same full marker, not a summary.
        assert_eq!(reported_marker(&api, t), p.freshness.marker);
    }

    /// …and a real return records its cause as a workflow event
    /// (`artifacts.staled` with `cause`), which the marker then repeats: the
    /// cause is READ from the store, not reconstructed at presentation time.
    #[test]
    fn the_stale_marker_repeats_the_recorded_stale_cause() {
        let dir = tempfile::tempdir().unwrap();
        let api = seeded_api(dir.path());
        let t = fresh_task(&api, "freshness-cause");
        publish_form_report(&api, t);

        let task = api.store.get_task(&t.to_string()).unwrap();
        let mut open = Mutation {
            task: task.clone(),
            events: vec![],
            artifact_status: vec![],
            questions: vec![],
        };
        open.task.state = TaskState::Qa;
        api.store.commit(task.lock_version, &open).unwrap();

        let task = api.store.get_task(&t.to_string()).unwrap();
        let arts = api.store.artifacts(t).unwrap();
        let returned = workflow::return_to_rework(
            ProcessPolicy::factory(),
            &task,
            &arts,
            Some("ещё раз, с тестами"),
            "guerrero",
            Utc::now(),
        )
        .unwrap();
        api.store.commit(task.lock_version, &returned).unwrap();

        let marker = reported_marker(&api, t);
        assert!(
            marker.contains("отчёт возвращён человеком на реворк (guerrero)"),
            "the recorded cause is repeated: {marker}"
        );
        assert!(
            !marker.contains("возврат без ре-аудита"),
            "no cause is invented alongside the recorded one: {marker}"
        );
    }

    /// …and only a live RUNNING row earns «в работе»: the marker then names
    /// that execution and its own stored status.
    #[test]
    fn in_work_is_claimed_only_with_a_live_audit_row() {
        let dir = tempfile::tempdir().unwrap();
        let api = seeded_api(dir.path());
        let t = fresh_task(&api, "freshness-running");
        let artifact = publish_form_report(&api, t);
        return_to_rework_state(&api, t, artifact.id);

        let running = auditor_exec(t, Utc::now());
        api.store.start_execution(&running).unwrap();
        let p = api.presented_quality_report(t).unwrap().unwrap();
        assert_eq!(
            p.freshness.marker,
            format!(
                "⚠ отчёт v1 УСТАРЕЛ — задача в REWORK: готовится ре-аудит (execution {}, в работе)",
                short_id(&running.id)
            )
        );
        assert_eq!(
            p.freshness.execution_status.as_deref(),
            Some("RUNNING"),
            "the view names the live execution, not a finished one"
        );
    }

    /// Publish a plain-text quality report (the pre-@5 body shape) and return
    /// its artifact row.
    fn publish_text_report(api: &Api, t: Id, text: &str) -> Artifact {
        api.artifact_publish_row(PublishReq {
            id: t.to_string(),
            kind: ArtifactKind::QualityReport,
            name: "qr".into(),
            content: serde_json::to_vec(&serde_json::json!({ "report_text": text })).unwrap(),
            depends: vec![],
            contract: None,
            execution: None,
            category: DataCategory::Artifact,
            verdict: None,
        })
        .unwrap()
        .artifact
    }

    /// QA R2 / rework R4: «no report» (Ok(None)) and a stored text body are
    /// distinguishable — absence is answered as absence, never as an empty
    /// placeholder.
    #[test]
    fn quality_report_text_is_absent_before_a_report_and_verbatim_after() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let t = fresh_task(&api, "gate-text");
        assert_eq!(
            api.quality_report_text(t).unwrap(),
            None,
            "no report yet — honest absence"
        );
        publish_text_report(&api, t, "all good");
        assert_eq!(
            api.quality_report_text(t).unwrap().as_deref(),
            Some("all good")
        );
    }

    /// QA R2 / rework R4: a corrupt CURRENT blob is presented with a marker
    /// naming the artifact and a distinct render source — never as absence of
    /// a report and never as the generic «assembled before @2» placeholder.
    #[test]
    fn a_corrupt_current_blob_is_named_not_read_as_absence() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let t = fresh_task(&api, "gate-text-corrupt");
        let row = publish_text_report(&api, t, "all good");
        // Corrupt the stored blob under the CURRENT row: the next read must
        // say so, not fall through to «no report» and not show the
        // pre-@2 placeholder.
        std::fs::write(api.store.blob_path(&row.sha256), b"not-json-at-all").unwrap();
        let p = api.presented_quality_report(t).unwrap().unwrap();
        assert_eq!(p.source, present::RenderSource::CorruptBody);
        assert!(p.text.contains("повреждён"), "{}", p.text);
        assert!(!p.text.contains("before @2"), "{}", p.text);
        assert!(p.first_layer.is_none(), "a broken body has no first layer");
        let view = api.task_report(&t.to_string()).unwrap();
        assert_eq!(view.source, "corrupt_body");
        assert!(view.first_layer.is_none(), "the UI view invents no layer");
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
        assert!(
            api.presented_quality_report(t)
                .unwrap()
                .unwrap()
                .first_layer
                .is_none(),
            "a damaged form has no first layer — the flat text stays flat"
        );
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
        api.statement_approve_for_tests(&id).unwrap();
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
        api.statement_approve_for_tests(&id).unwrap();
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
        api.statement_approve_for_tests(&id).unwrap();
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
        api.statement_approve_for_tests(&id).unwrap();
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
            .task_run_inline(RunReq {
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
        // The drive also publishes the statement checker's verdict, so the
        // steps are "checker + analyst + reviewer" — at least the two the
        // gate presents must be there.
        assert!(
            arts.len() >= 2,
            "analyst + reviewer ran, got {}",
            arts.len()
        );
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
                serde_json::from_slice(&api.store.read_artifact_body(&a).unwrap()).unwrap();
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

    /// The four task kinds the backfill rule distinguishes, plus the two
    /// projects: freshly created for every test so no state leaks.
    struct BackfillFixture {
        free: Id,
        owned: Id,
        closed: Id,
        home: Id,
        project: Id,
        other: Id,
    }

    /// One unbound task, one owned by ANOTHER project, one parked terminal,
    /// one already on `devai` — the exact spread of the ratified rule.
    fn backfill_fixture(api: &Api) -> BackfillFixture {
        let p = devai_project(api);
        let other = api
            .project_add(ProjectAddReq {
                name: "other".into(),
                description: String::new(),
                repos: vec![],
            })
            .unwrap();
        let free = fresh_task(api, "unbound-draft");
        let owned = fresh_task(api, "owned-by-other");
        api.task_project_set(TaskProjectSetReq {
            id: owned.to_string(),
            project: other.id.to_string(),
            expected_lock: None,
        })
        .unwrap();
        let closed = fresh_task(api, "terminal-history");
        // Park it terminal through the workflow plane (CANCELLED).
        api.task_op(&closed.to_string(), WorkflowOp::Cancel { reason: None })
            .unwrap();
        let home = fresh_task(api, "already-devai");
        api.task_project_set(TaskProjectSetReq {
            id: home.to_string(),
            project: p.id.to_string(),
            expected_lock: None,
        })
        .unwrap();
        BackfillFixture {
            free,
            owned,
            closed,
            home,
            project: p.id,
            other: other.id,
        }
    }

    /// REQ-BIND-03 under the ratified OQ-BIND-2 rule: the backfill binds
    /// every UNBOUND NON-TERMINAL task, leaves terminal history at its old
    /// address, never re-binds another project's task. Each binding is one
    /// `task.project.set` event.
    #[test]
    fn task_project_backfill_applies_the_ratified_rule() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let f = backfill_fixture(&api);

        let o = api.task_project_backfill("devai").expect("backfill runs");
        assert_eq!(o.bound, vec![f.free]);
        assert_eq!(o.already_bound, vec![f.home]);
        assert_eq!(o.skipped_terminal, vec![f.closed]);
        assert_eq!(o.skipped_other_project, vec![f.owned]);

        let stored = api.store.get_task(&f.free.to_string()).unwrap();
        assert_eq!(stored.project_id, Some(f.project));
        assert_eq!(stored.lock_version, 2);
        let evs = api.task_events(&f.free.to_string(), None).unwrap();
        assert_eq!(
            evs.iter()
                .filter(|e| e.kind == event_kind::TASK_PROJECT_SET)
                .count(),
            1
        );
        // Terminal history untouched, another project's task not re-bound.
        assert_eq!(
            api.store
                .get_task(&f.closed.to_string())
                .unwrap()
                .project_id,
            None
        );
        assert_eq!(
            api.store.get_task(&f.owned.to_string()).unwrap().project_id,
            Some(f.other)
        );
    }

    /// Re-application finds no candidates and changes nothing; an unknown
    /// project refuses typed before touching a row.
    #[test]
    fn task_project_backfill_is_idempotent_and_refuses_unknown_project() {
        let dir = tempfile::tempdir().unwrap();
        let api = api(dir.path());
        let f = backfill_fixture(&api);
        api.task_project_backfill("devai").expect("first run");

        let o2 = api.task_project_backfill("devai").unwrap();
        assert!(o2.bound.is_empty());
        assert_eq!(o2.already_bound.len(), 2);
        assert_eq!(
            api.store
                .get_task(&f.free.to_string())
                .unwrap()
                .lock_version,
            2
        );

        let err = api
            .task_project_backfill("no-such")
            .expect_err("unknown project refuses");
        assert_eq!(err.code(), ErrorCode("not_found"));
    }
}
