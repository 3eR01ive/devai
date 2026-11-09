//! The command plane of devai-ui: every data the window renders is one
//! published READ operation of the API registry (REQ-UI-02). The frontend
//! never receives anything else — there is no store handle reachable from
//! this crate, and no domain verdict is re-derived here: typed views are
//! passed through exactly as `devai-api` produced them
//! (store-is-cache / no read-time rederivation).
//!
//! The UI is read-only in this iteration (OQ-UI-6 ratified): no command
//! here touches a Mutation-class operation. The mapping table
//! `COMMAND_OPERATIONS` + its test pin that dogmatically.

use serde::Serialize;
use tauri::State;

use devai_api::{ActiveSnapshot, DependenciesView, ProgressView, QualityReportView, StatusView};
use devai_api::{Api, ApiError, OpDescriptor};
use devai_api::{ConvergenceView, InboxSnapshotView, InboxTaskFacts, SpecBodyView, SpecFacts};
use devai_api::{WorkspaceChangesView, WorkspaceDiffView};
use devai_core::inbox::{
    InboxWaitPolicy, WHEN_NO_QUESTION, WHEN_PARKED_QUESTION, WaitGroup, WaitStatePolicy,
};
use devai_core::types::{
    Artifact, Decision, Project, Question, QuestionStatus, RoleExecution, Task, TaskState, short_id,
};

use crate::session::SessionInfo;

/// Shared state: the open `Api` session behind a mutex (the API takes
/// `&self`; the store is WAL — concurrent readers with the engine are the
/// ratified shape, REQ-UI-12), plus the facts for the status bar.
pub struct AppState {
    pub api: std::sync::Mutex<Api>,
    pub session: SessionInfo,
}

impl AppState {
    fn api(&self) -> std::sync::MutexGuard<'_, Api> {
        // Poisoned only if a command panicked mid-call; the reads leave no
        // mutation, so the handle is still sound.
        self.api.lock().unwrap_or_else(|p| p.into_inner())
    }
}

/// Machine-readable error surface forwarded to the frontend: `code()` from
/// the API's programmatic error contract (REQ-API-06 AC3) plus the platform
/// text — the UI branches on the code and never parses messages
/// (e.g. `not_found` on task.report is shown as the platform's own hint,
/// REQ-UI-07 AC2).
#[derive(Serialize, Clone, Debug)]
pub struct UiError {
    pub code: String,
    pub message: String,
}

impl From<ApiError> for UiError {
    fn from(e: ApiError) -> Self {
        UiError {
            code: e.code().to_string(),
            message: e.to_string(),
        }
    }
}

/// Artifact content presentation (REQ-UI-06 AC2): the platform answers
/// `NotUtf8` for non-text blobs; the UI turns that into an honest "binary"
/// fact with the stored size — it never renders raw bytes as text.
#[derive(Serialize, Clone, Debug)]
#[serde(tag = "shape", rename_all = "lowercase")]
pub enum ArtifactContent {
    Text { artifact_id: String, text: String },
    Binary { artifact_id: String, size: u64 },
}

/// One human-wait row (REQ-GATE-01..05): a task the section lists as a
/// point where the platform — or a human decision about a stopped task —
/// needs the operator. `demand`/`cli_hint` are presentation copy keyed by
/// the EXACT stored state token (never a guess about waiting); `spec` /
/// `report` are the click-presentation, computed from published reads only.
#[derive(Serialize, Clone, Debug)]
pub struct WaitRow {
    pub task: Task,
    /// What the platform requires FROM the operator, in human words
    /// (REQ-GATE-03 AC1) — `demand`, not `wants`: it is a requirement on
    /// the person, not a property of the task.
    pub demand: String,
    /// The literal CLI command for that decision (REQ-GATE-06 AC2) — the
    /// section observes, it never acts (read-only base, OQ-UI-6).
    pub cli_hint: String,
    /// Platform registry questions of the task, stored OPEN token only:
    /// they arrive with the section's single batch read — `inbox_view` calls
    /// `Api::inbox_snapshot`, never a `question.list` composition
    /// (REQ-GATE-03 AC2).
    pub open_questions: Vec<Question>,
    /// APPROVAL presentation: the spec the human is asked to sign
    /// (REQ-GATE-04/05 + ratified OQ-GATE-3/4).
    pub spec: Option<SpecPresentation>,
    /// QA presentation: the CURRENT quality report — the same view the
    /// registered `task.report` read publishes.
    pub report: Option<QualityReportView>,
    /// Honest fact when the QA row has no signable report (platform error
    /// text, never an empty hole).
    pub report_note: Option<String>,
    /// The task's convergence slice (REQ-CV-12), identical to the object the
    /// `task.progress` read answers; the row picks the stage to draw.
    pub convergence: Option<ConvergenceView>,
}

/// The spec body as the gate presents it: the human paragraph and the
/// document's own open questions, kept apart from platform questions
/// (REQ-GATE-05 — two different classes under different names).
#[derive(Serialize, Clone, Debug)]
pub struct SpecPresentation {
    pub artifact: Artifact,
    /// false when there is no CURRENT REQUIREMENTS_SPEC and this is the
    /// LAST spec artifact shown with an honest label (ratified OQ-GATE-3:
    /// not a silent hole, not a fake — the approve is impossible).
    pub current: bool,
    pub human_paragraph: Option<String>,
    /// Which text stands in the paragraph slot (ratified OQ-GATE-4).
    pub paragraph_source: ParagraphSource,
    /// The REQUIREMENTS_SPEC `open_questions` array — questions of the
    /// document, NOT registry questions; they have no question id and are
    /// not answerable via question.answer (REQ-GATE-05 AC2).
    pub open_questions: Vec<String>,
    /// Why the paragraph could not come from the spec body (absent field,
    /// binary artifact, unparsable JSON) — the honest fact, if any.
    pub unavailable_note: Option<String>,
}

/// Origin of the paragraph text shown for an APPROVAL row.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ParagraphSource {
    /// The spec's own top-level `human_paragraph` (contract @3).
    Spec,
    /// Ratified OQ-GATE-4: the spec predates the paragraph protocol (or
    /// its body is unreadable) — the stored task business description
    /// stands in, labelled as such. The presentation is never empty.
    BusinessDescription,
}

/// One informational row of the reference group (ratified OQ-GATE-1):
/// an unanswered NON-BLOCKING question — a note on an active task
/// (workflow.rs:747-748), where the platform does NOT stand. Shown as a
/// separate reference group below the waiting list, never as a wait row —
/// but it IS a row of the section, so the badge counts it
/// (badge == waiting + human_stopped + notes, REQ-GATE-08 AC3).
#[derive(Serialize, Clone, Debug)]
pub struct WaitNote {
    pub task: Task,
    pub question: Question,
}

/// The whole human-wait section as ONE snapshot: membership, ordering and
/// the badge are computed together, so the badge always equals the number
/// of section rows on the same data (REQ-GATE-08 AC3) and nothing is
/// cached between loads (REQ-GATE-07).
#[derive(Serialize, Clone, Debug)]
pub struct WaitSnapshot {
    /// Points where the platform stands on the human: APPROVAL,
    /// WAITING_FOR_INPUT, QA and FAILED (ratified OQ-GATE-2), sorted by
    /// task.updated_at ascending — the longest-waiting first (REQ-GATE-08).
    pub waiting: Vec<WaitRow>,
    /// Separate bottom group: tasks the OPERATOR stopped (PAUSED/BLOCKED)
    /// — "там стоит не платформа, а выбор оператора" (OQ-GATE-2).
    pub human_stopped: Vec<WaitRow>,
    /// Reference group of unanswered non-blocking OPEN questions.
    pub notes: Vec<WaitNote>,
    /// Badge on the nav element (REQ-GATE-08 AC3): the number of rows of
    /// the section on the same snapshot — waiting + human-stopped + notes,
    /// since the ratified OQ-GATE-1 shows the notes as real rows.
    pub badge: usize,
}

/// The section's membership predicate, read from the DECLARATIVE table
/// (`config/workflow/inbox-waits.yaml`, loaded through `inbox.snapshot`):
/// the stored state token selects its row, the row names the group. No
/// `match` arm per state lives in code any more (the demand table, once a
/// 62-line match, is data now); a state that has no row is `None`, and the
/// loader refuses a table that misses a `TaskState` variant (so a new
/// variant cannot silently drop out of the section).
pub fn wait_group_of(policy: &InboxWaitPolicy, state: &TaskState) -> Option<WaitGroup> {
    let row = policy.state(*state)?;
    match row.group {
        WaitGroup::NotWait => None,
        group => Some(group),
    }
}

/// Terminal states carry no live demand on the operator: their leftover
/// unanswered notes are not shown (the reference group speaks about the
/// line that could continue to need an answer). Terminality is the CORE's
/// question, not this file's: `TaskState::is_terminal` owns the rule, so a
/// new terminal variant cannot silently keep its notes visible.
pub fn is_note_visible(state: &TaskState) -> bool {
    !state.is_terminal()
}

/// The demand text + CLI hint of one wait row, rendered generically from the
/// data table (REQ-GATE-03/06): the row's unconditional wording, or the
/// variant whose stored-data predicate holds (the WAITING parked/no-question
/// split). `{id}` and `{question_id}` are substituted with stored ids, so
/// the printed command is real — never a placeholder left in print.
/// `Ok(None)` when the table does not list the state as a wait
/// state; `Err` when the table names a predicate this renderer does not
/// know — fail-closed at the render site, not only in the config validator:
/// an unknown `when` must never silently match nothing and leave the row
/// with an empty demand and an empty command.
pub fn wait_demand_for(
    policy: &InboxWaitPolicy,
    state: &TaskState,
    id: &str,
    open_questions: &[Question],
) -> Result<Option<WaitDemand>, UiError> {
    let Some(row) = policy.state(*state) else {
        return Ok(None);
    };
    if row.group == WaitGroup::NotWait {
        return Ok(None);
    }
    // Only a BLOCKING question parks the task (workflow.rs:750-763) — a
    // stray non-blocking note must never become the printed command.
    let parked = open_questions.iter().find(|q| q.blocking);
    let variant = variant_of(row, parked.is_some())?;
    let (demand, commands) = match variant {
        Some(variant) => (variant.demand.as_str(), variant.commands.as_slice()),
        None => (row.demand.as_str(), row.commands.as_slice()),
    };
    let cli_hint = commands
        .iter()
        .map(|template| substitute(template, id, parked))
        .collect::<Vec<_>>()
        .join(" · ");
    Ok(Some(WaitDemand {
        demand: demand.to_string(),
        cli_hint,
    }))
}

