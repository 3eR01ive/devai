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

/// The single canonical display width of an id prefix (R1 QA: the width was
/// duplicated as a wild literal in devai-api; one source, one number).
/// Rationale carried from the CLI: UUIDv7's leading bytes encode the creation
/// timestamp, so very short prefixes collide for tasks created in the same
/// instant. 12 chars showed only the top 44 bits of the 48-bit millisecond
/// timestamp — two tasks created within the same ~16 ms window then shared
/// the display prefix, and a chained CLI reference (`task new --blocked-by
/// <just-printed id>`) failed as `resolve_task` correctly reported the
/// ambiguity (QA-observed flake of the api_cycle2 parity suite). 13 chars
/// cover the FULL millisecond value: distinct display prefixes for tasks
/// created in different milliseconds, collisions only within the same ms.
/// `resolve_task` still fails loudly on any genuine ambiguity.
pub const ID_PREFIX_CHARS: usize = 13;

/// Contract versions the PLATFORM owns, declared ONCE (rework R3): the
/// runtime assembles/publishes under these versions, the seeded schemas are
/// keyed by them, and the store-side prompt text formats them instead of
/// spelling a literal. A bump here is the only place a version changes.
pub const QUALITY_REPORT_CONTRACT: (&str, u32) = ("quality-report", 5);

/// The shape the QA agent actually answers in; the platform assembles the
/// quality report from it afterwards.
pub const ASSESSMENT_CONTRACT: (&str, u32) = ("evaluation-assessment", 1);

/// Typed accessor for the canonical short display of an id. Taking `&Uuid`
/// (not a `&str`) guarantees the canonical 36-char `to_string` form, so the
/// byte slice below is always inside an ASCII boundary — no length check at
/// call sites can be forgotten.
pub fn short_id(id: &Uuid) -> String {
    id.to_string()[..ID_PREFIX_CHARS].to_string()
}

