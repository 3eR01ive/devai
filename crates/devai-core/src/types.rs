use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{CoreError, CoreResult};

pub type Id = Uuid;
pub type Timestamp = DateTime<Utc>;

/// Generates a new sortable unique identifier (UUIDv7).
pub fn new_id() -> Id {
    Uuid::now_v7()
}

/// Renders an entity as its JSON form (used for stable string columns).
pub fn json_string<T: Serialize>(v: &T) -> String {
    serde_json::to_string(v).unwrap_or_else(|e| panic!("core json serialize failed: {e}"))
}

pub fn json_parse<T: for<'de> Deserialize<'de>>(s: &str) -> Result<T, serde_json::Error> {
    serde_json::from_str(s)
}

/// Parses a JSON array column, tolerating NULL/empty as `[]`. A NON-empty
/// column that fails to parse is a loud error, never a silent empty list
/// (QA return7 п.(г): a corrupt `depends_on` read as `[]` would silently
/// empty the stale-closure graph — the dependency data itself is intact in
/// the row, so dropping it on read is data loss by projection).
pub fn json_array<T: for<'de> Deserialize<'de>>(s: Option<&str>) -> CoreResult<Vec<T>> {
    match s {
        None | Some("") => Ok(Vec::new()),
        Some(v) => serde_json::from_str(v)
            .map_err(|e| CoreError::guard(format!("corrupt JSON array column `{v}`: {e}"))),
    }
}

macro_rules! str_enum {
    ($ty:ty, $($variant:ident => $s:literal),+ $(,)?) => {
        impl $ty {
            pub fn as_str(self) -> &'static str {
                match self { $(Self::$variant => $s,)+ }
            }
            pub fn parse(s: &str) -> CoreResult<Self> {
                $( if s.eq_ignore_ascii_case($s) { return Ok(Self::$variant); } )+
                Err(CoreError::UnknownVariant { ty: stringify!($ty), value: s.to_string() })
            }
            pub const ALL: &'static [Self] = &[$(Self::$variant,)+];
        }
    };
}

// ---------------------------------------------------------------------------
// Workflow state machine
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum TaskState {
    Draft,
    Analysis,
    Review,
    Approval,
    Implementation,
    Qa,
    Rework,
    WaitingForInput,
    Paused,
    Blocked,
    Failed,
    Accepted,
    Cancelled,
}

str_enum!(TaskState,
    Draft => "DRAFT",
    Analysis => "ANALYSIS",
    Review => "REVIEW",
    Approval => "APPROVAL",
    Implementation => "IMPLEMENTATION",
    Qa => "QA",
    Rework => "REWORK",
    WaitingForInput => "WAITING_FOR_INPUT",
    Paused => "PAUSED",
    Blocked => "BLOCKED",
    Failed => "FAILED",
    Accepted => "ACCEPTED",
    Cancelled => "CANCELLED",
);

impl TaskState {
    pub fn is_terminal(self) -> bool {
        matches!(self, Self::Accepted | Self::Cancelled)
    }

    /// True while a task sits in an orthogonal hold state (a real stage is
    /// remembered in `return_state` and will be restored on release).
    pub fn is_hold(self) -> bool {
        matches!(
            self,
            Self::WaitingForInput | Self::Paused | Self::Blocked | Self::Failed
        )
    }
}

