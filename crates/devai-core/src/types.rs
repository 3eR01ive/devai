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
}

pub mod event_kind {
    pub const TASK_CREATED: &str = "task.created";
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
    pub const EXECUTION_CANCELLED: &str = "execution.cancelled";
    pub const ARTIFACT_PUBLISHED: &str = "artifact.published";
    /// Publishing a new artifact version supersedes the previous CURRENT
    /// row(s) of the same (task_id, kind) group: the status column flips,
    /// nothing is deleted. One batched event per publication that superseded
    /// at least one artifact — payload names the new version, the list of
    /// flipped ids, the kind and the task (REQ-001/REQ-002; the literal is
    /// exactly the one the task text names, design source
    /// wip/pre-t2-merge@5db7fb5 types.rs:490).
    pub const ARTIFACT_SUPERSEDED: &str = "task.artifact.superseded";
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
    // Migration (one-time repair) records.
    pub const BACKFILL: &str = "backfill";
    pub const SCRIPT_PATH: &str = "script";
}