/// One rendered demand.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WaitDemand {
    pub demand: String,
    pub cli_hint: String,
}

/// The variant whose `when` predicate the stored data satisfies. FAIL-CLOSED:
/// a token outside [`WHEN_ALL`] is an error at the
/// render site — the config validator rejects it too, but a table that
/// reached this point (an older home, a hand-edited snapshot) must never
/// degrade into "matches nothing" and an empty demand with an empty
/// command. `Ok(None)` = the row has no conditional wording at all.
fn variant_of(
    row: &WaitStatePolicy,
    has_parked_question: bool,
) -> Result<Option<&devai_core::inbox::WaitVariant>, UiError> {
    for variant in &row.variants {
        let matched = match variant.when.as_str() {
            WHEN_PARKED_QUESTION => has_parked_question,
            WHEN_NO_QUESTION => !has_parked_question,
            unknown => {
                return Err(UiError {
                    code: "invalid_config".to_string(),
                    message: format!(
                        "inbox_waits: unknown variant predicate `{unknown}` (accepted: {}); \
                         the row cannot be rendered",
                        devai_core::inbox::WHEN_ALL.join(", ")
                    ),
                });
            }
        };
        if matched {
            return Ok(Some(variant));
        }
    }
    Ok(None)
}

/// Placeholder substitution of one command template.
fn substitute(template: &str, id: &str, parked: Option<&Question>) -> String {
    let with_id = template.replace("{id}", id);
    match parked {
        Some(question) => with_id.replace("{question_id}", &question.id.to_string()),
        None => with_id,
    }
}

/// Parse the spec artifact body into the gate presentation. The JSON of
/// REQUIREMENTS_SPEC is read, never rewritten (REQ-GATE-04 AC1: parsing
/// on the presentation side does not change data); the array shapes are
/// the seeded contract's (config seed: `open_questions` is an array of
/// strings). Ratified OQ-GATE-4: absent paragraph → the stored task
/// business description with an honest source + note.
pub fn spec_presentation(
    artifact: &Artifact,
    body: Result<&str, &str>,
    task: &Task,
    current: bool,
) -> SpecPresentation {
    match body {
        Ok(text) => document_presentation(artifact, task, current, text),
        Err(reason) => fallback_presentation(
            artifact,
            task,
            current,
            Vec::new(),
            format!("{reason}; показываю бизнес-описание задачи"),
        ),
    }
}

/// The readable-body path: the document's own paragraph when it has one,
/// else the ratified pre-protocol fallback. The document's `open_questions`
/// survive either way (they are the document's, not the platform's).
fn document_presentation(
    artifact: &Artifact,
    task: &Task,
    current: bool,
    text: &str,
) -> SpecPresentation {
    let value = match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) => value,
        Err(_) => {
            return fallback_presentation(
                artifact,
                task,
                current,
                Vec::new(),
                "тело артефакта специки не разбирается как JSON; показываю бизнес-описание задачи"
                    .to_string(),
            );
        }
    };
    let open_questions = spec_open_questions(&value);
    match spec_paragraph(&value) {
        Some(paragraph) => SpecPresentation {
            artifact: artifact.clone(),
            current,
            human_paragraph: Some(paragraph),
            paragraph_source: ParagraphSource::Spec,
            open_questions,
            unavailable_note: None,
        },
        // Ratified OQ-GATE-4, verbatim wording of the decision.
        None => fallback_presentation(
            artifact,
            task,
            current,
            open_questions,
            "аналитик абзац не писал — спека до протокола; подставляю бизнес-описание задачи"
                .to_string(),
        ),
    }
}

/// The ONE constructor of the fail-closed shape (ratified OQ-GATE-4: the
/// presentation is never empty). The three fail-closed branches — body
/// unreadable, JSON unparsable, spec predating the paragraph protocol —
/// differ only in `note`, never in the shape, so the wording cannot drift
/// as a copy.
fn fallback_presentation(
    artifact: &Artifact,
    task: &Task,
    current: bool,
    open_questions: Vec<String>,
    note: String,
) -> SpecPresentation {
    SpecPresentation {
        artifact: artifact.clone(),
        current,
        human_paragraph: non_empty(task.business_description.as_str()).map(str::to_string),
        paragraph_source: ParagraphSource::BusinessDescription,
        open_questions,
        unavailable_note: Some(note),
    }
}