/// Display-short of an id that already arrives as a string (API views):
/// same width as `short_id`, tolerant of already-short inputs.
pub fn short_id_str(id: &str) -> String {
    id[..ID_PREFIX_CHARS.min(id.len())].to_string()
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
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        repos: Vec<String>,
        now: Timestamp,
    ) -> Self {
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

#[derive(Deserialize, Clone, Debug)]
pub struct Task {
    pub id: Id,
    pub project_id: Option<Id>,
    pub title: String,
    pub business_description: String,
    /// The human name of the task (REQ-TN-01): produced by the analyst as a
    /// required field of the CURRENT specification, stamped onto the task at
    /// publication, overridable by the human `task.rename` operation
    /// (ratified OQ-TN-1). NEVER overwrites `title`: the formal title stays
    /// the verbatim text its creator typed (REQ-TN-01 AC2). Exactly ONE
    /// current name lives on the task — a new spec version replaces the
    /// value, it does not accumulate a list (REQ-TN-01 AC3). Legacy tasks
    /// that passed analysis before names existed keep `None` and show their
    /// title (ratified OQ-TN-2: no forced one-time rename).
    #[serde(default)]
    pub human_name: Option<String>,
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

/// The serde output of a task CARRIES THE FINISHED SIGNBOARD: `display` is
/// produced here, on the producer side, by `display_sign()` — the ONE
/// REQ-TN-03 rule. Consumers (CLI human output, the UI's typed data, any
/// future surface) render this string verbatim and never re-derive it
/// (doctrine: no read-time rederivation; ratified OQ-TN-3/4/5). The
/// formal `title` still travels alongside it for traceability, so a detail
/// view can show the creator's text next to the name (OQ-TN-4).
impl Serialize for Task {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let mut st = s.serialize_struct("Task", 12)?;
        st.serialize_field("id", &self.id)?;
        st.serialize_field("project_id", &self.project_id)?;
        st.serialize_field("title", &self.title)?;
        st.serialize_field("business_description", &self.business_description)?;
        st.serialize_field("human_name", &self.human_name)?;
        st.serialize_field("state", &self.state)?;
        st.serialize_field("return_state", &self.return_state)?;
        st.serialize_field("requirements_seq", &self.requirements_seq)?;
        st.serialize_field("lock_version", &self.lock_version)?;
        st.serialize_field("created_at", &self.created_at)?;
        st.serialize_field("updated_at", &self.updated_at)?;
        st.serialize_field("display", &self.display_sign())?;
        st.end()
    }
}

impl Task {
    pub fn new(
        title: impl Into<String>,
        business_description: impl Into<String>,
        project_id: Option<Id>,
        now: Timestamp,
    ) -> Self {
        Self {
            id: new_id(),
            project_id,
            title: title.into(),
            business_description: business_description.into(),
            human_name: None,
            state: TaskState::Draft,
            return_state: None,
            requirements_seq: 0,
            lock_version: 1,
            created_at: now,
            updated_at: now,
        }
    }

    /// THE signboard rule (REQ-TN-03, ONE implementation for every list and
    /// view — consumers render this value, they never re-derive it from
    /// artifact bodies or duplicate the rule per renderer):
    ///  * a human name exists → the name;
    ///  * no name and the analysis never happened (DRAFT) → the FIRST
    ///    SENTENCE of the stored business description (ratified OQ-TN-3:
    ///    neither the whole statement nor a mid-word cut);
    ///  * neither yields a non-blank value → the formal title.
    ///
    /// Reads only stored task fields — never an artifact body.
    pub fn display_sign(&self) -> String {
        task_display_sign(
            self.state,
            self.human_name.as_deref(),
            &self.business_description,
            &self.title,
        )
    }
}

/// The signboard rule spelled over bare fields — the same ONE rule serving
/// endpoint projections of dependency edges, where the whole `Task` row of
/// the far endpoint is not carried (REQ-TN-03: the read plane hands the
/// finished signboard to its consumers).
pub fn task_display_sign(
    state: TaskState,
    human_name: Option<&str>,
    business_description: &str,
    title: &str,
) -> String {
    if let Some(n) = human_name.filter(|n| !n.trim().is_empty()) {
        return n.to_string();
    }
    if state == TaskState::Draft {
        let first = first_sentence(business_description);
        if !first.is_empty() {
            return first;
        }
    }
    title.to_string()
}

/// The first sentence of a human statement: up to the first sentence-ending
/// punctuation (`.`, `!`, `?`, `…`) followed by whitespace or the end of the
/// text. Newlines are NOT terminators — they are ordinary whitespace that
/// gets collapsed into single spaces together with the other whitespace runs,
/// so a line break inside the first sentence never cuts it short and the
/// signboard still stays a single list line. Whitespace-only input reads as
/// an empty sentence (the caller falls through to the next rule limb).
/// Deterministic and mechanical — no model, no truncation mid-word.
pub fn first_sentence(text: &str) -> String {
    let chars: Vec<char> = text.chars().collect();
    let ends: &[char] = &['.', '!', '?', '…'];
    let mut cut = chars.len();
    for (i, c) in chars.iter().enumerate() {
        if !ends.contains(c) {
            continue;
        }
        // a terminator ends the sentence only when followed by whitespace
        // (newlines included) or by nothing at all
        match chars.get(i + 1) {
            Some(next) if !next.is_whitespace() => continue,
            _ => {
                cut = i + 1;
                break;
            }
        }
    }
    chars[..cut]
        .iter()
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
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
    /// Ratified OQ-TN-5: BOTH ends of a graph edge are signed with the
    /// human signboard (the REQ-TN-03 rule, computed once on the READ plane
    /// from stored task fields); `id` + the formal title above stay the
    /// machine-readable traceability half. Consumers render these values and
    /// never re-derive a signboard (REQ-TN-03: no read-time re-derivation
    /// per renderer, no artifact parsing for a label).
    #[serde(default)]
    pub task_display: String,
    #[serde(default)]
    pub depends_on_display: String,
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

impl ArtifactKind {
    /// The SINGLE source of the advancing-kind set: kinds whose publication
    /// carries the workflow forward (QA R4: the set was duplicated in the
    /// API compound publish, the API standalone advance and the CLI render —
    /// all three consumers now derive membership from this predicate, so a
    /// new advancing kind is declared exactly once and can never silently
    /// skip the advance in one of the consumers).
    pub fn advances_state(self) -> bool {
        matches!(
            self,
            Self::RequirementsSpec | Self::ReviewReport | Self::ImplementationReport
        )
    }

    /// Kinds derived from the approved requirements spec: they are marked
    /// STALE when the requirements are revised (the dependency implicit of
    /// the manual publish path — a domain rule, never consumer-local;
    /// QA R3 moved it out of devai-api next to its owner).
    pub fn is_downstream_of_spec(self) -> bool {
        matches!(
            self,
            Self::ReviewReport
                | Self::ImplementationPlan
                | Self::ImplementationReport
                | Self::Patch
                | Self::TestReport
                | Self::QualityReport
        )
    }
}

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
        let (name, ver) = s.rsplit_once('@').ok_or_else(|| {
            CoreError::guard(format!("contract ref must be `name@version`, got `{s}`"))
        })?;
        let version = ver
            .parse::<u32>()
            .map_err(|_| CoreError::guard(format!("contract version is not a number: `{ver}`")))?;
        Ok(Self {
            name: name.to_string(),
            version,
        })
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
    /// The DRIVER (`mock`/`claude`/`hermes`) — this store column is NOT the
    /// LLM-provider; the config vocabulary is `driver`/`llm_provider`/`model`
    /// (REQ-MS-01). The LLM-provider rides `runtime` below.
    pub provider: String,
    pub model: String,
    /// The LLM-provider for drivers that spell one (empty/`local` = not
    /// set, the driver chooses). Named `runtime` for storage-compat; the
    /// read paths that speak the config vocabulary call it `llm_provider`.
    pub runtime: String,
    /// The REASONING LEVEL of the selected model (the ratified OQ-6 answer):
    /// part of the bundle, chosen together with the LLM-provider
    /// and the model. Empty = the bundle is INCOMPLETE and a run on it does
    /// not start — it is never silently delegated to the driver's default.
    #[serde(default)]
    pub reasoning: String,
    pub created_at: Timestamp,
    /// REQ-MS-02/04: this record is the ONE marked bundle in force (a
    /// partial unique index keeps it unique). `false` = not in force.
    #[serde(default)]
    pub active: bool,
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
    /// The model-bundle STAMP of this run (REQ-MS-05): driver, LLM-provider
    /// and model captured by the PLATFORM from the agent record at start —
    /// never supplied by the caller, never rewritten afterwards (a later
    /// `model.switch` changes the agent record, not this row). `None` on rows
    /// written before the v11 rung: they read as «связка не записана», never
    /// as an empty identity and never re-derived from `agent_name`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle: Option<ExecutionBundle>,
    /// The RUN this execution belongs to (REQ-AR-04/07): one launch drives a
    /// chain of executions, and every row of that chain carries the same
    /// value. The run number is minted by the LAUNCH (`task_run` →
    /// `new_id()`), BEFORE the first execution row exists, and is handed to
    /// the runner as its own identity — it is NOT the id of the first
    /// execution (the two are different values by construction). `None` on
    /// rows written before the v14 rung or imported from an older bundle:
    /// «run identity not recorded», never a guessed one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<Id>,
}

fn default_attempt() -> u32 {
    1
}

/// The CURRENT consecutive-failure series of one (task, agent) pair
/// (task 01a1b60c, REQ-AF-05/06): a counter row, not an event edit. A
/// successful execution of the same agent ANYWHERE deletes the row — only
/// success resets the series (REQ-AF-05 AC2/AC6). `parked_at` records that
/// the threshold already parked the task, so one series never parks twice in
/// the same failure.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AgentFailureSeries {
    pub task_id: Id,
    pub agent_name: String,
    pub consecutive_count: i64,
    pub first_at: Timestamp,
    pub last_at: Timestamp,
    pub last_class: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parked_at: Option<Timestamp>,
}

/// The human surface of one task's agent failure (task 01a1b60c,
/// REQ-AF-05/08): the latest stored failure, the series and the declared
/// threshold. Words and the action come from the ONE class dictionary, never
/// from a local copy; the tail is the bounded tail, the full text stays in
/// the execution row and the `agent.failed` event (REQ-AF-09 AC3).
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AgentFailureView {
    pub agent: String,
    pub class: String,
    pub class_word: String,
    pub action: String,
    pub series_count: i64,
    pub threshold: i64,
    /// Failures counted inside the current anti-spam window of this class.
    pub window_count: i64,
    /// All actual failures of this agent ever recorded (window counters
    /// summed) — suppressed repeats are included (REQ-AF-06 AC6).
    pub total_failures: i64,
    pub first_at: Timestamp,
    pub last_at: Timestamp,
    pub parked: bool,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<i64>,
    pub tail: String,
    pub output_empty: bool,
    pub driver_log_tail: Option<String>,
    pub driver_log_note: Option<String>,
    pub message: String,
    pub basis: String,
}

