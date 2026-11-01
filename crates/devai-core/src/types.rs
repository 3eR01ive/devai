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

/// Parses a JSON array column, tolerating NULL/empty as `[]`.
pub fn json_array<T: for<'de> Deserialize<'de>>(s: Option<&str>) -> Vec<T> {
    match s {
        None | Some("") => Vec::new(),
        Some(v) => serde_json::from_str(v).unwrap_or_default(),
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
    pub const EXPORTED: &str = "task.exported";
    pub const IMPORTED: &str = "task.imported";
}