/// The document's own top-level `human_paragraph`, trimmed; `None` when
/// the key is absent, empty or not a string.
fn spec_paragraph(value: &serde_json::Value) -> Option<String> {
    value
        .get("human_paragraph")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The REQUIREMENTS_SPEC `open_questions` array verbatim (strings as-is,
/// other JSON shapes stringified): questions of the document, never
/// registry questions (REQ-GATE-05 AC2).
fn spec_open_questions(value: &serde_json::Value) -> Vec<String> {
    value
        .get("open_questions")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .map(|item| match item {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn non_empty(s: &str) -> Option<&str> {
    let t = s.trim();
    if t.is_empty() { None } else { Some(t) }
}

/// Shape-contract ledger of the command plane (ux/contract-stability).
/// `COMMAND_OPERATIONS` pins WHICH published read backs each command; this
/// const pins the IPC SHAPE each command publishes, so a breaking revision
/// is a recorded, versioned fact the frontend and its `types.ts` mirror are
/// reviewed against — not an unwritten convention. `version` starts at 2
/// for a recorded breaking revision (1 = the un-versioned first shape);
/// `breaking` is a non-empty human note naming what changed and when.
#[allow(dead_code)] // the ledger is consumed by the pinning test, not at runtime
#[derive(Debug, Clone, Copy)]
pub struct CommandContract {
    pub command: &'static str,
    pub version: u32,
    pub breaking: &'static str,
}

#[allow(dead_code)] // consumed by the shape-contract test, not at runtime
pub const COMMAND_CONTRACTS: &[CommandContract] = &[CommandContract {
    command: "inbox_view",
    version: 2,
    breaking: "v2 (2026-11-06): inbox_view заменил InboxEntry[] на WaitSnapshot (REQ-GATE-01..10) — прежней формы больше нет; frontend/src/types.ts зеркалит v2.",
}];

/// The (command → registry operation) mapping. Every data command is
/// backed by a published Read operation; `session_info` is bootstrap state
/// (`Api::open`, the ratified OQ-10 exception), not an operation.
#[allow(dead_code)] // consumed by the test below + mirrors OpsView's `used` list
pub const COMMAND_OPERATIONS: &[(&str, &str)] = &[
    ("registry_operations", "api.registry"),
    ("projects_list", "project.list"),
    ("projects_get", "project.get"),
    ("tasks_list", "task.list"),
    ("task_get", "task.get"),
    ("task_status", "task.status"),
    ("task_progress", "task.progress"),
    ("task_dependencies", "task.dependencies"),
    ("task_report", "task.report"),
    ("task_events", "task.events"),
    ("task_decisions", "task.decisions"),
    ("task_active", "task.active"),
    ("task_workspace_changes", "task.workspace_changes"),
    ("task_workspace_diff", "task.workspace_diff"),
    ("artifacts_list", "artifact.list"),
    ("artifact_content", "artifact.cat_text"),
    ("artifact_content", "artifact.cat"),
    ("questions_list", "question.list"),
    ("executions_list", "exec.list"),
    ("execution_liveness", "exec.liveness"),
    // The whole human-wait section is ONE batch read:
    // `inbox.snapshot` carries the demand table, the questions and the gate
    // presentations in a single call, so the IPC cost per row is O(1).
    ("inbox_view", "inbox.snapshot"),
];

// ---------------------------------------------------------------------
// session / registry
// ---------------------------------------------------------------------

/// Bootstrap facts for the title/status bar (REQ-UI-01 AC3).
#[tauri::command]
pub fn session_info(state: State<AppState>) -> SessionInfo {
    state.session.clone()
}

/// `api.registry` — the published operation contract, shown in the UI so
/// every panel stays comparable to a registry row (REQ-UI-02 AC2).
#[tauri::command]
pub fn registry_operations(state: State<AppState>) -> Result<Vec<OpDescriptor>, UiError> {
    Ok(state.api().api_registry())
}

// ---------------------------------------------------------------------
// projects (REQ-UI-03) / tasks (REQ-UI-04)
// ---------------------------------------------------------------------

#[tauri::command]
pub fn projects_list(state: State<AppState>) -> Result<Vec<Project>, UiError> {
    Ok(state.api().project_list()?)
}

/// project.get accepts id OR name — the resolution is the API's (REQ-UI-03
/// constraint), the UI just forwards the string.
#[tauri::command(rename_all = "snake_case")]
pub fn projects_get(
    state: State<AppState>,
    id_or_name: String,
) -> Result<Option<Project>, UiError> {
    Ok(state.api().project_get(&id_or_name)?)
}

/// All tasks of the home, ordered by last update BY THE OPERATION
/// (the registered `task.list` read) — the UI re-sorts nothing.
#[tauri::command]
pub fn tasks_list(state: State<AppState>) -> Result<Vec<Task>, UiError> {
    Ok(state.api().task_list()?)
}

#[tauri::command(rename_all = "snake_case")]
pub fn task_get(state: State<AppState>, id_prefix: String) -> Result<Task, UiError> {
    Ok(state.api().task_get(&id_prefix)?)
}

// ---------------------------------------------------------------------
// task detail panels (REQ-UI-05 … REQ-UI-08, REQ-UI-10)
// ---------------------------------------------------------------------

/// StatusView is passed through verbatim: allowed_operations are exactly
/// what `workflow::allowed_ops` issued via the API (REQ-UI-05 AC2).
#[tauri::command(rename_all = "snake_case")]
pub fn task_status(state: State<AppState>, id_prefix: String) -> Result<StatusView, UiError> {
    Ok(state.api().task_status(&id_prefix)?)
}

/// PipelineView{order,current,position,position_index,off_spine,terminal}
/// — the stage order comes from the platform, never from the UI (AC1).
#[tauri::command(rename_all = "snake_case")]
pub fn task_progress(state: State<AppState>, id_prefix: String) -> Result<ProgressView, UiError> {
    Ok(state.api().task_progress(&id_prefix)?)
}

#[tauri::command(rename_all = "snake_case")]
pub fn task_dependencies(
    state: State<AppState>,
    id_prefix: String,
) -> Result<DependenciesView, UiError> {
    Ok(state.api().task_dependencies(&id_prefix)?)
}

/// The CURRENT quality report (REQ-UI-07). When QA has not run, the
/// platform's NotFound (with its API hint text) is surfaced as UiError —
/// the frontend renders it as the platform's own message, not an empty
/// screen (AC2).
#[tauri::command(rename_all = "snake_case")]
pub fn task_report(
    state: State<AppState>,
    id_prefix: String,
) -> Result<QualityReportView, UiError> {
    Ok(state.api().task_report(&id_prefix)?)
}

/// Event history, newest tail limited by the OPERATION's limit parameter
/// (REQ-UI-08 AC1).
#[tauri::command(rename_all = "snake_case")]
pub fn task_events(
    state: State<AppState>,
    id_prefix: String,
    limit: Option<usize>,
) -> Result<Vec<devai_core::types::WorkflowEvent>, UiError> {
    Ok(state.api().task_events(&id_prefix, limit)?)
}

#[tauri::command(rename_all = "snake_case")]
pub fn task_decisions(state: State<AppState>, id_prefix: String) -> Result<Vec<Decision>, UiError> {
    Ok(state.api().task_decisions(&id_prefix)?)
}

/// Cross-task RUNNING snapshot with liveness triage and the worktree link
/// where the platform recorded `worktree.prepared` (REQ-UI-10 AC1,
/// REQ-UI-11: branch/worktree arrive as ActiveRow data, never re-spelled).
#[tauri::command]
pub fn task_active(state: State<AppState>) -> Result<ActiveSnapshot, UiError> {
    Ok(state.api().task_active()?)
}

/// The committed changeset of the task's isolated workspace against the
/// stored bound baseline (REQ-WV-04): file list, status markers, the range
/// and the per-run stamps — all platform-composed, the UI re-spells
/// nothing. State verdicts arrive as data (REQ-WV-01 AC2, REQ-WV-07).
#[tauri::command(rename_all = "snake_case")]
pub fn task_workspace_changes(
    state: State<AppState>,
    id_prefix: String,
) -> Result<WorkspaceChangesView, UiError> {
    Ok(state.api().task_workspace_changes(&id_prefix)?)
}

/// The readable unified diff of ONE changeset-listed file (REQ-WV-02):
/// same baseline..tip range the list view used; a path outside the list is
/// the platform's typed not_found — the frontend branches on UiError.code,
/// never on text (AC2).
#[tauri::command(rename_all = "snake_case")]
pub fn task_workspace_diff(
    state: State<AppState>,
    id_prefix: String,
    path: String,
) -> Result<WorkspaceDiffView, UiError> {
    Ok(state.api().task_workspace_diff(&id_prefix, &path)?)
}

// ---------------------------------------------------------------------
// artifacts (REQ-UI-06)
// ---------------------------------------------------------------------

/// All artifact rows: kind, seq, stored CURRENT/STALE/SUPERSEDED status —
/// the UI displays the status as data and never re-assigns it.
#[tauri::command(rename_all = "snake_case")]
pub fn artifacts_list(state: State<AppState>, id_prefix: String) -> Result<Vec<Artifact>, UiError> {
    Ok(state.api().artifact_list(&id_prefix)?)
}

/// artifact.cat_text with the platform's NotUtf8 → artifact.cat fallback:
/// binary content is reported as a binary fact (size), never rendered.
#[tauri::command(rename_all = "snake_case")]
pub fn artifact_content(
    state: State<AppState>,
    artifact_id: String,
) -> Result<ArtifactContent, UiError> {
    let api = state.api();
    let id = artifact_id.clone();
    match api.artifact_text(&artifact_id) {
        Ok(text) => Ok(ArtifactContent::Text {
            artifact_id: id,
            text,
        }),
        Err(e) => {
            if matches!(e, ApiError::NotUtf8) {
                let bytes = api.artifact_bytes(&artifact_id)?;
                Ok(ArtifactContent::Binary {
                    artifact_id: id,
                    size: bytes.len() as u64,
                })
            } else {
                Err(e.into())
            }
        }
    }
}

// ---------------------------------------------------------------------
// questions / executions (REQ-UI-09, REQ-UI-10)
// ---------------------------------------------------------------------

#[tauri::command(rename_all = "snake_case")]
pub fn questions_list(state: State<AppState>, id_prefix: String) -> Result<Vec<Question>, UiError> {
    Ok(state.api().question_list(&id_prefix)?)
}

/// Execution rows of one task (or all): RUNNING/COMPLETED/FAILED/
/// CANCELLED tokens as stored (`ExecutionStatus` in `devai-core`).
#[tauri::command(rename_all = "snake_case")]
pub fn executions_list(
    state: State<AppState>,
    task_ref: Option<String>,
) -> Result<Vec<RoleExecution>, UiError> {
    Ok(state.api().exec_list(task_ref.as_deref())?)
}

#[tauri::command(rename_all = "snake_case")]
pub fn execution_liveness(
    state: State<AppState>,
    exec_id: String,
) -> Result<devai_api::LivenessView, UiError> {
    Ok(state.api().exec_liveness(&exec_id)?)
}

/// The human-wait section (REQ-GATE-01..10), published as one command at
/// the address the navigation already owns (#/inbox): every point where
/// the platform stands on the operator — APPROVAL / WAITING_FOR_INPUT /
/// QA / FAILED rows — plus the separate "stopped by the operator" group
/// (PAUSED/BLOCKED) and the reference group of unanswered NON-BLOCKING
/// questions (ratified OQ-GATE-1/2). ONE published read backs the whole
/// section (`inbox.snapshot`): the Tauri command is a
/// thin adapter — one API call, then the pure composer below. Nothing is
/// cached between loads (REQ-GATE-07).
#[tauri::command]
pub fn inbox_view(state: State<AppState>) -> Result<WaitSnapshot, UiError> {
    let facts = state.api().inbox_snapshot()?;
    compose_wait_snapshot(facts)
}

/// The section composer: PURE — data in, section out. No Tauri state, no
/// API handle, no IO (the seam is exactly this function: the batch view in,
/// the section out, so the test below exercises every render branch on an
/// injected snapshot without a live window). Membership is read from the
/// declarative table by
/// the exact stored state token; grouping, ordering (REQ-GATE-08: longest
/// waiting first) and the badge are computed here on the same snapshot.
/// `Err` only for a table the renderer cannot read (fail-closed).
pub fn compose_wait_snapshot(facts: InboxSnapshotView) -> Result<WaitSnapshot, UiError> {
    let InboxSnapshotView { policy, tasks, .. } = facts;
    let mut section = SectionRows::default();
    for task_facts in tasks {
        section.push(&policy, task_facts)?;
    }
    Ok(section.finish())
}

/// The three lists of the section, filled row by row: the platform's wait
/// rows, the operator-stopped rows and the reference notes (ratified
/// OQ-GATE-1). One struct so the grouping decision is made once per task and
/// the badge in `finish` sees the very rows that were produced.
#[derive(Default)]
struct SectionRows {
    waiting: Vec<WaitRow>,
    human_stopped: Vec<WaitRow>,
    notes: Vec<WaitNote>,
}

impl SectionRows {
    /// File one task's facts into its group. The group comes from the
    /// declarative table (`wait_group_of`); `None` — a state the table does
    /// not know — carries no wait row but may still own reference notes.
    fn push(&mut self, policy: &InboxWaitPolicy, facts: InboxTaskFacts) -> Result<(), UiError> {
        let target = match wait_group_of(policy, &facts.task.state) {
            Some(WaitGroup::Platform) => &mut self.waiting,
            Some(WaitGroup::HumanStopped) => &mut self.human_stopped,
            // `NotWait` rows have no place in the section; the arm exists
            // because the group vocabulary is shared with the table.
            Some(WaitGroup::NotWait) => return Ok(()),
            None => {
                self.notes.extend(reference_notes_from(facts));
                return Ok(());
            }
        };
        if let Some(row) = wait_row_from(policy, facts)? {
            target.push(row);
        }
        Ok(())
    }

    /// Ordering by wait age (REQ-GATE-08) and the badge, in ONE place, so
    /// `badge == waiting.len() + human_stopped.len() + notes.len()` holds by
    /// construction (the ratified OQ-GATE-1 shows the notes as rows).
    fn finish(mut self) -> WaitSnapshot {
        sort_by_wait_age(&mut self.waiting);
        sort_by_wait_age(&mut self.human_stopped);
        self.notes.sort_by(|a, b| {
            a.task
                .updated_at
                .cmp(&b.task.updated_at)
                .then(a.question.created_at.cmp(&b.question.created_at))
        });
        let badge = self.waiting.len() + self.human_stopped.len() + self.notes.len();
        WaitSnapshot {
            waiting: self.waiting,
            human_stopped: self.human_stopped,
            notes: self.notes,
            badge,
        }
    }
}

fn sort_by_wait_age(rows: &mut [WaitRow]) {
    rows.sort_by(|a, b| a.task.updated_at.cmp(&b.task.updated_at));
}

/// One wait row from the batch facts: stored OPEN questions, the demand
/// rendered from the data table, and the click-presentation of the gate type
/// (spec for APPROVAL, report for QA — already read by the batch op).
fn wait_row_from(
    policy: &InboxWaitPolicy,
    facts: InboxTaskFacts,
) -> Result<Option<WaitRow>, UiError> {
    let InboxTaskFacts {
        task,
        questions,
        spec,
        report,
        report_note,
        convergence,
    } = facts;
    let open_questions: Vec<Question> = questions
        .into_iter()
        .filter(|q| q.status == QuestionStatus::Open)
        .collect();
    let Some(demand) = wait_demand_for(policy, &task.state, &short_id(&task.id), &open_questions)?
    else {
        return Ok(None);
    };
    Ok(Some(WaitRow {
        spec: spec.as_ref().map(|spec| spec_of(&task, spec)),
        task,
        demand: demand.demand,
        cli_hint: demand.cli_hint,
        open_questions,
        report,
        report_note,
        convergence,
    }))
}

/// Reference-group rows (ratified OQ-GATE-1): stored OPEN NON-blocking
/// questions of a task the platform does NOT stand on — a note on an
/// active task (workflow.rs:747-748), never a wait row. Terminal tasks
/// carry no live demand, so their leftovers stay out (is_note_visible).
fn reference_notes_from(facts: InboxTaskFacts) -> Vec<WaitNote> {
    if !is_note_visible(&facts.task.state) {
        return Vec::new();
    }
    facts
        .questions
        .into_iter()
        .filter(|q| q.status == QuestionStatus::Open && !q.blocking)
        .map(|question| WaitNote {
            task: facts.task.clone(),
            question,
        })
        .collect()
}

/// The APPROVAL presentation (REQ-GATE-04/05): the artifact and body chosen
/// by the batch read (CURRENT REQUIREMENTS_SPEC, else the LAST one marked
/// «not current, approve is impossible» — ratified OQ-GATE-3/4). Parsing the
/// JSON extracts the human paragraph and the document's open_questions; the
/// stored bytes are never rewritten.
fn spec_of(task: &Task, facts: &SpecFacts) -> SpecPresentation {
    match &facts.body {
        SpecBodyView::Text { text } => {
            spec_presentation(&facts.artifact, Ok(text.as_str()), task, facts.current)
        }
        SpecBodyView::Binary { size } => {
            let note = format!("артефакт бинарный (не UTF-8, {size} байт)");
            spec_presentation(&facts.artifact, Err(note.as_str()), task, facts.current)
        }
        SpecBodyView::Error { message } => {
            spec_presentation(&facts.artifact, Err(message.as_str()), task, facts.current)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devai_core::types::ArtifactKind;

    /// REQ-UI-02 AC2: every command's data source is a published registry
    /// entry of class Read. If a command starts calling something the
    /// registry does not publish (or any Mutation), this test fails.
    #[test]
    fn every_command_operation_is_a_registered_read() {
        let ops = devai_api::operations();
        let by_name: std::collections::HashMap<&str, &OpDescriptor> =
            ops.iter().map(|o| (o.name, o)).collect();
        assert!(!COMMAND_OPERATIONS.is_empty());
        for (cmd, op) in COMMAND_OPERATIONS {
            let d = by_name
                .get(op)
                .unwrap_or_else(|| panic!("`{cmd}` calls `{op}`: not a published operation"));
            assert_eq!(
                d.class,
                devai_api::OpClass::Read,
                "`{cmd}` calls `{op}` — a Mutation-class operation is forbidden in the read-only UI (OQ-UI-6)"
            );
        }
    }

    /// The crate's direct dependency graph must not contain the store or
    /// sqlite (REQ-UI-02 AC1 — the dogfood boundary of REQ-API-08).
    /// Comments are stripped first: prose about the prohibition is not a
    /// dependency declaration.
    #[test]
    fn manifest_has_no_direct_store_or_sqlite_dependency() {
        let manifest: String = include_str!("../Cargo.toml")
            .lines()
            .filter(|l| !l.trim_start().starts_with('#'))
            .collect::<Vec<_>>()
            .join("\n");
        for forbidden in ["devai-store", "rusqlite"] {
            assert!(
                !manifest.contains(forbidden),
                "Cargo.toml must not declare `{forbidden}` directly"
            );
        }
        assert!(
            manifest.contains("devai-api"),
            "the UI must reach data through devai-api"
        );
    }

    /// No source file of the UI may import the store crate by name — the
    /// only sanctioned door is `Api` (REQ-MUT-03: the handle stays inside).
    /// The crate name is assembled so this test's own text cannot match it.
    #[test]
    fn sources_never_import_the_store() {
        let needle: String = ["devai", "store"].join("_");
        let dir = concat!(env!("CARGO_MANIFEST_DIR"), "/src");
        for file in ["main.rs", "session.rs", "commands.rs"] {
            let src = std::fs::read_to_string(format!("{dir}/{file}")).unwrap();
            assert!(
                !src.contains(needle.as_str()),
                "{file}: direct store import — the API is the only door"
            );
        }
    }

    #[test]
    fn convergence_chart_stays_static_and_dependency_free() {
        // REQ-CV-10 AC4: инлайн-SVG без интерактива и без chart-зависимостей.
        // Источник читается как текст — это дешёвая страховка от «одного
        // клика для подсказки» и от тихой замены на библиотеку графиков.
        let crate_dir = env!("CARGO_MANIFEST_DIR");
        let chart = std::fs::read_to_string(format!(
            "{crate_dir}/frontend/src/components/ConvergenceChart.vue"
        ))
        .unwrap();
        for forbidden in ["@click", "@mouseover", "@mouseenter", "@keydown", "v-on:"] {
            assert!(
                !chart.contains(forbidden),
                "the convergence chart must stay non-interactive: found `{forbidden}`"
            );
        }
        let package: serde_json::Value = serde_json::from_str(
            &std::fs::read_to_string(format!("{crate_dir}/frontend/package.json")).unwrap(),
        )
        .unwrap();
        let deps = package["dependencies"].as_object().unwrap();
        for name in deps.keys() {
            assert!(
                !name.contains("chart") && !name.contains("d3") && !name.contains("plot"),
                "no chart library may join the frontend: found `{name}`"
            );
        }
    }

    /// Static coverage of the command core: the serde output of the ipc
    /// command core is
    /// the frontend's contract — `types.ts` is written against it. This
    /// fixture pins the exact JSON shape: field names of `UiError`, the
    /// `shape` tag token and the variant field names of `ArtifactContent`.
    /// A rename or a tag move fails HERE (cargo test, before the window
    /// ever opens), not as a runtime symptom of a silent `undefined`
    /// through types.ts.
    #[test]
    fn ipc_serde_output_is_pinned_by_fixtures() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ipc_shapes.json"))
                .expect("ipc fixtures must be valid JSON");
        let id = "01a1aaaa-bbbb-cccc-dddd-eeeeeeeeeeee".to_string();

        let err = UiError {
            code: "not_found".to_string(),
            message: "task `x1` was not found".to_string(),
        };
        assert_eq!(serde_json::to_value(&err).unwrap(), fixture["ui_error"]);

        let text = ArtifactContent::Text {
            artifact_id: id.clone(),
            text: "hello ipc".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&text).unwrap(),
            fixture["artifact_text"]
        );

        let bin = ArtifactContent::Binary {
            artifact_id: id,
            size: 42,
        };
        assert_eq!(
            serde_json::to_value(&bin).unwrap(),
            fixture["artifact_binary"]
        );
    }

    /// QA R4 (mt/change-isolation + ux/contract-stability) + R1
    /// (no-read-time-rederivation): the name/signboard plane crosses the
    /// Rust↔TS boundary in exactly THREE serde shapes — the task row (the
    /// stored name `human_name` AND the finished signboard `display`), the
    /// `task.rename` outcome (`TaskRenameOutcome`), and the dependency
    /// edge (both-end signage `task_display`/`depends_on_display`). The
    /// fixture pins their exact JSON form (the `ui_error`/artifact idiom
    /// above); the second half compares the pinned KEY SETS against the
    /// `Task`/`TaskDependency` interfaces declared in `types.ts` — drift
    /// on either side (a Rust rename, an unmoved TS field) fails the
    /// build now, not as a silent `undefined` in a rendered list.
    #[test]
    fn name_plane_serde_shapes_are_pinned_by_fixtures() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ipc_shapes.json"))
                .expect("ipc fixtures must be valid JSON");

        let task = pinned_task();
        assert_eq!(serde_json::to_value(&task).unwrap(), fixture["task_row"]);

        let outcome = devai_api::TaskRenameOutcome {
            task: task.clone(),
            human_name: "Понятные имена задач".to_string(),
            changed: true,
        };
        assert_eq!(
            serde_json::to_value(&outcome).unwrap(),
            fixture["task_rename_outcome"]
        );

        let id = |s: &str| devai_core::types::Id::parse_str(s).unwrap();
        let edge = devai_core::types::TaskDependency {
            id: id("01a1bbbb-0000-0000-0000-000000000001"),
            task_id: id("01a1aaaa-bbbb-cccc-dddd-eeeeeeeeeeee"),
            task_state: TaskState::Review,
            task_title: task.title.clone(),
            depends_on_id: id("01a1cccc-1111-1111-1111-111111111111"),
            depends_on_state: TaskState::Draft,
            depends_on_title: "заголовок блока, написанный создавшим".to_string(),
            task_display: "Понятные имена задач".to_string(),
            depends_on_display: "Названия задач неразборчивы в списках.".to_string(),
            kind: devai_core::types::DependencyKind::Blocks,
            actor: "operator".to_string(),
            created_at: pinned_now(),
        };
        assert_eq!(
            serde_json::to_value(&edge).unwrap(),
            fixture["task_dependency_edge"]
        );

        // The TS mirror half (drift the other direction): field NAME SETS
        // of the frontend interfaces must equal the pinned serde keys —
        // prose-parsed here exactly like api.ts above.
        let crate_dir = env!("CARGO_MANIFEST_DIR");
        let types_ts =
            std::fs::read_to_string(format!("{crate_dir}/frontend/src/types.ts")).unwrap();
        for (iface, shape) in [
            ("Task", "task_row"),
            ("TaskDependency", "task_dependency_edge"),
        ] {
            let mut ts_fields = ts_interface_fields(&types_ts, iface);
            ts_fields.sort();
            let mut pinned: Vec<String> = fixture[shape]
                .as_object()
                .unwrap()
                .keys()
                .cloned()
                .collect();
            pinned.sort();
            assert_eq!(
                ts_fields, pinned,
                "`types.ts` interface `{iface}` drifted from the pinned serde shape"
            );
        }
    }

    /// The fixed instant behind every pinned name-plane shape — the
    /// fixture is deterministic, never `Utc::now()`.
    fn pinned_now() -> devai_core::types::Timestamp {
        chrono::DateTime::parse_from_rfc3339("2026-11-06T12:00:00Z")
            .unwrap()
            .with_timezone(&chrono::Utc)
    }

    /// The pinned task row: a task whose analyst already stamped a human
    /// name — the exact shape `tasks_list`/`task_get` hand to `types.ts`.
    fn pinned_task() -> Task {
        Task {
            id: devai_core::types::Id::parse_str("01a1aaaa-bbbb-cccc-dddd-eeeeeeeeeeee").unwrap(),
            project_id: None,
            title: "task names: mechanical name plane and list signage".to_string(),
            business_description:
                "Названия задач неразборчивы в списках. Хочу человеческое имя после аналитика."
                    .to_string(),
            human_name: Some("Понятные имена задач".to_string()),
            state: TaskState::Review,
            return_state: None,
            requirements_seq: 2,
            lock_version: 7,
            created_at: pinned_now(),
            updated_at: pinned_now(),
        }
    }

    /// Field names declared by `export interface NAME { … }` in a TS
    /// source: comment-only lines skipped, every field is the token
    /// before its first colon.
    fn ts_interface_fields(ts: &str, iface: &str) -> Vec<String> {
        let open = format!("export interface {iface} {{");
        let start = ts
            .find(&open)
            .unwrap_or_else(|| panic!("types.ts must declare `{iface}`"));
        let body = &ts[start + open.len()..];
        let close = body.find('}').expect("interface closing brace");
        body[..close]
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty() && !l.starts_with("//"))
            .map(|l| l.split(':').next().unwrap_or(l).trim().to_string())
            .collect()
    }

    /// REQ-WV-04 AC1 + AC4 (mechanical half, the window-content criterion
    /// itself is T7's harness): the frontend's data door is closed over
    /// this table — EVERY `invoke("<name>")` in api.ts must
    ///   1) appear in COMMAND_OPERATIONS mapped to a published Read op, and
    ///   2) be registered in main.rs `generate_handler!` (no dead names).
    /// A new frontend call that bypasses the registry mapping fails here,
    /// so the workspace panel (and anything after it) cannot drift into
    /// git knowledge or disk-path invention.
    #[test]
    fn frontend_invokes_are_exactly_mapped_reads() {
        let crate_dir = env!("CARGO_MANIFEST_DIR");
        let api_ts = std::fs::read_to_string(format!("{crate_dir}/frontend/src/api.ts")).unwrap();
        let main_rs = std::fs::read_to_string(format!("{crate_dir}/src/main.rs")).unwrap();

        // Extract every invoke("<name>") / invoke<X>("<name>") token:
        // real calls are the ONLY occurrences of `invoke<` in api.ts
        // (the import and the prose never carry the generic bracket).
        let mut names: Vec<String> = Vec::new();
        let mut rest = api_ts.as_str();
        while let Some(i) = rest.find("invoke<") {
            rest = &rest[i + "invoke<".len()..];
            if let Some(q) = rest.find('"') {
                rest = &rest[q + 1..];
                if let Some(e) = rest.find('"') {
                    names.push(rest[..e].to_string());
                    rest = &rest[e..];
                }
            }
        }
        assert!(
            names.iter().any(|n| n == "task_workspace_changes"),
            "the workspace panel's changeset call is missing from api.ts"
        );
        assert!(
            names.iter().any(|n| n == "task_workspace_diff"),
            "the workspace panel's diff call is missing from api.ts"
        );
        for name in names {
            // Bootstrap facts are the one ratified non-operation (OQ-10).
            if name == "session_info" {
                continue;
            }
            assert!(
                COMMAND_OPERATIONS.iter().any(|(c, _)| *c == name),
                "api.ts invokes `{name}` with no COMMAND_OPERATIONS mapping"
            );
            assert!(
                main_rs.contains(&format!("commands::{name},")),
                "`{name}` is not registered in the invoke_handler of main.rs"
            );
        }
    }

    /// REQ-GATE-02 AC2 + ratified OQ-GATE-2, data-driven (R1): membership is
    /// read from the DECLARATIVE table — the platform's own wait set
    /// (APPROVAL, WAITING_FOR_INPUT, QA, FAILED), the separate
    /// operator-stopped set (PAUSED/BLOCKED), and nothing else. The factory
    /// bytes must name EVERY TaskState variant (the store loader enforces
    /// the completeness; this half pins the expected verdict per variant).
    /// An ACTIVE task carrying only a non-blocking OPEN question creates no
    /// wait row (workflow.rs:747-748).
    #[test]
    fn wait_membership_is_the_state_guard_not_a_guess() {
        use WaitGroup::{HumanStopped, Platform};
        let policy = devai_api::factory_inbox_wait_policy().unwrap();
        let table: [(TaskState, Option<WaitGroup>); 13] = [
            (TaskState::Draft, None),
            (TaskState::Analysis, None),
            (TaskState::Review, None),
            (TaskState::Approval, Some(Platform)),
            (TaskState::Implementation, None),
            (TaskState::Qa, Some(Platform)),
            (TaskState::Rework, None),
            (TaskState::WaitingForInput, Some(Platform)),
            (TaskState::Paused, Some(HumanStopped)),
            (TaskState::Blocked, Some(HumanStopped)),
            (TaskState::Failed, Some(Platform)),
            (TaskState::Accepted, None),
            (TaskState::Cancelled, None),
        ];
        for (state, expected) in table {
            assert_eq!(wait_group_of(&policy, &state), expected, "{state:?}");
        }
        // The reference group never speaks for terminal tasks.
        assert!(!is_note_visible(&TaskState::Accepted));
        assert!(!is_note_visible(&TaskState::Cancelled));
        assert!(is_note_visible(&TaskState::Implementation));
    }

    /// The rendered demand of one state from the FACTORY table — the shared
    /// seam of the wording tests below (a missing row is a test failure, not
    /// a silent `None`).
    fn demand_row(state: TaskState, id: &str, questions: &[Question]) -> WaitDemand {
        let policy = devai_api::factory_inbox_wait_policy().unwrap();
        wait_demand_for(&policy, &state, id, questions)
            .unwrap()
            .unwrap_or_else(|| panic!("{state:?} missing from the data table"))
    }

    /// REQ-GATE-03 AC1 + REQ-GATE-06 AC2, rendered from the data table (R1):
    /// every wait type carries human wording of the demand and the literal
    /// CLI command. Ратифицированная «дословная команда» = фактический
    /// синтаксис CLI: rationale/text — --long-опции (crates/devai-cli), не
    /// позиционные аргументы.
    #[test]
    fn wait_demand_names_the_operation_and_the_cli() {
        let approval = demand_row(TaskState::Approval, "01a1b0ea", &[]);
        assert!(approval.demand.contains("спецификацию"), "{approval:?}");
        assert!(
            approval
                .cli_hint
                .contains("task approve 01a1b0ea --rationale"),
            "{approval:?}"
        );
        assert!(
            approval
                .cli_hint
                .contains("task reject 01a1b0ea --rationale"),
            "{approval:?}"
        );

        let qa = demand_row(TaskState::Qa, "abc123", &[]);
        assert!(qa.demand.contains("quality-отчёт"), "{qa:?}");
        assert!(
            qa.cli_hint.contains("task accept abc123 --rationale"),
            "{qa:?}"
        );
        assert!(
            qa.cli_hint.contains("task return abc123 --rationale"),
            "{qa:?}"
        );
        let failed = demand_row(TaskState::Failed, "f1", &[]);
        assert!(failed.cli_hint.contains("task resume f1"), "{failed:?}");

        // A non-wait state has no row at all — the table says so (group
        // `none`), so there is nothing to render: honest emptiness, not a
        // placeholder token dressed as a command.
        let policy = devai_api::factory_inbox_wait_policy().unwrap();
        assert!(
            wait_demand_for(&policy, &TaskState::Implementation, "i1", &[])
                .unwrap()
                .is_none()
        );
    }

    /// The WAITING hint must be runnable as printed: a real question id when
    /// a BLOCKING question is parked — a stray non-blocking note beside it
    /// must not steal the printed id.
    #[test]
    fn waiting_demand_names_the_parked_question() {
        let parked = question_fixture("01a1b0ea-0000-4000-8000-0000000000bb", true);
        let waiting = demand_row(TaskState::WaitingForInput, "x", &[parked]);
        assert!(
            waiting
                .cli_hint
                .contains("question answer 01a1b0ea-0000-4000-8000-0000000000bb --text"),
            "{waiting:?}"
        );
        assert!(!waiting.cli_hint.contains("<вопрос-id>"), "{waiting:?}");

        let note = question_fixture("01a1b0ea-0000-4000-8000-0000000000cc", false);
        let parked = question_fixture("01a1b0ea-0000-4000-8000-0000000000bb", true);
        let waiting = demand_row(TaskState::WaitingForInput, "x", &[note, parked]);
        assert!(
            waiting
                .cli_hint
                .contains("question answer 01a1b0ea-0000-4000-8000-0000000000bb --text"),
            "{waiting:?}"
        );
    }

    /// No BLOCKING question = the empty-rework gate: the real escape command
    /// (a non-blocking note is not answerable from that gate).
    #[test]
    fn waiting_demand_names_the_empty_rework_gate() {
        let note = question_fixture("01a1b0ea-0000-4000-8000-0000000000cc", false);
        let waiting = demand_row(TaskState::WaitingForInput, "x", &[note]);
        assert!(waiting.demand.contains("пустого реворка"), "{waiting:?}");
        assert!(
            waiting
                .cli_hint
                .contains("task ratify-empty-rework x --rationale"),
            "{waiting:?}"
        );
    }

    /// An unknown `when` predicate in the delivered
    /// table is a LOUD render error, not a silently non-matching variant.
    /// Without the parked-question term the row would otherwise print empty
    /// demand and an empty command — the fail-open shape this test kills.
    #[test]
    fn unknown_variant_predicate_is_a_render_error() {
        let mut policy = devai_api::factory_inbox_wait_policy().unwrap();
        let row = policy
            .states
            .get_mut("WAITING_FOR_INPUT")
            .expect("factory table has the parking state");
        row.variants[0].when = "parked".to_string();
        let err = wait_demand_for(
            &policy,
            &TaskState::WaitingForInput,
            "01a1b0ea",
            &[question_fixture(
                "01a1b0ea-0000-4000-8000-0000000000bb",
                true,
            )],
        )
        .expect_err("an unknown predicate must not render");
        assert_eq!(err.code, "invalid_config");
        assert!(err.message.contains("`parked`"), "{}", err.message);
        assert!(err.message.contains("parked_question"), "{}", err.message);
    }

    /// REQ-GATE-04 AC1 + ratified OQ-GATE-4: the paragraph and the spec's
    /// own open_questions come from the artifact JSON; when the body
    /// predates the paragraph protocol the stored business description
    /// stands in — with the honest label, never an empty presentation.
    #[test]
    fn spec_presentation_reads_the_document_without_rewriting_it() {
        let now = now_fixture();
        let task = task_fixture("дело", "бизнес-описание задачи", now);
        let artifact = spec_artifact_fixture();

        let body = r#"{"version":3,"human_paragraph":"Коротко: что происходит.",
            "open_questions":["OQ-A: первый?","OQ-B: второй?"]}"#;
        let p = spec_presentation(&artifact, Ok(body), &task, true);
        assert_eq!(p.paragraph_source, ParagraphSource::Spec);
        assert_eq!(
            p.human_paragraph.as_deref(),
            Some("Коротко: что происходит.")
        );
        assert_eq!(p.open_questions.len(), 2);
        assert!(p.unavailable_note.is_none());
        assert!(p.current);

        // Pre-protocol spec (no human_paragraph key) → business_description
        // fallback with the ratified label (OQ-GATE-4).
        let old = r#"{"version":2,"open_questions":["OQ-TN: вопрос документа"]}"#;
        let p = spec_presentation(&artifact, Ok(old), &task, false);
        assert_eq!(p.paragraph_source, ParagraphSource::BusinessDescription);
        assert_eq!(p.human_paragraph.as_deref(), Some("бизнес-описание задачи"));
        assert!(
            p.unavailable_note
                .as_deref()
                .is_some_and(|n| n.contains("спека до протокола"))
        );
        assert_eq!(
            p.open_questions,
            vec!["OQ-TN: вопрос документа".to_string()]
        );
        assert!(!p.current, "OQ-GATE-3 label rides on the same view");

        // Unparsable body / binary artifact (AC2): honest fact + fallback,
        // raw bytes are never rendered.
        for p in [
            spec_presentation(&artifact, Ok("не json {{{"), &task, true),
            spec_presentation(
                &artifact,
                Err("артефакт бинарный (не UTF-8, 12 байт)"),
                &task,
                true,
            ),
        ] {
            assert_eq!(p.paragraph_source, ParagraphSource::BusinessDescription);
            assert!(p.unavailable_note.is_some());
            assert_eq!(p.human_paragraph.as_deref(), Some("бизнес-описание задачи"));
        }
    }

    /// One row of the section with the given title — the minimal shape the
    /// grouping and badge tests need.
    fn wait_row_fixture(title: &str, now: chrono::DateTime<chrono::Utc>) -> WaitRow {
        WaitRow {
            task: task_fixture(title, "", now),
            demand: "w".into(),
            cli_hint: "h".into(),
            open_questions: vec![],
            spec: None,
            report: None,
            report_note: None,
            convergence: None,
        }
    }

    /// Badge discipline (REQ-GATE-08 AC3): the badge is the number of rows
    /// of the section on the same snapshot — waiting + human-stopped + the
    /// ratified OQ-GATE-1 reference notes (they are shown rows, not a
    /// hidden bucket). Asserted through `SectionRows::finish` itself, the
    /// same function `compose_wait_snapshot` ends with, so a badge formula
    /// that dropped the notes would fail here.
    #[test]
    fn badge_counts_every_section_row() {
        let now = now_fixture();
        let snapshot = SectionRows {
            waiting: vec![wait_row_fixture("a", now)],
            human_stopped: vec![wait_row_fixture("b", now)],
            notes: vec![WaitNote {
                task: task_fixture("c", "", now),
                question: question_fixture("01a1b0ea-0000-4000-8000-0000000000cc", false),
            }],
        }
        .finish();
        // 1 waiting + 1 human-stopped + 1 note.
        assert_eq!(snapshot.badge, 3);
        assert_eq!(
            snapshot.badge,
            snapshot.waiting.len() + snapshot.human_stopped.len() + snapshot.notes.len()
        );
    }

    /// Shape pinning for the two shapes the wait section
    /// publishes — pinned exactly like `ui_error`/`artifact_*`: a
    /// renamed field or a moved tag fails HERE, in cargo test, not as a
    /// silent `undefined` through types.ts at runtime.
    #[test]
    fn wait_section_shapes_are_pinned_by_fixtures() {
        let fixture = ipc_shapes_fixture();
        let (snapshot, presentation) = wait_section_fixture();
        assert_eq!(
            serde_json::to_value(&snapshot).unwrap(),
            fixture["wait_snapshot"]
        );
        assert_eq!(
            serde_json::to_value(&presentation).unwrap(),
            fixture["spec_presentation"]
        );
    }

    /// R3 parity half: every field name the Rust shapes serialize exists
    /// in the mirrored interface of `types.ts` (snake_case mirror — the
    /// serde structs carry no rename_all), so a Rust-side rename is caught
    /// by the fixture test and a TS-side rename by this one.
    #[test]
    fn wait_section_shapes_parity_with_types_ts() {
        let ts = std::fs::read_to_string(format!(
            "{}/frontend/src/types.ts",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let (snapshot, presentation) = wait_section_fixture();
        let snapshot = serde_json::to_value(&snapshot).unwrap();
        let specification = serde_json::to_value(&presentation).unwrap();
        let pairs: [(&str, &serde_json::Value); 4] = [
            ("WaitSnapshot", &snapshot),
            ("WaitRow", &snapshot["waiting"][0]),
            ("WaitNote", &snapshot["notes"][0]),
            ("SpecPresentation", &specification),
        ];
        for (iface, value) in pairs {
            let block = ts_interface_block(&ts, iface);
            for field in value.as_object().expect("object shape").keys() {
                assert!(
                    block.contains(&format!("{field}:")),
                    "types.ts `{iface}` has no field `{field}` the Rust view serializes"
                );
            }
        }
    }

    /// One deterministic snapshot + presentation used by both R3 tests.
    fn wait_section_fixture() -> (WaitSnapshot, SpecPresentation) {
        let now = "2026-11-06T12:00:00Z"
            .parse::<chrono::DateTime<chrono::Utc>>()
            .unwrap();
        let mut task = task_fixture("Спека ожидания", "бизнес-описание", now);
        task.id = "01a1b0ea-0000-4000-8000-000000000001".parse().unwrap();
        task.state = TaskState::Approval;
        task.requirements_seq = 3;
        task.lock_version = 4;
        let question = Question {
            id: "01a1b0ea-0000-4000-8000-0000000000bb".parse().unwrap(),
            task_id: task.id,
            execution_id: None,
            text: "Ответить на вопрос?".into(),
            context: None,
            options: vec![],
            blocking: true,
            status: QuestionStatus::Open,
            answer: None,
            answered_by: None,
            answered_at: None,
            created_at: now,
        };
        // The reference note is the NON-blocking twin (OQ-GATE-1): the
        // platform does not stand on it.
        let note = Question {
            id: "01a1b0ea-0000-4000-8000-0000000000cc".parse().unwrap(),
            text: "Заметка без блокировки?".into(),
            blocking: false,
            ..question.clone()
        };
        let mut artifact = spec_artifact_fixture();
        artifact.id = "01a1b0ea-0000-4000-8000-0000000000aa".parse().unwrap();
        artifact.task_id = task.id;
        artifact.created_at = now;
        let presentation = SpecPresentation {
            artifact,
            current: true,
            human_paragraph: Some("Коротко: что происходит.".into()),
            paragraph_source: ParagraphSource::Spec,
            open_questions: vec!["OQ-GATE-1: вопрос документа?".into()],
            unavailable_note: None,
        };
        let row = WaitRow {
            task: task.clone(),
            demand: "Утвердить или отклонить текущую спецификацию задачи".into(),
            cli_hint: "devai task approve 01a1b0ea --rationale \"<обоснование>\"".into(),
            open_questions: vec![question.clone()],
            spec: Some(presentation.clone()),
            report: None,
            report_note: None,
            convergence: None,
        };
        let snapshot = WaitSnapshot {
            waiting: vec![row],
            human_stopped: vec![],
            notes: vec![WaitNote {
                task: task.clone(),
                question: note,
            }],
            // 1 waiting + 1 reference note = every row of the section
            // (REQ-GATE-08 AC3; the notes are shown, ratified OQ-GATE-1).
            badge: 2,
        };
        (snapshot, presentation)
    }

    fn ipc_shapes_fixture() -> serde_json::Value {
        serde_json::from_str(include_str!("../tests/fixtures/ipc_shapes.json"))
            .expect("ipc fixtures must be valid JSON")
    }

    /// The `export interface <name> {` block of types.ts, up to its
    /// closing brace.
    fn ts_interface_block<'a>(source: &'a str, name: &str) -> &'a str {
        let head = format!("export interface {name} {{");
        let start = source
            .find(&head)
            .unwrap_or_else(|| panic!("types.ts has no `{head}`"));
        let rest = &source[start..];
        let end = rest.find("\n}").expect("unterminated types.ts interface");
        &rest[..end]
    }

    fn task_fixture(
        title: &str,
        business_description: &str,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Task {
        let mut t = Task::new(title, business_description, None, now);
        t.updated_at = now;
        t
    }

    fn spec_artifact_fixture() -> Artifact {
        let now = now_fixture();
        Artifact {
            id: devai_core::types::new_id(),
            task_id: devai_core::types::new_id(),
            kind: ArtifactKind::RequirementsSpec,
            name: "requirements-specification".into(),
            seq: 3,
            category: devai_core::types::DataCategory::Artifact,
            status: devai_core::types::ArtifactStatus::Current,
            sha256: "x".repeat(64),
            size: 12,
            media_type: "application/json".into(),
            contract: None,
            created_by: "user".into(),
            execution_id: None,
            depends_on: vec![],
            created_at: now,
        }
    }

    /// A breaking command shape is a RECORDED,
    /// versioned fact — every entry of the ledger is a mapped command with
    /// version >= 2 and a non-empty breaking note, and the section's
    /// breaking revision is present. A new un-versioned break fails here.
    #[test]
    fn breaking_command_contracts_are_versioned_and_mapped() {
        for contract in COMMAND_CONTRACTS {
            assert!(
                contract.version >= 2,
                "{}: a recorded breaking revision starts at v2",
                contract.command
            );
            assert!(
                !contract.breaking.trim().is_empty(),
                "{}: a breaking revision must say what changed",
                contract.command
            );
            assert!(
                COMMAND_OPERATIONS
                    .iter()
                    .any(|(cmd, _)| *cmd == contract.command),
                "{} is versioned but not mapped in COMMAND_OPERATIONS",
                contract.command
            );
        }
        assert!(
            COMMAND_CONTRACTS
                .iter()
                .any(|c| c.command == "inbox_view" && c.version == 2),
            "inbox_view changed shape this iteration and must be versioned"
        );
    }

    /// R4 parity half: the section's row TEXTS (demand copy + the literal
    /// CLI hint) are pinned by a snapshot, so a copy drift or a placeholder
    /// creeping back into the hint fails in cargo test rather than in the
    /// window. Each hint must be runnable as printed.
    #[test]
    fn wait_section_row_texts_are_pinned_by_a_snapshot() {
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/wait_texts.json"))
                .expect("wait_texts fixture must be valid JSON");
        let policy = devai_api::factory_inbox_wait_policy().unwrap();
        let id = fixture["id"].as_str().unwrap();
        let question = question_fixture(fixture["question_id"].as_str().unwrap(), true);

        let mut actual = serde_json::Map::new();
        let cases: [(&str, TaskState, Vec<Question>); 7] = [
            ("APPROVAL", TaskState::Approval, vec![]),
            (
                "WAITING_FOR_INPUT",
                TaskState::WaitingForInput,
                vec![question],
            ),
            (
                "WAITING_FOR_INPUT_EMPTY_REWORK",
                TaskState::WaitingForInput,
                vec![],
            ),
            ("QA", TaskState::Qa, vec![]),
            ("FAILED", TaskState::Failed, vec![]),
            ("PAUSED", TaskState::Paused, vec![]),
            ("BLOCKED", TaskState::Blocked, vec![]),
        ];
        for (token, state, questions) in cases {
            let demand = wait_demand_for(&policy, &state, id, &questions)
                .unwrap()
                .unwrap_or_else(|| panic!("{token} missing from the demand table"));
            actual.insert(
                token.to_string(),
                serde_json::json!({ "demand": demand.demand, "cli_hint": demand.cli_hint }),
            );
        }
        assert_eq!(
            serde_json::Value::Object(actual),
            fixture["rows"],
            "row texts drifted from the pinned shape/text ledger"
        );
    }

    /// The injected batch snapshot the composer tests drive: all thirteen
    /// stored states, both groups, the notes group, the gate presentations
    /// and their honest absences. Returns the snapshot input plus the parked
    /// question id the CLI hint must name.
    fn composer_fixture() -> (
        InboxSnapshotView,
        devai_core::types::Id,
        chrono::DateTime<chrono::Utc>,
    ) {
        let policy = devai_api::factory_inbox_wait_policy().unwrap();
        let now = now_fixture();
        let parked = question_fixture("01a1b0ea-0000-4000-8000-0000000000bb", true);
        let parked_id = parked.id;
        let mut facts: Vec<InboxTaskFacts> = ALL_STATES
            .iter()
            .enumerate()
            .map(|(i, state)| facts_for_state(*state, at(now, i as i64), &parked))
            .collect();
        attach_approval_spec(&mut facts, now);
        attach_qa_rows(&mut facts, now, at(now, 600));

        (
            InboxSnapshotView {
                snapshot_at: now,
                policy,
                tasks: facts,
            },
            parked_id,
            now,
        )
    }

    /// Every stored TaskState in order — the composer tests drive all of them.
    const ALL_STATES: [TaskState; 13] = [
        TaskState::Draft,
        TaskState::Analysis,
        TaskState::Review,
        TaskState::Approval,
        TaskState::Implementation,
        TaskState::Qa,
        TaskState::Rework,
        TaskState::WaitingForInput,
        TaskState::Paused,
        TaskState::Blocked,
        TaskState::Failed,
        TaskState::Accepted,
        TaskState::Cancelled,
    ];

    fn at(base: chrono::DateTime<chrono::Utc>, secs: i64) -> chrono::DateTime<chrono::Utc> {
        base + chrono::Duration::seconds(secs)
    }

    /// One task's injected facts for the given stored state: the parked
    /// blocking question for WAITING_FOR_INPUT, the non-blocking note on the
    /// ACTIVE row (its TERMINAL twin must stay out).
    fn facts_for_state(
        state: TaskState,
        when: chrono::DateTime<chrono::Utc>,
        parked: &Question,
    ) -> InboxTaskFacts {
        let mut task = task_fixture(&format!("{state:?}"), "бизнес-описание", when);
        task.state = state;
        task.id = devai_core::types::new_id();
        let mut questions = Vec::new();
        if state == TaskState::WaitingForInput {
            let mut q = parked.clone();
            q.task_id = task.id;
            questions.push(q);
        }
        if matches!(state, TaskState::Implementation | TaskState::Accepted) {
            let mut q = question_fixture("01a1b0ea-0000-4000-8000-0000000000cc", false);
            q.task_id = task.id;
            questions.push(q);
        }
        InboxTaskFacts {
            task,
            questions,
            spec: None,
            report: None,
            report_note: None,
            convergence: None,
        }
    }

    /// The APPROVAL row's presentation: human paragraph + the document's own
    /// open question, as separate data (REQ-GATE-04/05).
    fn attach_approval_spec(facts: &mut [InboxTaskFacts], now: chrono::DateTime<chrono::Utc>) {
        let ix = facts
            .iter()
            .position(|f| f.task.state == TaskState::Approval)
            .expect("the fixture contains the APPROVAL row");
        let mut artifact = spec_artifact_fixture();
        artifact.task_id = facts[ix].task.id;
        artifact.created_at = now;
        facts[ix].spec = Some(SpecFacts {
            artifact,
            current: true,
            body: SpecBodyView::Text {
                text: "{\"version\":3,\"human_paragraph\":\"Абзац спеки.\",\"open_questions\":[\"OQ-1: вопрос документа?\"]}".to_string(),
            },
        });
    }

    /// The QA rows: one with a signable report, one without (honest platform
    /// text in `report_note` instead of an empty hole).
    fn attach_qa_rows(
        facts: &mut Vec<InboxTaskFacts>,
        now: chrono::DateTime<chrono::Utc>,
        empty_at: chrono::DateTime<chrono::Utc>,
    ) {
        let ix = facts
            .iter()
            .position(|f| f.task.state == TaskState::Qa)
            .expect("the fixture contains the QA row");
        facts[ix].report = Some(QualityReportView {
            task: facts[ix].task.id.to_string(),
            artifact: devai_core::types::new_id().to_string(),
            seq: 2,
            contract: "quality-report@1".to_string(),
            created_at: now,
            execution: None,
            report: serde_json::json!({"verdict": "PASS"}),
            report_text: Some("Отчёт на подпись".to_string()),
            // Поля свежести/источника (@5): фикстура изображает свежий CURRENT
            // отчёт с плоским текстом (до человеческой формы).
            status: "CURRENT".to_string(),
            source: "flat_text".to_string(),
            freshness: None,
        });
        let mut empty_qa = facts[ix].clone();
        empty_qa.task.id = devai_core::types::new_id();
        empty_qa.task.updated_at = empty_at;
        empty_qa.report = None;
        empty_qa.report_note = Some("task has no CURRENT quality report".to_string());
        facts.push(empty_qa);
    }

    /// Membership and ordering: the stored state token picks the group from
    /// the data table; the platform's rows sort by wait age; non-wait states
    /// and the note of a TERMINAL task produce no row.
    #[test]
    fn composer_groups_rows_by_stored_state() {
        let (facts, _, _) = composer_fixture();
        let snapshot = compose_wait_snapshot(facts).unwrap();

        // Membership: APPROVAL, WAITING_FOR_INPUT, QA, QA-without-report,
        // FAILED on the platform group; PAUSED/BLOCKED stopped by the human;
        // one reference note. The terminal note stays out.
        assert_eq!(snapshot.waiting.len(), 5, "{:?}", snapshot.waiting);
        assert_eq!(snapshot.human_stopped.len(), 2);
        assert_eq!(snapshot.notes.len(), 1);
        assert_eq!(snapshot.badge, 8);
        assert_eq!(
            snapshot.badge,
            snapshot.waiting.len() + snapshot.human_stopped.len() + snapshot.notes.len()
        );

        // REQ-GATE-08 ordering: the oldest updated_at first.
        assert_eq!(snapshot.waiting[0].task.state, TaskState::Approval);
        assert!(
            snapshot
                .waiting
                .windows(2)
                .all(|w| w[0].task.updated_at <= w[1].task.updated_at)
        );
    }

    /// The APPROVAL presentation arrives whole: the spec's human paragraph
    /// and the DOCUMENT's own open questions (not registry questions), plus
    /// the parked WAITING question whose id the CLI hint must carry.
    #[test]
    fn composer_carries_the_approval_and_waiting_presentations() {
        let (facts, parked_id, _) = composer_fixture();
        let snapshot = compose_wait_snapshot(facts).unwrap();

        let approval = &snapshot.waiting[0];
        let spec = approval.spec.as_ref().expect("APPROVAL presents its spec");
        assert_eq!(spec.human_paragraph.as_deref(), Some("Абзац спеки."));
        assert_eq!(spec.open_questions, vec!["OQ-1: вопрос документа?"]);
        assert!(spec.current);

        let waiting_row = snapshot
            .waiting
            .iter()
            .find(|r| r.task.state == TaskState::WaitingForInput)
            .unwrap();
        assert!(waiting_row.demand.contains("припаркованный вопрос"));
        assert!(
            waiting_row.cli_hint.contains(&parked_id.to_string()),
            "{}",
            waiting_row.cli_hint
        );
        assert_eq!(waiting_row.open_questions.len(), 1);
    }

    /// The QA presentation arrives whole — the signable report on one row and
    /// the platform's honest note on the row without one — and non-wait
    /// states produced no row anywhere.
    #[test]
    fn composer_carries_the_qa_presentations() {
        let (facts, _, _) = composer_fixture();
        let snapshot = compose_wait_snapshot(facts).unwrap();

        let qa_row = snapshot
            .waiting
            .iter()
            .find(|r| r.task.state == TaskState::Qa && r.report.is_some())
            .unwrap();
        assert_eq!(
            qa_row.report.as_ref().unwrap().report_text.as_deref(),
            Some("Отчёт на подпись")
        );
        let empty_qa_row = snapshot
            .waiting
            .iter()
            .find(|r| r.task.state == TaskState::Qa && r.report.is_none())
            .unwrap();
        assert!(empty_qa_row.report_note.is_some());

        // Non-wait states produced no row anywhere.
        for state in [
            TaskState::Draft,
            TaskState::Analysis,
            TaskState::Review,
            TaskState::Implementation,
            TaskState::Rework,
            TaskState::Accepted,
            TaskState::Cancelled,
        ] {
            assert!(
                !snapshot.waiting.iter().any(|r| r.task.state == state)
                    && !snapshot.human_stopped.iter().any(|r| r.task.state == state),
                "{state:?} must not be a wait row"
            );
        }
    }

    /// The convergence slice survives the composer untouched: the row
    /// carries exactly the object the batch read produced (the same one the
    /// progress read answers), and a terminal row carries `None`
    /// (REQ-CV-12 AC1/AC5/AC6).
    #[test]
    fn composer_carries_the_convergence_slice() {
        use devai_api::convergence::{
            ConvergenceCounters, ConvergencePoint, ConvergenceView, ModeBasis, StageConvergence,
            mode, rule,
        };
        let (mut facts, _, _) = composer_fixture();
        let point = ConvergencePoint {
            round: 1,
            artifact_id: "art-1".into(),
            created_at: chrono::Utc::now(),
            stage: "quality",
            status: "CURRENT".into(),
            measured: true,
            comparable: false,
            missing: vec![],
            score_milli: Some(940),
            theta_milli: Some(950),
            low_count: Some(2),
            unscored_count: Some(0),
            closed_count: None,
            delta_score_milli: None,
            verdict: None,
            requirements_count: None,
            oq_count: None,
            delta_requirements: None,
            delta_oq: None,
            volume_direction: None,
            time_seconds: Some(60),
            profile: Some("home@1".into()),
            mode: mode::UNKNOWN,
            mode_basis: ModeBasis {
                rule: rule::UNKNOWN,
                reason: Some("insufficient_data"),
                rounds: vec![],
                check_id: None,
            },
        };
        let view = ConvergenceView {
            gap_milli: 10,
            stages: vec![StageConvergence {
                stage: "quality",
                series: vec![point],
                counters: ConvergenceCounters {
                    rounds_total: 1,
                    rounds_unmeasured: 0,
                    rounds_without_time: 0,
                    spec_versions_without_review: 0,
                },
                stage_total_seconds: Some(60),
                missing: vec![],
            }],
        };
        let qa = facts
            .tasks
            .iter_mut()
            .find(|f| f.task.state == TaskState::Qa)
            .expect("fixture has a QA task");
        qa.convergence = Some(view.clone());

        let snapshot = compose_wait_snapshot(facts).unwrap();
        let row = snapshot
            .waiting
            .iter()
            .find(|r| r.task.state == TaskState::Qa)
            .unwrap();
        let carried = row.convergence.as_ref().expect("slice rides the row");
        assert_eq!(carried.stages[0].stage, "quality");
        assert_eq!(carried.stages[0].series[0].score_milli, Some(940));
    }

    /// IPC cost per row is O(1): the command is backed by exactly ONE
    /// published read, so the number of rows cannot multiply the calls
    /// (the counter is the mapping itself: a second entry, or a fan-out
    /// command, fails here).
    #[test]
    fn inbox_view_is_one_batch_read() {
        let ops: Vec<&str> = COMMAND_OPERATIONS
            .iter()
            .filter(|(cmd, _)| *cmd == "inbox_view")
            .map(|(_, op)| *op)
            .collect();
        assert_eq!(
            ops,
            vec!["inbox.snapshot"],
            "the section must be one batch read, not a per-task fan-out"
        );
        let registered = devai_api::operations()
            .into_iter()
            .find(|o| o.name == "inbox.snapshot")
            .expect("inbox.snapshot must be a published operation");
        assert_eq!(registered.class, devai_api::OpClass::Read);
    }

    /// The shared token dictionary: `frontend/src/states.ts` mirrors EVERY
    /// stored TaskState token of Rust, and the view compares through that
    /// shared const instead of carrying raw literals — a Rust token rename
    /// fails HERE; a raw literal in the template fails the second half.
    #[test]
    fn states_ts_mirrors_rust_task_state_tokens() {
        let crate_dir = env!("CARGO_MANIFEST_DIR");
        let ts = std::fs::read_to_string(format!("{crate_dir}/frontend/src/states.ts"))
            .expect("frontend/src/states.ts must exist");
        let view = std::fs::read_to_string(format!("{crate_dir}/frontend/src/views/InboxView.vue"))
            .expect("InboxView.vue must exist");
        let states: [TaskState; 13] = [
            TaskState::Draft,
            TaskState::Analysis,
            TaskState::Review,
            TaskState::Approval,
            TaskState::Implementation,
            TaskState::Qa,
            TaskState::Rework,
            TaskState::WaitingForInput,
            TaskState::Paused,
            TaskState::Blocked,
            TaskState::Failed,
            TaskState::Accepted,
            TaskState::Cancelled,
        ];
        for state in states {
            let token = serde_json::to_value(&state).unwrap();
            let token = token.as_str().unwrap();
            assert!(
                ts.contains(&format!("\"{token}\"")),
                "states.ts has no mirror for TaskState::{state:?} (`{token}`)"
            );
        }
        for raw in [
            "\"APPROVAL\"",
            "'APPROVAL'",
            "\"WAITING_FOR_INPUT\"",
            "'WAITING_FOR_INPUT'",
        ] {
            assert!(
                !view.contains(raw),
                "InboxView.vue carries a raw state token {raw} — use TaskStateToken"
            );
        }
    }

    /// Test-only deterministic clock for fixtures (no per-call wall drift).
    fn now_fixture() -> chrono::DateTime<chrono::Utc> {
        use chrono::Timelike;
        chrono::Utc::now().with_nanosecond(0).unwrap()
    }

    fn question_fixture(id: &str, blocking: bool) -> Question {
        let now = now_fixture();
        Question {
            id: id.parse().unwrap(),
            task_id: devai_core::types::new_id(),
            execution_id: None,
            text: "Ответить на вопрос?".into(),
            context: None,
            options: vec![],
            blocking,
            status: QuestionStatus::Open,
            answer: None,
            answered_by: None,
            answered_at: None,
            created_at: now,
        }
    }

    /// Rework R4: the check-state and item-kind unions are GENERATED from
    /// `crates/devai-core/vocab.json` by `build.rs` into
    /// `frontend/src/vocab.generated.ts`; `types.ts` only re-exports them.
    /// This test keeps two promises: the generated file matches the Rust
    /// enums (no drift, no hand-edit), and `types.ts` really re-exports it
    /// (deleting the re-export would silently unhook the UI from the data).
    #[test]
    fn ts_types_match_the_rust_vocabulary() {
        let generated = include_str!("../frontend/src/vocab.generated.ts");
        assert!(
            generated.starts_with("// @generated"),
            "vocab.generated.ts must announce itself as generated"
        );
        let states = quoted_union(generated, "CheckStateToken");
        let expected: Vec<&str> = devai_core::vocab::CheckState::ALL
            .iter()
            .map(|s| s.as_str())
            .collect();
        assert_eq!(
            states, expected,
            "vocab.generated.ts CheckStateToken drifted"
        );
        let kinds = quoted_union(generated, "HumanItemKind");
        let expected: Vec<&str> = devai_core::vocab::ItemKind::ALL
            .iter()
            .map(|k| k.as_str())
            .collect();
        assert_eq!(kinds, expected, "vocab.generated.ts HumanItemKind drifted");

        let ts = include_str!("../frontend/src/types.ts");
        assert!(
            ts.contains(
                "export type { CheckStateToken, HumanItemKind } from \"./vocab.generated\";"
            ),
            "types.ts must re-export both unions from the generated vocabulary"
        );
    }

    /// The double-quoted members of `export type NAME = "a" | "b";`.
    fn quoted_union(ts: &str, name: &str) -> Vec<String> {
        let head = format!("export type {name} =");
        let start = ts
            .find(&head)
            .unwrap_or_else(|| panic!("types.ts declares `{name}`"))
            + head.len();
        let body = &ts[start..];
        let end = body.find(';').expect("the union ends with a semicolon");
        body[..end]
            .split('"')
            .skip(1)
            .step_by(2)
            .map(str::to_string)
            .collect()
    }
}