/// One stored `agent.failed` counter window (task 01a1b60c, REQ-AF-01/06):
/// the full evidence of the failure that OPENED the window, plus the live
/// counter. Repeats inside the window update this row, never append events.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AgentFailureRow {
    pub id: Id,
    pub task_id: Id,
    pub execution_id: Option<Id>,
    pub agent_name: String,
    pub driver: String,
    pub llm_provider: String,
    pub model: String,
    pub class: String,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<i64>,
    pub tail: String,
    pub output_empty: bool,
    pub driver_log_tail: Option<String>,
    pub driver_log_note: Option<String>,
    pub message: String,
    pub basis: String,
    pub window_count: i64,
    pub window_started_at: Timestamp,
    pub first_at: Timestamp,
    pub last_at: Timestamp,
    pub threshold: i64,
    pub created_at: Timestamp,
}

/// Input of one recorded agent failure: everything the platform observed at
/// execution completion (REQ-AF-01 AC3). The store owns the counters, the
/// window boundary and the conditional event write.
#[derive(Clone, Debug)]
pub struct AgentFailureRecord {
    pub id: Id,
    pub task_id: Id,
    pub execution_id: Option<Id>,
    pub agent_name: String,
    pub driver: String,
    pub llm_provider: String,
    pub model: String,
    pub class: String,
    pub exit_code: Option<i32>,
    pub duration_ms: Option<i64>,
    pub tail: String,
    pub output_empty: bool,
    pub driver_log_tail: Option<String>,
    pub driver_log_note: Option<String>,
    pub message: String,
    pub basis: String,
    pub threshold: i64,
    pub window_minutes: i64,
    pub at: Timestamp,
}

/// What one recording produced (task 01a1b60c, REQ-AF-06 AC2/AC3): whether
/// the anti-spam window let the event through, the window counter, the
/// separate series counter, and the accumulated total (suppressed repeats
/// included).
#[derive(Clone, Debug)]
pub struct AgentFailureRecorded {
    pub event_written: bool,
    pub window_count: i64,
    pub window_started_at: Timestamp,
    pub series_count: i64,
    pub first_at: Timestamp,
    pub last_at: Timestamp,
    pub total_failures: i64,
}

/// The executed model bundle as STORED on one execution row (REQ-MS-01/05):
/// four independent fields — the DRIVER (`mock`/`claude`/`hermes`), the
/// LLM-provider behind it (empty = the driver chooses), the model (empty =
/// not passed) and the REASONING LEVEL (the ratified OQ-6 answer: part of the selection,
/// never defaulted). «Just provider» is never a name for these: the
/// store column `agent_instances.provider` holds the DRIVER, its `runtime`
/// column holds the LLM-provider; the config vocabulary is
/// `driver`/`llm_provider`/`model`/`reasoning`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct ExecutionBundle {
    pub driver: String,
    #[serde(default)]
    pub llm_provider: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub reasoning: String,
}

impl ExecutionBundle {
    pub fn new(driver: &str, llm_provider: &str, model: &str, reasoning: &str) -> Self {
        Self {
            driver: driver.to_string(),
            llm_provider: llm_provider.to_string(),
            model: model.to_string(),
            reasoning: reasoning.to_string(),
        }
    }

    /// The config vocabulary of the identity, for events and the snapshot.
    pub fn as_json(&self) -> serde_json::Value {
        serde_json::json!({
            (json_field::DRIVER): self.driver,
            (json_field::LLM_PROVIDER): self.llm_provider,
            (json_field::MODEL): self.model,
            (json_field::REASONING): self.reasoning,
        })
    }
}

/// The closed probe-verdict vocabulary (REQ-MS-03). `timeout` and `unknown`
/// are «не доказано» — a dead `no_credits` is never invented from them.
/// `not_probeable` is what `mock` is: no process is started for it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProbeVerdict {
    Ok,
    NoCredits,
    ModelNotFound,
    NoKey,
    Network,
    Timeout,
    Unknown,
    NotProbeable,
}

impl ProbeVerdict {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::NoCredits => "no_credits",
            Self::ModelNotFound => "model_not_found",
            Self::NoKey => "no_key",
            Self::Network => "network",
            Self::Timeout => "timeout",
            Self::Unknown => "unknown",
            Self::NotProbeable => "not_probeable",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [
            Self::Ok,
            Self::NoCredits,
            Self::ModelNotFound,
            Self::NoKey,
            Self::Network,
            Self::Timeout,
            Self::Unknown,
            Self::NotProbeable,
        ]
        .into_iter()
        .find(|v| v.as_str() == s)
    }

    /// The verdicts that PROVE the bundle dead — only these may be shown as
    /// «не работает» with a reason (REQ-MS-03/06).
    pub fn proves_dead(&self) -> bool {
        matches!(
            self,
            Self::NoCredits | Self::ModelNotFound | Self::NoKey | Self::Network
        )
    }

    /// `timeout`/`unknown`: not dead, not alive — «не доказано».
    pub fn not_proven(&self) -> bool {
        matches!(self, Self::Timeout | Self::Unknown)
    }

    pub const ALL: [Self; 8] = [
        Self::Ok,
        Self::NoCredits,
        Self::ModelNotFound,
        Self::NoKey,
        Self::Network,
        Self::Timeout,
        Self::Unknown,
        Self::NotProbeable,
    ];
}

/// The closed refusal reasons of a switch (REQ-MS-05): the platform never
/// derives one — the human picks it.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum SwitchReason {
    Manual,
    Limits,
    Error,
}

impl SwitchReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Limits => "limits",
            Self::Error => "error",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        [Self::Manual, Self::Limits, Self::Error]
            .into_iter()
            .find(|r| r.as_str() == s)
    }
}

/// Freshness of the evidence behind a snapshot status (REQ-MS-05): derived
/// from the last probe time and the DECLARED window, never stored.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Freshness {
    Fresh,
    Stale,
    Unknown,
}

impl Freshness {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Fresh => "fresh",
            Self::Stale => "stale",
            Self::Unknown => "unknown",
        }
    }
}

// ---------------------------------------------------------------------------
// The ONE operator-word dictionary of the model-bundle surface (R3 quality
// return). The API snapshot, the console mirror and the window all read
// these functions; no consumer keeps its own copy of the mapping. Wire tokens
// stay frozen (they are stored history), only the words are chosen here.
// ---------------------------------------------------------------------------

/// The factory probe policy (ASM-4): the home's `models.yaml` carries the
/// real policy; these are ONLY the defaults the loader and `ProbeRequest`
/// fall back to. One source for all three defaults (R6 quality return).
pub const DEFAULT_PROBE_PHRASE: &str = "ping";
pub const DEFAULT_PROBE_TIMEOUT_MS: u64 = 20_000;
pub const DEFAULT_PROBE_FRESH_SECS: u64 = 900;