// ---------------------------------------------------------------------------
// Entities
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Project {
    pub id: Id,
    pub name: String,
    pub description: String,
    /// Absolute paths of local git repositories belonging to the project.
    pub repos: Vec<String>,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Project {
    pub fn new(name: impl Into<String>, description: impl Into<String>, repos: Vec<String>, now: Timestamp) -> Self {
        Self {
            id: new_id(),
            name: name.into(),
            description: description.into(),
            repos,
            created_at: now,
            updated_at: now,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Task {
    pub id: Id,
    pub project_id: Option<Id>,
    pub title: String,
    pub business_description: String,
    pub state: TaskState,
    /// Stage currently remembered while the task is in a hold state.
    pub return_state: Option<TaskState>,
    /// Sequence number of the current requirements specification version.
    /// 0 means "nothing approved-published yet".
    pub requirements_seq: u32,
    /// Monotonic token for optimistic concurrency (incremented on every write).
    pub lock_version: u64,
    pub created_at: Timestamp,
    pub updated_at: Timestamp,
}

impl Task {
    pub fn new(title: impl Into<String>, business_description: impl Into<String>, project_id: Option<Id>, now: Timestamp) -> Self {
        Self {
            id: new_id(),
            project_id,
            title: title.into(),
            business_description: business_description.into(),
            state: TaskState::Draft,
            return_state: None,
            requirements_seq: 0,
            lock_version: 1,
            created_at: now,
            updated_at: now,
        }
    }
}

// ---------------------------------------------------------------------------
// Task dependency graph (REQ-01/02/07)
// ---------------------------------------------------------------------------

/// The vocabulary of typed task-to-task dependencies. Required minimum per
/// the approved spec (REQ-02): `parent` ("породила" — why this task exists,
/// genealogy only) and `blocks` ("не может продолжиться без той" — resume
/// and human acceptance are refused while the edge is open). `parent` is
/// one kind of the same n-ary relation, not a separate mechanism.
///
/// Stored as TEXT (`kind` column of `task_dependencies`): adding a new kind
/// is a new vocabulary entry in this enum — no stored row is ever rewritten
/// (REQ-02 AC2). RATIFIED DECISION OQ3 (human approval of the spec):
/// `parent` is purely genealogical and NEVER gates continuation — only an
/// explicit `blocks` edge does, enforced in `workflow::open_blockers`.
/// RATIFIED DECISION OQ4: each kind forms a DAG on its own (cycles are
/// rejected at link time per kind); a MIXED cycle — child `blocks` its
/// parent while the parent `parent`-links the child, the standard
/// backfill/gate case — is legal and must stay expressible.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DependencyKind {
    Parent,
    Blocks,
}

str_enum!(DependencyKind,
    Parent => "parent",
    Blocks => "blocks",
);

/// One stored dependency edge (table `task_dependencies`) together with the
/// read-time projection of BOTH endpoint tasks' state and title. The edge is
/// the stored fact; the states come from the same query, they are not stored
/// on the edge (REQ-04 AC1: the whole graph — edges and task states — is
/// retrieved from the store in one query).
/// This type describes the LIVE graph: an edge is immutable (ratified OQ7),
/// removal marks the row SUPERSEDED and the row then leaves every ACTIVE
/// read, so instances of this type always carry a still-active relation;
/// the superseded history stays in the table and in the event log.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TaskDependency {
    pub id: Id,
    /// The dependent task — the one that "exists because of" / "cannot
    /// continue without".
    pub task_id: Id,
    pub task_state: TaskState,
    pub task_title: String,
    /// The depended-on task (parent / blocker).
    pub depends_on_id: Id,
    pub depends_on_state: TaskState,
    pub depends_on_title: String,
    pub kind: DependencyKind,
    /// Author of the edge — attributed like `decisions.actor` (REQ-07 AC3).
    pub actor: String,
    pub created_at: Timestamp,
}

/// One task node of the stored dependency graph with its outgoing edges:
/// the planning-level view (REQ-05) and the roadmap input (REQ-04) — node =
/// task + state, edge = typed dependency. A task with no stored edges is a
/// node with an empty `dependencies` set, never a corrupt row (REQ-08 AC2):
/// rows written before schema v4 simply have no dependency records.
/// An "epic" is not a separate entity: per the RATIFIED DECISION OQ8 an
/// epic is a task VERTEX IN THE SAME GRAPH — no outgoing ACTIVE `parent`
/// edge and at least one incoming one (a root of a derivation tree) — named
/// by the dependency store read `Store::epic_heads`, no new table (REQ-06).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DependencyNode {
    pub task: Task,
    pub dependencies: Vec<TaskDependency>,
}

/// READ VIEW of the epic registry — explicitly NOT a stored entity (REQ-06
/// AC1: no epic table or column was introduced). RATIFIED DECISION OQ8: an
/// epic is a task vertex in the same dependency graph with NO outgoing
/// ACTIVE `parent` edge and AT LEAST ONE incoming one; the store derives
/// this view with one query over `task_dependencies` + `tasks`, the very
/// same relation any other dependency read uses (REQ-06 AC2).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EpicVertex {
    pub task: Task,
    /// Direct derived children — tasks carrying an ACTIVE `parent` edge
    /// into this vertex.
    pub derived_children: usize,
}

// ---------------------------------------------------------------------------
// Artifacts
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ArtifactKind {
    ProjectProfile,
    RequirementsSpec,
    ReviewReport,
    ImplementationPlan,
    ImplementationReport,
    Patch,
    TestReport,
    QualityReport,
    DecisionRecord,
    Other,
}

str_enum!(ArtifactKind,
    ProjectProfile => "PROJECT_PROFILE",
    RequirementsSpec => "REQUIREMENTS_SPEC",
    ReviewReport => "REVIEW_REPORT",
    ImplementationPlan => "IMPLEMENTATION_PLAN",
    ImplementationReport => "IMPLEMENTATION_REPORT",
    Patch => "PATCH",
    TestReport => "TEST_REPORT",
    QualityReport => "QUALITY_REPORT",
    DecisionRecord => "DECISION_RECORD",
    Other => "OTHER",
);

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ArtifactStatus {
    Current,
    Stale,
    Superseded,
}