/// The closed wire vocabulary of the snapshot's ONE status field (REQ-MS-06).
/// Producers (the API snapshot) and consumers (console, window) name these
/// tokens through here, so a new state is added in ONE place — the parity
/// test below fails if a token has no operator word.
pub mod status_state {
    pub const LIVE: &str = "live";
    pub const NOT_WORKING: &str = "not_working";
    pub const NOT_PROVEN: &str = "not_proven";
    pub const NOT_SELECTED: &str = "not_selected";
    pub const UNKNOWN: &str = "unknown";
    pub const ALL: [&str; 5] = [LIVE, NOT_WORKING, NOT_PROVEN, NOT_SELECTED, UNKNOWN];
}

/// The closed wire vocabulary of a switch's OUTCOME (REQ-MS-05): `applied`,
/// `refused_probe`, `failed`. Stored history keeps these tokens; the API,
/// the console and the tests compare through this module, never a literal.
pub mod switch_outcome {
    pub const APPLIED: &str = "applied";
    pub const REFUSED_PROBE: &str = "refused_probe";
    pub const FAILED: &str = "failed";
    pub const ALL: [&str; 3] = [APPLIED, REFUSED_PROBE, FAILED];
}

/// The closed wire vocabulary of a bundle MUTATION outcome (task 01a1be5c,
/// REQ-MC-07): `created` / `updated` / `deleted` are the applied results,
/// `refused` an attempt the platform declined (probe, duplicate, active
/// bundle) and `failed` a write that died and was rolled back. Stored
/// history keeps these tokens; the surface renders words from
/// [`bundle_outcome_word`] / [`bundle_reason_word`], never a local copy.
pub mod bundle_outcome {
    pub const CREATED: &str = "created";
    pub const UPDATED: &str = "updated";
    pub const DELETED: &str = "deleted";
    pub const REFUSED: &str = "refused";
    pub const FAILED: &str = "failed";
    pub const ALL: [&str; 5] = [CREATED, UPDATED, DELETED, REFUSED, FAILED];
}

/// One word for a bundle-mutation outcome. An unknown token is never
/// dressed as a success.
pub fn bundle_outcome_word(outcome: &str) -> &'static str {
    match outcome {
        bundle_outcome::CREATED => "создана",
        bundle_outcome::UPDATED => "изменена",
        bundle_outcome::DELETED => "удалена",
        bundle_outcome::REFUSED => "отказано",
        bundle_outcome::FAILED => "сбой записи",
        _ => "неизвестно",
    }
}

/// One word for the REASON a bundle mutation was refused (REQ-MC-04/05/07).
/// The tokens are the stored history; the sentences are the operator words
/// the section prints. A probe refusal keeps the verdict's own word through
/// [`verdict_word`].
pub fn bundle_reason_word(code: &str) -> &str {
    match code {
        "duplicate_id" => "идентификатор занят другой связкой",
        "duplicate_identity" => "такая связка уже объявлена",
        "active_bundle" => "связка действующая — сначала переключитесь",
        "probe_refused" => "проба не прошла",
        "stale_form" => "файл изменён другим окном — форма устарела",
        "write_failed" => "запись файла не удалась",
        other => failure_word(other),
    }
}

/// Which stored evidence won the status rule (REQ-MS-06): a probe fact or a
/// finished execution row. One source for the snapshot, the console and the
/// freshness word.
pub mod evidence_kind {
    pub const PROBE: &str = "probe";
    pub const EXECUTION: &str = "execution";
}

/// One status word for a snapshot `state` (REQ-MS-06: exactly one word on
/// screen). An unknown token is never dressed as live or dead.
pub fn status_word(state: &str) -> &'static str {
    match state {
        status_state::LIVE => "живая",
        status_state::NOT_WORKING => "не работает",
        status_state::NOT_PROVEN => "не доказано",
        status_state::NOT_SELECTED => "связка не выбрана",
        _ => "неизвестно",
    }
}

/// The LLM-provider as humans read it (REQ-MS-01 AC2): empty AND the neutral
/// `local` default both mean «the driver chooses». ONE source — the API, the
/// console and the window print this word, never a local copy.
pub fn llm_provider_word(value: &str) -> &str {
    if value.trim().is_empty() || value == "local" {
        "не задан — выбирает драйвер"
    } else {
        value
    }
}

/// An empty model reads «не передаётся» — no substitution (REQ-MS-01 AC2).
pub fn model_word(value: &str) -> &str {
    if value.trim().is_empty() {
        "не передаётся"
    } else {
        value
    }
}

/// An empty reasoning level reads «связка неполна»: the level is part of the
/// bundle (the ratified OQ-6 answer), so there is no driver default to show.
pub fn reasoning_word(value: &str) -> &str {
    if value.trim().is_empty() {
        "не задан — связка неполна"
    } else {
        value
    }
}

/// One failure/reason word. `manual|limits|error` and the honest
/// «причина не классифицирована» are pass-through (the operator picked that
/// token); anything unrecognised reads «причина не распознана» — never a
/// guessed cause.
pub fn failure_word(code: &str) -> &str {
    match code {
        "no_credits" => "нет кредитов",
        "model_not_found" => "модель не существует",
        "no_key" => "нет ключа провайдера",
        "network" => "сеть недоступна",
        "provider" => "отказ провайдера",
        "manual" | "limits" | "error" | "причина не классифицирована" => {
            code
        }
        _ => "причина не распознана",
    }
}

/// One word for a probe verdict: `ok` is not a failure, timeout/unknown are
/// «не доказано», mock is «не пробуется».
pub fn verdict_word(verdict: &str) -> &str {
    match verdict {
        "ok" => "успешна",
        "timeout" => "таймаут — не доказано",
        "unknown" => "не доказано",
        "not_probeable" => "не пробуется",
        other => failure_word(other),
    }
}