str_enum!(ArtifactStatus,
    Current => "CURRENT",
    Stale => "STALE",
    Superseded => "SUPERSEDED",
);

/// Data categories from the ТЗ: formal results vs recoverable vs caches.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum DataCategory {
    Artifact,
    Checkpoint,
    Cache,
}

str_enum!(DataCategory,
    Artifact => "ARTIFACT",
    Checkpoint => "CHECKPOINT",
    Cache => "CACHE",
);

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Artifact {
    pub id: Id,
    pub task_id: Id,
    pub kind: ArtifactKind,
    pub name: String,
    /// Per-task, per-kind sequence number (requirements versioning).
    pub seq: u32,
    pub category: DataCategory,
    pub status: ArtifactStatus,
    pub sha256: String,
    pub size: u64,
    pub media_type: String,
    pub contract: Option<ContractRef>,
    /// `"user"` or the id of the RoleExecution that produced it.
    pub created_by: String,
    pub execution_id: Option<Id>,
    pub depends_on: Vec<Id>,
    pub created_at: Timestamp,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ContractRef {
    pub name: String,
    pub version: u32,
}

impl ContractRef {
    pub fn parse(s: &str) -> CoreResult<Self> {
        let (name, ver) = s
            .rsplit_once('@')
            .ok_or_else(|| CoreError::guard(format!("contract ref must be `name@version`, got `{s}`")))?;
        let version = ver
            .parse::<u32>()
            .map_err(|_| CoreError::guard(format!("contract version is not a number: `{ver}`")))?;
        Ok(Self { name: name.to_string(), version })
    }
}

impl std::fmt::Display for ContractRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}@{}", self.name, self.version)
    }
}

// ---------------------------------------------------------------------------
// Roles, agents, executions
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RoleDefinition {
    pub id: Id,
    pub slug: String,
    pub version: u32,
    pub name: String,
    pub purpose: String,
    pub instructions: String,
    pub tools: Vec<String>,
    pub input_contract: Option<ContractRef>,
    pub output_contract: Option<ContractRef>,
    pub done_criteria: String,
    pub created_at: Timestamp,
}

/// Capability name for a role that MUTATES workspace files. Declared in
/// `config/roles/*.yaml` `tools:` — the vocabulary entry lives here, in core,
/// next to the rest of the domain vocabulary.
pub const CAP_FILESYSTEM_READ_WRITE: &str = "filesystem:read-write";