/// Freshness as the operator reads it. The word names WHICH evidence is
/// fresh — a run row must never be called «свежая проба» (R8 quality
/// return: the execution branch was mislabelled on screen).
pub fn freshness_word(freshness: &str, evidence: Option<&str>) -> &'static str {
    match (freshness, evidence) {
        ("fresh", Some(evidence_kind::PROBE)) => "свежая проба",
        ("fresh", Some(evidence_kind::EXECUTION)) => "свежий прогон",
        ("fresh", _) => "свежие доказательства",
        ("stale", Some(evidence_kind::PROBE)) => "проба устарела",
        ("stale", Some(evidence_kind::EXECUTION)) => "прогон устарел",
        ("stale", _) => "доказательства устарели",
        _ => "доказательств нет",
    }
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
    /// The role's owner PROCESS vanished on this host (REQ-AR-05 AC1): OOM,
    /// `kill -9`, a crashed supervisor. The platform recorded the absence
    /// itself — no human had to guess from `pgrep`.
    pub const PROCESS_VANISHED: &str = "process_vanished";
    /// The owner was gone because the HOST rebooted (REQ-AR-05 AC1): the row
    /// started before the current boot, so nothing about it can still run.
    pub const HOST_RESTARTED: &str = "host_restarted";
    /// A RUNNING row written before the platform owned processes (owner
    /// columns NULL). The ratified OQ-4 answer: such rows are closed as
    /// failures with this class — «потеряно до введения владения» — instead
    /// of blocking launches forever.
    pub const LOST_BEFORE_OWNERSHIP: &str = "lost_before_ownership";
    /// The LAUNCH itself failed — the supervisor could not be spawned or the
    /// stage could not start (REQ-AR-08/13). Distinct from «the role ran and
    /// failed»: the role never produced a result at all.
    pub const SPAWN: &str = "spawn";

    /// The closed vocabulary of classes a declaration may name as
    /// park-immediately (`process.brakes.park_immediately`) — an unknown name
    /// is a typo that would silently never fire, so init refuses it.
    pub const KNOWN: &[&str] = &[
        RESPONSE_FORM,
        PROVIDER,
        PLATFORM,
        PROCESS_VANISHED,
        HOST_RESTARTED,
        LOST_BEFORE_OWNERSHIP,
        SPAWN,
    ];
}

/// The closed vocabulary of STOP reasons the task history can yield (task
/// 01a1ca39, REQ-PK-06): the `brake` token a `process.parked` event carries,
/// plus the reader-side names of a parking question and of the manual holds.
/// The brake tokens are written by the producers through these constants, so
/// an event and the phrase that words it cannot drift apart; the question and
/// manual names are the reader's vocabulary (derived from the event kind).
pub mod stop_kind {
    /// A brake tripped: rounds of the same stage role without a new artifact.
    pub const ROUNDS_WITHOUT_SHIFT: &str = "rounds_without_shift";
    /// A brake tripped: launches that did not happen in a row.
    pub const CONSECUTIVE_LAUNCH_FAILURES: &str = "consecutive_launch_failures";
    /// The failure CLASS is declared to park on its first occurrence.
    pub const PARK_IMMEDIATELY: &str = "park_immediately";
    /// The agent-failure threshold parked the task on a blocking question.
    pub const AGENT_FAILURE: &str = "agent_failure";
    /// A question hold that is not an agent failure (form loop, human ask).
    pub const QUESTION: &str = "question";
    /// The human blocked / paused / cancelled / failed the task by hand.
    pub const MANUAL_BLOCK: &str = "manual_block";
    pub const MANUAL_PAUSE: &str = "manual_pause";
    pub const MANUAL_CANCEL: &str = "manual_cancel";
    pub const MANUAL_FAIL: &str = "manual_fail";

    /// Every kind a status read must be able to word — the declaration is
    /// checked against this list, so a missing phrase is a loud config error
    /// and a typo'd key never becomes an inert row (REQ-PK-03 AC3/AC4).
    pub const ALL: &[&str] = &[
        ROUNDS_WITHOUT_SHIFT,
        CONSECUTIVE_LAUNCH_FAILURES,
        PARK_IMMEDIATELY,
        AGENT_FAILURE,
        QUESTION,
        MANUAL_BLOCK,
        MANUAL_PAUSE,
        MANUAL_CANCEL,
        MANUAL_FAIL,
    ];
}

/// The closed vocabulary of the `cause` token on `artifacts.staled` events —
/// WHY an artifact stopped being CURRENT. The workflow WRITES these strings
/// and the report presentation READS them back to word its «устарел» marker,
/// so the token text directly fixes what the operator is told: one literal on
/// either side is how the marker silently stops naming the stored cause
/// (rework R2, criterion arch/no-magic). Wire strings are frozen history —
/// changing a value here changes the reading of already-stored events.
pub mod stale_cause {
    /// `workflow::return_to_rework` retired the quality report the human
    /// rejected (ratified OQ-QH-3): the return is a DATA change, not only a
    /// presentation one.
    pub const REWORK_RETURN: &str = "rework: quality report returned by the human";
    /// A requirements revision superseded the spec the result was built on;
    /// the stale closure retired its downstream artifacts.
    pub const REQUIREMENTS_REVISION: &str = "requirements revision";
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

impl Verdict {
    /// The operator-facing verdict vocabulary: `Verdict::parse` (from the
    /// str_enum) covers the canonical SCREAMING_SNAKE form; humans and CLI
    /// arguments type lowercase synonyms. The dictionary is domain
    /// vocabulary, owned here beside `parse` (QA R3: it lived in
    /// devai-api, which declares it owns NO business rules). Consumers map
    /// `None` to their own machine-coded refusal.
    pub fn from_token(s: &str) -> Option<Verdict> {
        Some(match s.to_ascii_lowercase().as_str() {
            "approved" | "approve" | "pass" => Self::Approved,
            "needs_revision" | "revision" | "changes" => Self::NeedsRevision,
            "rejected" | "reject" => Self::Rejected,
            "returned" | "return" => Self::Returned,
            _ => return None,
        })
    }

    /// The refusal hint listing the accepted canonical tokens — DERIVED
    /// from the enum itself (primary token per variant, lowercase) so the
    /// hint cannot drift from the vocabulary it documents.
    pub fn token_hint() -> String {
        Self::ALL
            .iter()
            .map(|v| v.as_str().to_ascii_lowercase())
            .collect::<Vec<_>>()
            .join("|")
    }
}

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
    /// Kind-specific structured data stored WITH the decision row (schema
    /// v10). `profile_ratification` keeps its `<project, file, edition,
    /// items>` binding here so `task decisions` and every consumer read the
    /// ratified EDITION from the row itself instead of re-deriving it from
    /// the rules file at read time (no-read-time-rederivation). `Null` for
    /// kinds that carry no payload — the historic row shape.
    #[serde(default)]
    pub payload: serde_json::Value,
}

/// The `payload` of a `profile_ratification` decision (task 01a1bacf,
/// RV1/RV2/RV6): WHAT was ratified — one project, one file edition
/// fingerprint, the covered item ids — and, for a revocation row, which
/// APPROVED ratification it took out of force.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ProfileRatification {
    /// `projects.id` the ratification belongs to (never inferred from the
    /// carrier task — ASM-2).
    pub project_id: String,
    /// The rules file, relative to the config directory
    /// (`profiles/<project_id>.overlay.yaml`).
    pub file: String,
    /// Fingerprint of the ratified EDITION: `sha256:<hex>` over the rule
    /// content with the ratification pointers stripped (ASM-1).
    pub edition: String,
    /// The covered overlay item ids (full list, no shorthand).
    pub items: Vec<String>,
    /// Set on a REVOCATION row: the APPROVED decision it takes out of force.
    #[serde(default)]
    pub revokes: Option<String>,
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
    /// Profile overlay ratification (REQ-SP-301 schema, load-side mechanism
    /// already in C1 per REQ-SP-106/A-5): a human decision confirming one
    /// overlay item (or a section of items) — `payload` carries the project,
    /// the rules file, the EDITION fingerprint and the covered item ids, and
    /// ONLY rows of this kind with verdict APPROVED that no later revocation
    /// cites give a `profiles/<projects.id>.overlay.yaml` item force. The
    /// read-side index is `config::ratification_index`; the human surface is
    /// the `profile ratify` / `profile revoke` CLI (task 01a1bacf), which is
    /// the ONLY writer of this kind. A distinct kind (not
    /// requirements_approval/rework_merge) keeps the decision history
    /// distinguishable by purpose (spec v3 RV5 note).
    pub const PROFILE_RATIFICATION: &str = "profile_ratification";
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
    /// REQ-BIND-02 (C5 D1): the OWNING project of an existing task was
    /// assigned/replaced by the `task.project.set` operation. One append-only
    /// event per EFFECTIVE change — an idempotent re-apply of the same
    /// project writes nothing. Payload: the new `project_id` and the
    /// `previous_project_id` (null when the task was projectless).
    pub const TASK_PROJECT_SET: &str = "task.project.set";
    /// REQ-TN (ratified OQ-TN-1): the human signboard of a task was set or
    /// replaced by the HUMAN `task.rename` operation. Publication of a
    /// CURRENT spec writes the name through `requirements.published` /
    /// `requirements.revised` (the payload carries `human_name` there); this
    /// event is the manual half — one append-only event per EFFECTIVE
    /// change. Payload: the new `human_name`.
    pub const TASK_HUMAN_NAME_SET: &str = "task.human_name.set";
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
    /// The platform LAUNCHED a role in its own detached process (REQ-AR-01/
    /// 04). Payload: run id, execution id, owner pid/host/start, agent.
    pub const EXECUTION_LAUNCHED: &str = "execution.launched";
    /// A LAUNCH did not happen (REQ-AR-13): spawn failure, unusable stage,
    /// skipped by a brake. Distinct from `execution.failed` — the role never
    /// produced a result. Payload: `{failure_class, consecutive, reason,
    /// parked}` — the counter does NOT reset by itself.
    pub const EXECUTION_LAUNCH_FAILED: &str = "execution.launch_failed";
    /// The platform PROVED an owner process gone (REQ-AR-05 AC1) and closed
    /// its RUNNING row itself: OOM, `kill -9`, a reboot. Payload: id, the
    /// class (`process_vanished` / `host_restarted` /
    /// `lost_before_ownership`) and the evidence the reader used.
    pub const EXECUTION_LOST: &str = "execution.lost";
    /// A declared brake tripped (REQ-AR-09/13): the task is parked with the
    /// reason and the counter; nothing auto-continues from here.
    pub const PROCESS_PARKED: &str = "process.parked";
    /// A best-effort evidence write of the run did NOT land (rework R5): the
    /// settled outcome a `--wait` caller reads back. The role's result is not
    /// lost — the rows still explain the run — but the failure is a FACT with
    /// the path and the reason, never silence. Payload: `{run_id, path,
    /// reason, what}`.
    pub const EXECUTION_EVIDENCE_UNWRITTEN: &str = "execution.evidence_unwritten";
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
    /// One record of an AGENT failure at execution completion (task
    /// 01a1b60c, REQ-AF-01/06): class from the closed list, agent/driver/
    /// LLM-provider/model, exit code, duration, the bounded tail of the
    /// captured output, the emptiness flag, the driver-journal evidence and
    /// the anti-spam counters. Written by the PLATFORM on completion, not by
    /// the launching loop; a repeat of the same class inside the declared
    /// window increments the counter instead of appending another event
    /// (REQ-AF-06).
    pub const AGENT_FAILED: &str = "agent.failed";
    /// The v6→v7 single-writer migration closed duplicate RUNNING rows of
    /// one task (ratified OQ-1: newest `started_at` survives, the rest are
    /// closed FAILED). One fact event per duplicated task — actor
    /// `migration:<rung id>`, the supersession/decisions backfill discipline
    /// (REQ-003/REQ-BF: the one-time mutation is auditable, not silent).
    pub const EXECUTION_SINGLE_WRITER_REPAIR: &str = "execution.single_writer_repair";
    /// Warning FACT (R5 / ratified OQ-7, provider `hermes`): the run was
    /// given a platform evidence dir and expected a `--usage-file` report,
    /// but `usage.json` is absent afterwards. Recorded as an observable
    /// execution fact, NEVER silence — the whole point of collecting usage
    /// is cost as a witnessed field of the execution. Payload: execution
    /// id, provider, the expected `usage_file` path.
    pub const EXECUTION_USAGE_MISSING: &str = "execution.usage_missing";
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
    /// A human ratified a PROJECT RULES EDITION (task 01a1bacf, RV5): one
    /// append-only fact per ratification act, payload carrying the decision
    /// id, the project, the file, the edition fingerprint and the covered
    /// item ids. Distinct from `requirements.approved` by kind AND payload:
    /// ratifying project rules never approves a task's spec.
    pub const PROFILE_RATIFIED: &str = "profile.ratified";
    /// A human took a ratification out of force (RV6): payload names the
    /// revoking decision and the revoked one. The rules file is not touched.
    pub const PROFILE_REVOKED: &str = "profile.revoked";
    /// One PROBE of a declared model bundle (REQ-MS-03/05): identity,
    /// verdict from the closed vocabulary, latency, time and the stored
    /// evidence (exit code, bounded output fragment). No task is involved —
    /// a probe is a fact about a BUNDLE, not about a run.
    pub const MODEL_PROBE: &str = "model.probe";
    /// One SWITCH attempt of the platform's model bundle (REQ-MS-04/05):
    /// actor, from/to identities, the probe it rested on, the outcome
    /// (`applied`/`refused_probe`), the refusing verdict, the human-chosen
    /// reason (`manual`/`limits`/`error`) and whether the agent NAME changed.
    pub const MODEL_SWITCHED: &str = "model.switched";
    /// One CREATE attempt of a declared model bundle from the window
    /// (task 01a1be5c, REQ-MC-01/02/07): the identity written, the probe it
    /// rested on, the outcome (`created`/`refused`/`failed`) and the stored
    /// file revision. A refusal is an attempt too — the history never hides
    /// the tries that wrote nothing.
    pub const MODEL_BUNDLE_CREATED: &str = "model.bundle_created";
    /// One EDIT attempt of a declared model bundle (REQ-MC-03/07): `from` is
    /// the identity before, `bundle` the identity after, plus the probe and
    /// the outcome. An edit that was the marked record in force carries the
    /// agent-name change flag (the record keeps the marker).
    pub const MODEL_BUNDLE_UPDATED: &str = "model.bundle_updated";
    /// One DELETE attempt of a declared model bundle (REQ-MC-05/07): the
    /// identity removed (or the refusal — an active bundle is never removed).
    pub const MODEL_BUNDLE_DELETED: &str = "model.bundle_deleted";

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
        TASK_PROJECT_SET,
        TASK_HUMAN_NAME_SET,
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
        EXECUTION_LAUNCHED,
        EXECUTION_LAUNCH_FAILED,
        EXECUTION_LOST,
        PROCESS_PARKED,
        EXECUTION_EVIDENCE_UNWRITTEN,
        EXECUTION_FORM_REJECTED,
        EXECUTION_FORM_ESCALATED,
        EXECUTION_CANCELLED,
        EXECUTION_SINGLE_WRITER_REPAIR,
        EXECUTION_USAGE_MISSING,
        AGENT_FAILED,
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
        PROFILE_RATIFIED,
        PROFILE_REVOKED,
        MODEL_PROBE,
        MODEL_SWITCHED,
        MODEL_BUNDLE_CREATED,
        MODEL_BUNDLE_UPDATED,
        MODEL_BUNDLE_DELETED,
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
    /// The id of the stored DECISION a human gate event cites.
    pub const DECISION: &str = "decision";
    /// The APPROVED ratification a `profile_ratification` revocation row
    /// takes out of force (RV6): one vocabulary for the stored payload, the
    /// force index and the task history event.
    pub const REVOKES: &str = "revokes";
    pub const QA_RETURN_RATIONALE: &str = "qa_return_rationale";
    // Migration (one-time repair) records.
    pub const BACKFILL: &str = "backfill";
    pub const SCRIPT_PATH: &str = "script";
    // Task dependency graph (store `link_task_dependency`, REQ-07).
    pub const DEPENDENCY_ID: &str = "dependency_id";
    pub const DEPENDS_ON_ID: &str = "depends_on_id";
    pub const DEPENDENCY_KIND: &str = "dependency_kind";
    /// Project binding (REQ-BIND-02, store `set_task_project`): the payload
    /// keys of the `task.project.set` event — the project the task is bound
    /// to NOW and the one it was bound to BEFORE (null for a projectless
    /// task). Producer and reader speak one vocabulary.
    pub const PROJECT_ID: &str = "project_id";
    pub const PREVIOUS_PROJECT_ID: &str = "previous_project_id";
    /// Human task name (REQ-TN, task 01a1ad1d): the signboard written by the
    /// `task.human_name.set` event and by the requirements publication
    /// payloads. Producer and reader speak this one literal.
    pub const HUMAN_NAME: &str = "human_name";
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
    // Model bundles (task 01a1b71f, REQ-MS-01/05/09): the FOUR identity
    // fields are spelled driver/llm_provider/model/reasoning — «provider»
    // alone never names them, because the store's `agent_instances.provider`
    // is the DRIVER. `reasoning` is the level of thinking, part of the
    // selection (ratified OQ-6 answer).
    pub const DRIVER: &str = "driver";
    pub const LLM_PROVIDER: &str = "llm_provider";
    pub const MODEL: &str = "model";
    pub const REASONING: &str = "reasoning";
    /// Probe/switch event payloads: the bundle id, the verdict from the
    /// closed vocabulary, the measured latency, the stored evidence and the
    /// switch outcome vocabulary (`applied`/`refused_probe`).
    pub const BUNDLE_ID: &str = "bundle_id";
    pub const BUNDLE: &str = "bundle";
    pub const LATENCY_MS: &str = "latency_ms";
    pub const EXIT_CODE: &str = "exit_code";
    pub const OUTPUT_EXCERPT: &str = "output_excerpt";
    pub const OUTCOME: &str = "outcome";
    pub const FROM: &str = "from";
    pub const TO: &str = "to";
    pub const PROBE_EVENT: &str = "probe_event";
    pub const AGENT_CHANGED: &str = "agent_changed";
    /// The agent record NAME of a bundle identity snapshot (task 01a1be5c,
    /// REQ-MC-03/07): an edit may move it, so the history names it beside
    /// the four driver/model fields.
    pub const AGENT: &str = "agent";
    pub const FRESHNESS: &str = "freshness";
    /// The declared `models.yaml` revision a bundle mutation was based on
    /// (task 01a1be5c, REQ-MC-06 AC6): the history names the file edition
    /// the action wrote, so «who changed the file» stays answerable.
    pub const REVISION: &str = "revision";
    /// The STEP of a `failed` switch (REQ-MS-05: «шаг отказа»): the
    /// transaction that rolled back names the step it died on. Written only
    /// on the `failed` outcome — an applied/refused switch carries no step.
    pub const FAILURE_STEP: &str = "failure_step";
    // Agent failures (task 01a1b60c, REQ-AF-01/02/06): the class word, the
    // bounded error tail, the emptiness flag, the driver-journal evidence,
    // the basis of the class, the duration, and the anti-spam counters —
    // one vocabulary for the event producer and every read surface.
    pub const CLASS: &str = "class";
    pub const TAIL: &str = "tail";
    pub const OUTPUT_EMPTY: &str = "output_empty";
    pub const DRIVER_LOG_TAIL: &str = "driver_log_tail";
    pub const DRIVER_LOG_NOTE: &str = "driver_log_note";
    pub const BASIS: &str = "basis";
    /// The driver process's own failure message (bounded on the event; the
    /// full text stays on the execution row).
    pub const MESSAGE: &str = "message";
    pub const DURATION_MS: &str = "duration_ms";
    pub const WINDOW_COUNT: &str = "window_count";
    pub const WINDOW_STARTED_AT: &str = "window_started_at";
    pub const SERIES_COUNT: &str = "series_count";
    pub const FAILURES_TOTAL: &str = "failures_total";
    pub const FIRST_AT: &str = "first_at";
    pub const LAST_AT: &str = "last_at";
    pub const THRESHOLD: &str = "threshold";
    /// The observation time of the failure itself.
    pub const AT: &str = "at";
    // The STOP surface (task 01a1ca39, REQ-PK-01/02/06): the brake token a
    // `process.parked` event carries, the counter it tripped and the declared
    // limit, and the failed-launch facts — the consecutive counter, its limit
    // and whether THAT event parked the task. The producers
    // (`launch_brakes::commit_park` / `record_launch_failure`) and the reader
    // (`devai_api::stop`) speak these SAME consts: one re-spelled literal on
    // either side is how a stop block silently empties.
    pub const BRAKE: &str = "brake";
    pub const COUNTER: &str = "counter";
    pub const LIMIT: &str = "limit";
    pub const PARKED: &str = "parked";
    pub const CONSECUTIVE: &str = "consecutive";

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
        DECISION,
        REVOKES,
        QA_RETURN_RATIONALE,
        BACKFILL,
        SCRIPT_PATH,
        DEPENDENCY_ID,
        DEPENDS_ON_ID,
        DEPENDENCY_KIND,
        PROJECT_ID,
        PREVIOUS_PROJECT_ID,
        HUMAN_NAME,
        SOURCE_EVENTS,
        DECISION_IDS,
        RATIONALE,
        ATTEMPT,
        VIOLATIONS,
        FAILURE_CLASS,
        ROLE,
        STATUS,
        SUMMARY,
        DRIVER,
        LLM_PROVIDER,
        MODEL,
        REASONING,
        BUNDLE_ID,
        BUNDLE,
        LATENCY_MS,
        EXIT_CODE,
        OUTPUT_EXCERPT,
        OUTCOME,
        FROM,
        TO,
        PROBE_EVENT,
        AGENT_CHANGED,
        AGENT,
        FRESHNESS,
        REVISION,
        FAILURE_STEP,
        CLASS,
        BRAKE,
        COUNTER,
        LIMIT,
        PARKED,
        CONSECUTIVE,
        TAIL,
        OUTPUT_EMPTY,
        DRIVER_LOG_TAIL,
        DRIVER_LOG_NOTE,
        BASIS,
        MESSAGE,
        DURATION_MS,
        WINDOW_COUNT,
        WINDOW_STARTED_AT,
        SERIES_COUNT,
        FAILURES_TOTAL,
        FIRST_AT,
        LAST_AT,
        THRESHOLD,
        AT,
    ];
}