impl RoleDefinition {
    /// A WRITING role, decided from the role's declarative capability list —
    /// never from which artifact kind it happens to output. Which roles write
    /// is configuration (the same idiom as `rework-context.yaml`: WHICH role
    /// acts is data, not code); when a second writing role is declared it
    /// inherits workspace isolation with no engine edit.
    pub fn writes_workspace(&self) -> bool {
        self.tools.iter().any(|t| t == CAP_FILESYSTEM_READ_WRITE)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AgentInstance {
    pub id: Id,
    pub name: String,
    pub provider: String,
    pub model: String,
    pub runtime: String,
    pub created_at: Timestamp,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ExecutionStatus {
    Running,
    Completed,
    Failed,
    Cancelled,
}

str_enum!(ExecutionStatus,
    Running => "RUNNING",
    Completed => "COMPLETED",
    Failed => "FAILED",
    Cancelled => "CANCELLED",
);

/// The workspace state an execution was bound to, captured PLATFORM-side at
/// execution start (never supplied by a model): git HEAD, a canonical digest
/// of the dirty working tree, the evaluation profile identity, and the set of
/// current role definitions. Every field is required to be PRESENT; values
/// are NULL where the workspace cannot answer (no git repo / no commits →
/// head+digest NULL; no profile loaded → profile NULL) — such a revision is
/// NOT comparable, and freshness must not be claimed from it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct WorkspaceRevision {
    /// `git rev-parse HEAD` of the probe workspace; NULL outside a git repo
    /// or in a repo without commits.
    pub git_head: Option<String>,
    /// SHA-256 hex over the canonical dirty-tree line sequence: for every
    /// path `git status --porcelain -z --no-renames --untracked-files=all`
    /// reports, one TAB-separated line `XY<TAB>worktree-oid<TAB>index-oid
    ///<TAB>path` (worktree-oid from read-only `git hash-object`, index-oid
    /// from `git ls-files -s`, `-` when absent) — capturing tracked
    /// modifications, staged content, and untracked non-ignored files; lines
    /// sorted lexicographically, joined by newlines. A clean tree hashes the
    /// EMPTY sequence (the canonical empty-SHA256 constant). NULL when the
    /// workspace is not a git repo or the computation could not complete.
    pub dirty_digest: Option<String>,
    /// Evaluation profile identity `name@version` (name = declared `name:`
    /// or the config file stem); NULL when no profile is loaded.
    pub profile: Option<String>,
    /// Participating role definitions as sorted `slug@version` (the latest
    /// stored version of every role at execution time).
    pub roles: Vec<String>,
}

impl WorkspaceRevision {
    /// True when the revision can support a freshness claim: HEAD and the
    /// dirty digest are both recorded (a non-git workspace has no comparable
    /// tree state and must never be reported fresh).
    pub fn is_comparable(&self) -> bool {
        self.git_head.is_some() && self.dirty_digest.is_some()
    }
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct RoleExecution {
    pub id: Id,
    pub task_id: Id,
    pub role_slug: String,
    pub role_version: u32,
    pub agent_name: Option<String>,
    pub status: ExecutionStatus,
    /// Artifact ids given as inputs to the execution.
    pub inputs: Vec<String>,
    pub summary: Option<String>,
    pub error: Option<String>,
    pub started_at: Timestamp,
    pub finished_at: Option<Timestamp>,
    /// Workspace provenance recorded at execution start. Append-only: rows
    /// written before this field existed deserialize/round-trip as NULL.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workspace_revision: Option<WorkspaceRevision>,
    /// 1-based counter of THIS attempt inside the role's response-form
    /// correction loop (OQ-FG-06: one execution row per attempt — the
    /// rejection history is platform data, not a log line). Rows written
    /// before the loop existed round-trip as 1 (the single attempt they were).
    #[serde(default = "default_attempt")]
    pub attempt: u32,
    /// Lineage: id of the FIRST attempt row of the same role stage (NULL on
    /// attempt 1 itself and on pre-loop rows). `attempt` + this link rebuild
    /// the whole loop from rows alone (`devai exec list`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent_execution: Option<Id>,
    /// WHY a failed execution failed, as DATA (REQ-FG-01 AC1): an operator
    /// distinguishes `response_form` (protocol — the reply did not pass the
    /// contract gate) from a provider/process or platform-assembly failure
    /// without parsing the error text. NULL = no recorded failure class
    /// (success, or a row written before the split existed).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure_class: Option<String>,
    /// Owner identity recorded PLATFORM-side at start (REQ-SW-03, ratified
    /// OQ-2: dedicated columns, not payload JSON — queryability of the
    /// orphan detect and bundle export outweighs the column economy). The
    /// pid of the process that OPENED the row (the spec's wording: «pid
    /// процесса, открывшего исполнение»), never supplied by a model — the
    /// same precedence as `workspace_revision` above. NULL on rows written
    /// before v7: they read as «owner unknown», never as «owner dead».
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_pid: Option<i64>,
    /// Hostname of the owner process's machine (ratified OQ-3: a host other
    /// than the current one makes liveness NOT CHECKABLE — it must render
    /// as «unknown», never as dead).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_host: Option<String>,
    /// Start time of the owner PROCESS (epoch seconds, platform-computed),
    /// not of the execution row. Ratified OQ-3 closes pid-reuse with
    /// pid + process start time — a bare `kill(pid,0)` is explicitly NOT
    /// enough, because a recycled pid would fake liveness.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub owner_process_start: Option<i64>,
}

fn default_attempt() -> u32 {
    1
}

/// The closed vocabulary of `RoleExecution.failure_class` (REQ-FG-01/03).
/// Producers write these strings; consumers read them back — never a raw
/// literal at a call site.
pub mod failure_class {
    /// The reply did not satisfy the stored reply-shape contract (invalid
    /// JSON, no extractable object, schema violations — OQ-FG-02 ratified:
    /// unparseable JSON IS the form class). Owned by the form loop.
    pub const RESPONSE_FORM: &str = "response_form";
    /// The provider/process itself failed (`claude` exit != 0, broken
    /// envelope, unknown provider). NOT the form loop's to retry on this
    /// task (OQ-FG-04 ratified: provider retries stay outside).
    pub const PROVIDER: &str = "provider";
    /// The PLATFORM produced the failure: broken stored contract schema,
    /// platform-assembled report rejected by its own gate, stamping error.
    /// Never replayed to the role (REQ-FG-03 AC2) — the role is not guilty
    /// of a shape the platform assembled.
    pub const PLATFORM: &str = "platform";
}

// ---------------------------------------------------------------------------
// Decisions, questions, events
// ---------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum Verdict {
    Approved,
    NeedsRevision,
    Rejected,
    Returned,
}

str_enum!(Verdict,
    Approved => "APPROVED",
    NeedsRevision => "NEEDS_REVISION",
    Rejected => "REJECTED",
    Returned => "RETURNED",
);

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Decision {
    pub id: Id,
    pub task_id: Id,
    /// e.g. `requirements_approval`, `acceptance`, `rework`
    pub kind: String,
    pub verdict: Verdict,
    pub actor: String,
    pub rationale: Option<String>,
    pub refs: Vec<String>,
    pub created_at: Timestamp,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum QuestionStatus {
    Open,
    Answered,
}

str_enum!(QuestionStatus,
    Open => "OPEN",
    Answered => "ANSWERED",
);

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct Question {
    pub id: Id,
    pub task_id: Id,
    pub execution_id: Option<Id>,
    pub text: String,
    pub context: Option<String>,
    pub options: Vec<String>,
    pub blocking: bool,
    pub status: QuestionStatus,
    pub answer: Option<String>,
    pub answered_by: Option<String>,
    pub answered_at: Option<Timestamp>,
    pub created_at: Timestamp,
}

/// An immutable record of a significant task event (append-only history).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct WorkflowEvent {
    pub id: Id,
    pub task_id: Option<Id>,
    pub kind: String,
    pub actor: String,
    pub payload: serde_json::Value,
    pub created_at: Timestamp,
}

/// Event before it is persisted (no id/timestamp assigned yet).
#[derive(Clone, Debug)]
pub struct EventDraft {
    pub kind: &'static str,
    pub actor: String,
    pub payload: serde_json::Value,
}

/// A blocking question before it is persisted. Like an [`EventDraft`] it is
/// carried by the [`Mutation`](crate::workflow::Mutation) that raised it, so
/// the question row and the state change land in ONE transaction — a
/// producer can commit the parked state without an answerable question just
/// as little as it can append half an event (the empty-rework gate,
/// REQ-RG-02: the loud state is only loud if the operator can act on it).
#[derive(Clone, Debug)]
pub struct QuestionDraft {
    pub id: Id,
    pub text: String,
    pub context: Option<String>,
    pub options: Vec<String>,
    pub blocking: bool,
}

pub mod decision_kind {
    /// The human merge gate (REQ-WT-07): an explicit human decision that the
    /// task's platform-created `rework/<task-id>` branch is merged into the
    /// baseline. Recorded ONLY by the human-invoked CLI command — no
    /// engine/runtime/agent path performs or records a merge.
    pub const REWORK_MERGE: &str = "rework_merge";
    /// The human approval gate: approve + its rationale (which ratifies the
    /// open questions the approved spec left) is a STORED decision row, read
    /// by the QA auditor via `list_decisions` (REQ-QAD-001; precedent
    /// 01a1a0b5-0fcf — the rationale lived only in the event payload, so the
    /// auditor scoring spec-fidelity never saw the rejected alternative).
    /// Recorded by the human-invoked CLI `task approve` only.
    pub const REQUIREMENTS_APPROVAL: &str = "requirements_approval";
    /// The empty-rework gate (REQ-RG-03): the operator's explicit ratification
    /// that a QA return needed NO new changes — `refs` carry the pair
    /// (previous report id, current report id) of exactly one rework cycle, so
    /// the ratification is never lifetime (AC4). Recorded ONLY by the
    /// human-invoked CLI `task ratify-empty-rework`; no engine/agent path
    /// creates it (the 04.11 precedent 01a1a14e-2c07: a silent no-op rework
    /// rode into QA on an unchanged commit — that passage is now gated on this
    /// stored decision).
    pub const REWORK_NOOP_RATIFIED: &str = "rework_noop_ratified";
    /// The human escape of the dependency gate (RATIFIED DECISION OQ2):
    /// resume/acceptance refusal is unconditional, BUT an operator may
    /// explicitly ratify "continue despite the open blockers" — recorded as
    /// a decision row with this kind, `refs` = the full task ids of the
    /// blockers it covers, rationale = the operator's stated reason. The
    /// decision is what releases the gate (`workflow::override_covers`),
    /// never an implicit platform-side assumption. Recorded ONLY by the
    /// human-invoked CLI `task override`.
    pub const DEPENDENCY_OVERRIDE: &str = "dependency_override";
    /// Requirements rejection rationale (verdict REJECTED). Before the live
    /// write-path grows a decision row for `task reject`, the ONE-TIME rung
    /// `005_decisions_backfill` copies historical `requirements.rejected`
    /// payload rationales into rows of this kind; the auditor's section
    /// reproduces every stored row verbatim — the verdict field says which
    /// was not an approval (resolves spec OQ: the APPROVED_DECISIONS contract
    /// is "all stored human decisions", not an approval filter).
    pub const REQUIREMENTS_REJECTION: &str = "requirements_rejection";
    /// QA return rationale (verdict RETURNED), same backfill provenance as
    /// `REQUIREMENTS_REJECTION`.
    pub const QUALITY_RETURN: &str = "quality_return";
}

pub mod event_kind {
    pub const TASK_CREATED: &str = "task.created";
    /// A typed task-to-task dependency edge was registered (REQ-07): at
    /// derivation time or retroactively, attributed and timed.
    pub const TASK_DEPENDENCY_LINKED: &str = "task.dependency.linked";
    /// An edge was SUPERSEDED — the approved removal semantics (OQ7): the
    /// row stays, its ACTIVE status flips, history remains complete.
    pub const TASK_DEPENDENCY_UNLINKED: &str = "task.dependency.unlinked";
    /// A human ratified continuing DESPITE open blocking dependencies
    /// (approved OQ2 human-escape): recorded with the same decisions
    /// mechanism as every other human gate act.
    pub const TASK_DEPENDENCY_OVERRIDDEN: &str = "task.dependency.overridden";
    pub const WORKFLOW_STARTED: &str = "workflow.started";
    pub const STATE_CHANGED: &str = "workflow.state_changed";
    pub const REQUIREMENTS_PUBLISHED: &str = "requirements.published";
    pub const REQUIREMENTS_REVISED: &str = "requirements.revised";
    pub const REVIEW_RECORDED: &str = "review.recorded";
    pub const REQUIREMENTS_APPROVED: &str = "requirements.approved";
    pub const REQUIREMENTS_REJECTED: &str = "requirements.rejected";
    pub const IMPLEMENTATION_REPORTED: &str = "implementation.reported";
    pub const QUALITY_RETURNED: &str = "quality.returned";
    pub const TASK_ACCEPTED: &str = "task.accepted";
    pub const ARTIFACTS_STALED: &str = "artifacts.staled";
    pub const QUESTION_RAISED: &str = "question.raised";
    pub const QUESTION_ANSWERED: &str = "question.answered";
    pub const TASK_PAUSED: &str = "task.paused";
    pub const TASK_RESUMED: &str = "task.resumed";
    pub const TASK_BLOCKED: &str = "task.blocked";
    pub const TASK_UNBLOCKED: &str = "task.unblocked";
    pub const TASK_CANCELLED: &str = "task.cancelled";
    pub const TASK_FAILED: &str = "task.failed";
    pub const EXECUTION_STARTED: &str = "execution.started";
    pub const EXECUTION_FINISHED: &str = "execution.finished";
    pub const EXECUTION_FAILED: &str = "execution.failed";
    /// One attempt of the response-form correction loop was rejected by the
    /// contract gate (REQ-FG-07: the loop is LOUD — attempts and the FULL
    /// violation list are events, not silent retries). Payload: execution id,
    /// attempt, the complete `violations` array with json paths.
    pub const EXECUTION_FORM_REJECTED: &str = "execution.form_rejected";
    /// The form loop exhausted its configured attempts and the task parked
    /// on a blocking question instead of a bare FAILED (OQ-FG-08: the
    /// operator receives the list of WHAT to fix). Payload: attempts and the
    /// last attempt's violations.
    pub const EXECUTION_FORM_ESCALATED: &str = "execution.form_escalated";
    pub const EXECUTION_CANCELLED: &str = "execution.cancelled";
    /// The v6→v7 single-writer migration closed duplicate RUNNING rows of
    /// one task (ratified OQ-1: newest `started_at` survives, the rest are
    /// closed FAILED). One fact event per duplicated task — actor
    /// `migration:<rung id>`, the supersession/decisions backfill discipline
    /// (REQ-003/REQ-BF: the one-time mutation is auditable, not silent).
    pub const EXECUTION_SINGLE_WRITER_REPAIR: &str = "execution.single_writer_repair";
    pub const ARTIFACT_PUBLISHED: &str = "artifact.published";
    /// Publishing a new artifact version supersedes the previous CURRENT
    /// row(s) of the same (task_id, kind) group: the status column flips,
    /// nothing is deleted. One batched event per publication that superseded
    /// at least one artifact — payload names the new version, the list of
    /// flipped ids, the kind and the task (REQ-001/REQ-002; the literal is
    /// exactly the one the task text names, design source
    /// wip/pre-t2-merge@5db7fb5 types.rs:490).
    pub const ARTIFACT_SUPERSEDED: &str = "task.artifact.superseded";
    /// The decisions backfill (rung 004) created NEW decision rows from a
    /// historical approve/reject/return event's payload rationale. One fact
    /// event per (task_id, source event kind) group — never per row —
    /// actor `migration:<rung id>`, mirroring the supersession repair-event
    /// discipline (REQ-003/004: the one-time mutation is auditable, not silent).
    pub const DECISIONS_BACKFILLED: &str = "decisions.backfilled";
    pub const EXPORTED: &str = "task.exported";
    pub const IMPORTED: &str = "task.imported";
    /// QA freshness gate: the recomputed workspace revision equals the one
    /// embedded in the CURRENT quality report — the auditor is NOT run; an
    /// explicit human decision (or `--force-reaudit`) is required.
    pub const QA_FRESH_AWAITING_DECISION: &str = "qa.fresh_awaiting_decision";
    /// A human explicitly overrode the freshness gate and demanded a re-audit.
    pub const QA_REAUDIT_FORCED: &str = "qa.reaudit_forced";
    /// The human merge gate ran (REQ-WT-07): the task's `rework/<task-id>`
    /// branch was merged into the baseline by an explicit human command.
    pub const REWORK_MERGED: &str = "rework.merged";
    /// A writing stage was isolated: the platform created/reused the task's
    /// git worktree (REQ-WT-01) and/or committed the agent's work to the
    /// task's own branch (REQ-WT-06). Platform repository facts, not judgment.
    pub const WORKTREE_PREPARED: &str = "worktree.prepared";
    pub const WORKTREE_COMMITTED: &str = "worktree.committed";
    /// The empty-rework gate fired (REQ-RG-02): a REWORK publication names the
    /// SAME platform-stamped commit as the previous implementation report —
    /// the rework captured no new work. The transition to QA is withheld; the
    /// task parks in WAITING_FOR_INPUT on a blocking question until the
    /// operator ratifies (decision `rework_noop_ratified`) or a real re-run
    /// publishes a new commit. Payload: both report ids, both commit shas and
    /// the QA-return rationale that opened the cycle.
    pub const QA_REWORK_GATE_TRIPPED: &str = "qa.rework_gate_tripped";
    /// The gate could not compare (REQ-RG-04): one side of the pair carries no
    /// stamped commit, or there is no prior report to compare against. A
    /// distinguishable outcome — this event, and NEVER a verdict claim: the
    /// transition proceeds without a "gate passed" verdict in its payload.
    pub const QA_REWORK_GATE_INCOMPARABLE: &str = "qa.rework_gate_incomparable";

    /// The COMPLETE enumerable vocabulary of kinds the platform may persist.
    /// Load-time policy validation (the REWORK delivery table in
    /// `config/workflow/rework-context.yaml`) checks user-authored event
    /// strings against this list, so a typo (`quality.retruned`) is a loud
    /// config error at load — never a trigger that silently matches nothing
    /// forever (2026-11-02 QA return, block 2). A new const above MUST be
    /// added here too; the `vocabulary.rs` test pins that it is.
    pub const ALL: &[&str] = &[
        TASK_CREATED,
        TASK_DEPENDENCY_LINKED,
        TASK_DEPENDENCY_UNLINKED,
        TASK_DEPENDENCY_OVERRIDDEN,
        WORKFLOW_STARTED,
        STATE_CHANGED,
        REQUIREMENTS_PUBLISHED,
        REQUIREMENTS_REVISED,
        REVIEW_RECORDED,
        REQUIREMENTS_APPROVED,
        REQUIREMENTS_REJECTED,
        IMPLEMENTATION_REPORTED,
        QUALITY_RETURNED,
        TASK_ACCEPTED,
        ARTIFACTS_STALED,
        QUESTION_RAISED,
        QUESTION_ANSWERED,
        TASK_PAUSED,
        TASK_RESUMED,
        TASK_BLOCKED,
        TASK_UNBLOCKED,
        TASK_CANCELLED,
        TASK_FAILED,
        EXECUTION_STARTED,
        EXECUTION_FINISHED,
        EXECUTION_FAILED,
        EXECUTION_FORM_REJECTED,
        EXECUTION_FORM_ESCALATED,
        EXECUTION_CANCELLED,
        EXECUTION_SINGLE_WRITER_REPAIR,
        ARTIFACT_PUBLISHED,
        ARTIFACT_SUPERSEDED,
        DECISIONS_BACKFILLED,
        EXPORTED,
        IMPORTED,
        QA_FRESH_AWAITING_DECISION,
        QA_REAUDIT_FORCED,
        REWORK_MERGED,
        WORKTREE_PREPARED,
        WORKTREE_COMMITTED,
        QA_REWORK_GATE_TRIPPED,
        QA_REWORK_GATE_INCOMPARABLE,
    ];
}

/// The single vocabulary of JSON field names the platform WRITES into
/// persisted structures (workflow event payloads, migration records) and
/// READS back — the same discipline as `event_kind`, so a producer and a
/// consumer of one field can never drift into two different literals
/// (QA return7 п.(г): supersession payloads must not carry raw string keys).
/// The wire strings are frozen history: changing a value here changes the
/// interpretation of already-stored records, not just code. New payload
/// fields written by the platform MUST be added here, not inlined.
pub mod json_field {
    // Artifact lifecycle event payloads (store `publish_artifact`,
    // supersession, migration backfill).
    pub const ARTIFACT_ID: &str = "artifact_id";
    pub const CURRENT_ARTIFACT_ID: &str = "current_artifact_id";
    pub const SUPERSEDED: &str = "superseded";
    pub const TASK_ID: &str = "task_id";
    pub const KIND: &str = "kind";
    pub const NAME: &str = "name";
    pub const SEQ: &str = "seq";
    pub const SHA256: &str = "sha256";
    pub const SIZE: &str = "size";
    pub const CONTRACT: &str = "contract";
    // Workflow transition payloads read back by displays and delivery.
    pub const REQUIREMENTS_SEQ: &str = "requirements_seq";
    pub const SPEC_ID: &str = "spec_id";
    pub const REASON: &str = "reason";
    pub const STALED: &str = "staled";
    pub const CAUSE: &str = "cause";
    // Empty-rework gate (REQ-RG-01/02/04): the producer-written and
    // operator-read payload keys of the gate events and the ratified
    // transition. The gate event and the later QA transition it once
    // withheld speak through these same keys — no producer may rename them.
    pub const QUESTION_ID: &str = "question_id";
    pub const GATE: &str = "gate";
    pub const VERDICT: &str = "verdict";
    pub const PREV_REPORT: &str = "prev_report";
    pub const CURRENT_REPORT: &str = "current_report";
    pub const PREV_COMMIT: &str = "prev_commit";
    pub const CURRENT_COMMIT: &str = "current_commit";
    pub const RATIFIED_BY: &str = "ratified_by";
    pub const QA_RETURN_RATIONALE: &str = "qa_return_rationale";
    // Migration (one-time repair) records.
    pub const BACKFILL: &str = "backfill";
    pub const SCRIPT_PATH: &str = "script";
    // Task dependency graph (store `link_task_dependency`, REQ-07).
    pub const DEPENDENCY_ID: &str = "dependency_id";
    pub const DEPENDS_ON_ID: &str = "depends_on_id";
    pub const DEPENDENCY_KIND: &str = "dependency_kind";
    /// Source workflow events a backfill extracted from, and the decision
    /// rows it created (rung 005 fact-event payload; the ids are also cited
    /// row-to-event via `decisions.refs`).
    pub const SOURCE_EVENTS: &str = "source_events";
    pub const DECISION_IDS: &str = "decision_ids";
    /// The human's stated reason, stored in approve/reject/return event
    /// payloads and in decision rows (the backfill reads it verbatim).
    pub const RATIONALE: &str = "rationale";
    // Response-form correction loop (REQ-FG-01/02/07): the attempt counter,
    // the full violation list with json paths, and the failure class —
    // written by the adapter hook, the escalation command and the CLI.
    pub const ATTEMPT: &str = "attempt";
    pub const VIOLATIONS: &str = "violations";
    pub const FAILURE_CLASS: &str = "failure_class";
    pub const ROLE: &str = "role";
    // Execution lifecycle payloads (store `finish_execution_classified`): the
    // REWORK delivery window cutoff reads `role` + `status` back off the
    // append-only `execution.finished` record — the producer and the reader
    // must speak one vocabulary (R-002: the store already writes both keys).
    pub const STATUS: &str = "status";
    /// The reviewer/implementation text persisted on `review.recorded` and
    /// `implementation.reported` payloads; `review.recorded`'s rejection text
    /// is read from here (rework-context.yaml: `rationale_field: summary`).
    pub const SUMMARY: &str = "summary";

    /// The COMPLETE enumerable vocabulary of payload field names the platform
    /// itself mints. Load-time policy validation checks user-authored field
    /// selectors (a trigger's `rationale_field`, `verdict_field`) against
    /// this list: a typo is a config error naming the field, never a
    /// delivery that silently reads NOTHING. A new const above MUST be added
    /// here too; the `vocabulary.rs` test pins that it is.
    pub const ALL: &[&str] = &[
        ARTIFACT_ID,
        CURRENT_ARTIFACT_ID,
        SUPERSEDED,
        TASK_ID,
        KIND,
        NAME,
        SEQ,
        SHA256,
        SIZE,
        CONTRACT,
        REQUIREMENTS_SEQ,
        SPEC_ID,
        REASON,
        STALED,
        CAUSE,
        QUESTION_ID,
        GATE,
        VERDICT,
        PREV_REPORT,
        CURRENT_REPORT,
        PREV_COMMIT,
        CURRENT_COMMIT,
        RATIFIED_BY,
        QA_RETURN_RATIONALE,
        BACKFILL,
        SCRIPT_PATH,
        DEPENDENCY_ID,
        DEPENDS_ON_ID,
        DEPENDENCY_KIND,
        SOURCE_EVENTS,
        DECISION_IDS,
        RATIONALE,
        ATTEMPT,
        VIOLATIONS,
        FAILURE_CLASS,
        ROLE,
        STATUS,
        SUMMARY,
    ];
}