#[cfg(test)]
mod word_tests {
    use super::*;

    /// R3 quality return: ONE dictionary backs the API, the console and the
    /// window. The stored tokens never drift; unknown input is never guessed.
    #[test]
    fn the_failure_words_are_one_closed_dictionary() {
        assert_eq!(failure_word("no_credits"), "нет кредитов");
        assert_eq!(failure_word("model_not_found"), "модель не существует");
        assert_eq!(failure_word("no_key"), "нет ключа провайдера");
        assert_eq!(failure_word("network"), "сеть недоступна");
        assert_eq!(failure_word("provider"), "отказ провайдера");
        // The human-chosen reason tokens are pass-through, not re-worded.
        assert_eq!(failure_word("manual"), "manual");
        assert_eq!(failure_word("limits"), "limits");
        assert_eq!(failure_word("error"), "error");
        assert_eq!(
            failure_word("причина не классифицирована"),
            "причина не классифицирована"
        );
        // Not proven = not named.
        assert_eq!(failure_word("самопридуманное"), "причина не распознана");
    }

    #[test]
    fn every_state_and_freshness_has_one_word() {
        for (token, word) in [
            (status_state::LIVE, "живая"),
            (status_state::NOT_WORKING, "не работает"),
            (status_state::NOT_PROVEN, "не доказано"),
            (status_state::NOT_SELECTED, "связка не выбрана"),
            (status_state::UNKNOWN, "неизвестно"),
            ("whatever", "неизвестно"),
        ] {
            assert_eq!(status_word(token), word);
        }
        // The word names WHICH evidence is fresh — a run is not a probe.
        assert_eq!(
            freshness_word("fresh", Some(evidence_kind::PROBE)),
            "свежая проба"
        );
        assert_eq!(
            freshness_word("fresh", Some(evidence_kind::EXECUTION)),
            "свежий прогон"
        );
        assert_eq!(
            freshness_word("stale", Some(evidence_kind::PROBE)),
            "проба устарела"
        );
        assert_eq!(
            freshness_word("stale", Some(evidence_kind::EXECUTION)),
            "прогон устарел"
        );
        assert_eq!(freshness_word("unknown", None), "доказательств нет");
    }

    /// R2 quality return: the dictionaries are the ONLY source of the wire
    /// tokens. Adding a status without a word (or a word without a token)
    /// breaks this parity test instead of drifting surface by surface.
    #[test]
    fn the_status_dictionary_is_covered_by_the_word_function() {
        for state in status_state::ALL
            .iter()
            .filter(|s| **s != status_state::UNKNOWN)
        {
            let word = status_word(state);
            assert!(!word.is_empty(), "status `{state}` has no operator word");
            assert_ne!(
                word,
                status_word("token-outside-the-dictionary"),
                "status `{state}` falls through to the unknown word (only `unknown` may)"
            );
        }
        // The unknown token IS the honest fallback word — nothing else is.
        assert_eq!(
            status_word(status_state::UNKNOWN),
            status_word("token-outside-the-dictionary")
        );
        // Distinct tokens must not collapse into one word on screen.
        let mut words: Vec<&str> = status_state::ALL.iter().map(|s| status_word(s)).collect();
        words.sort_unstable();
        words.dedup();
        assert_eq!(words.len(), status_state::ALL.len());
    }

    /// The switch outcomes are a closed set: the API writes only these, the
    /// console compares through them.
    #[test]
    fn the_switch_outcome_vocabulary_is_closed() {
        assert_eq!(switch_outcome::ALL, ["applied", "refused_probe", "failed"]);
        let mut seen = switch_outcome::ALL.to_vec();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), switch_outcome::ALL.len());
    }

    /// One source of the empty-field words (R5 quality return): the API and
    /// the console both print these, so the words are pinned here once.
    #[test]
    fn the_empty_bundle_fields_read_as_words_not_defaults() {
        for empty in ["", "   ", "local"] {
            assert_eq!(llm_provider_word(empty), "не задан — выбирает драйвер");
        }
        assert_eq!(llm_provider_word("deepseek"), "deepseek");
        assert_eq!(model_word(""), "не передаётся");
        assert_eq!(model_word("m-1"), "m-1");
        assert_eq!(reasoning_word(""), "не задан — связка неполна");
        assert_eq!(reasoning_word("high"), "high");
    }

    /// The factory probe policy is ONE source: the loader and the probe
    /// request both read these consts (R6 quality return).
    #[test]
    fn the_probe_defaults_are_the_documented_policy() {
        assert_eq!(DEFAULT_PROBE_PHRASE, "ping");
        assert_eq!(DEFAULT_PROBE_TIMEOUT_MS, 20_000);
        assert_eq!(DEFAULT_PROBE_FRESH_SECS, 900);
    }
}
