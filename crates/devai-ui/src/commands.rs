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
use devai_api::{WorkspaceChangesView, WorkspaceDiffView};
use devai_core::types::{
    Artifact, ArtifactKind, Decision, Project, Question, QuestionStatus, RoleExecution, Task,
    TaskState, short_id,
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
    /// Platform registry questions of the task, stored OPEN token only
    /// (question.list composition, REQ-GATE-03 AC2).
    pub open_questions: Vec<Question>,
    /// APPROVAL presentation: the spec the human is asked to sign
    /// (REQ-GATE-04/05 + ratified OQ-GATE-3/4).
    pub spec: Option<SpecPresentation>,
    /// QA presentation: the CURRENT quality report (registry.rs:155-161).
    pub report: Option<QualityReportView>,
    /// Honest fact when the QA row has no signable report (platform error
    /// text, never an empty hole).
    pub report_note: Option<String>,
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

/// The section's membership predicate — the EXACT stored state tokens,
/// never a UI inference (REQ-GATE-02, ratified OQ-GATE-2). The platform
/// parks on: APPROVAL (approve/reject guard workflow.rs:250-255),
/// WAITING_FOR_INPUT (blocking raise workflow.rs:750-763; the only state
/// the answer guard admits workflow.rs:766-777), QA (the human signs the
/// CURRENT report registry.rs:155-161, 242-255) and FAILED (parked for a
/// resume decision). PAUSED/BLOCKED are the second, separate group.
pub fn wait_group_of(state: &TaskState) -> Option<WaitGroup> {
    match state {
        TaskState::Approval | TaskState::WaitingForInput | TaskState::Qa | TaskState::Failed => {
            Some(WaitGroup::Platform)
        }
        TaskState::Paused | TaskState::Blocked => Some(WaitGroup::StoppedByHuman),
        // Every remaining state is a non-wait state, listed explicitly so a
        // NEW TaskState variant fails to compile instead of silently
        // dropping out of the section (R3, mt/change-isolation).
        TaskState::Draft
        | TaskState::Analysis
        | TaskState::Review
        | TaskState::Implementation
        | TaskState::Rework
        | TaskState::Accepted
        | TaskState::Cancelled => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WaitGroup {
    Platform,
    StoppedByHuman,
}

/// Terminal states carry no live demand on the operator: their leftover
/// unanswered notes are not shown (the reference group speaks about the
/// line that could continue to need an answer).
pub fn is_note_visible(state: &TaskState) -> bool {
    !matches!(state, TaskState::Accepted | TaskState::Cancelled)
}

/// (demand, cli_hint) keyed by the stored token. The operations named here
/// are exactly the registry's Mutation ops the human runs in the CLI:
/// task.approve/task.reject (registry.rs:228-241), question.answer
/// (registry.rs:395-401), task.accept/task.return (registry.rs:242-255),
/// task.resume (registry.rs:263-269), task.ratify-empty-rework
/// (crates/devai-cli/src/main.rs:233).
///
/// REQ-GATE-06 AC2: the hint is a REAL command with real ids — the parked
/// question's id for WAITING_FOR_INPUT (never a literal `<вопрос-id>`),
/// and the empty-rework ratification when the WAITING task has no OPEN
/// registry question (that gate is the `task ratify-empty-rework` path).
pub fn wait_demand_for(
    state: &TaskState,
    id: &str,
    open_questions: &[Question],
) -> (String, String) {
    let (demand, verb): (String, String) = match state {
        TaskState::Approval => (
            "Утвердить или отклонить текущую спецификацию задачи".into(),
            "devai task approve {id} --rationale \"<обоснование>\" · devai task reject {id} --rationale \"<обоснование>\"".into(),
        ),
        // Only a BLOCKING question parks the task (workflow.rs:750-763), so
        // the answerable id is the blocking one; a stray non-blocking note
        // on the same task must never become the printed command (R6).
        TaskState::WaitingForInput => match open_questions
            .iter()
            .find(|q| q.blocking)
            .or_else(|| open_questions.first())
        {
            Some(question) => (
                "Ответить на припаркованный вопрос задачи".into(),
                format!(
                    "devai question answer {} --text \"<ответ>\"",
                    question.id
                ),
            ),
            None => (
                "Гейт пустого реворка: ратифицировать отсутствие новых изменений".into(),
                "devai task ratify-empty-rework {id} --rationale \"<обоснование>\"".into(),
            ),
        },
        TaskState::Qa => (
            "Подписать текущий quality-отчёт: принять или вернуть в реворк".into(),
            "devai task accept {id} --rationale \"<обоснование>\" · devai task return {id} --rationale \"<обоснование>\"".into(),
        ),
        TaskState::Failed => (
            "Решить по упавшей задаче: возобновить прогон".into(),
            "devai task resume {id}".into(),
        ),
        TaskState::Paused => (
            "Задача остановлена паузой оператора — возобновлять по вашему решению".into(),
            "devai task resume {id}".into(),
        ),
        TaskState::Blocked => (
            "Задача заблокирована оператором — снимать блок по вашему решению".into(),
            "devai task resume {id}".into(),
        ),
        // Non-wait states: wait_row is never built for them (wait_group_of
        // returns None above), so there is no demand and no command. Listed
        // exhaustively for the same reason as wait_group_of.
        TaskState::Draft
        | TaskState::Analysis
        | TaskState::Review
        | TaskState::Implementation
        | TaskState::Rework
        | TaskState::Accepted
        | TaskState::Cancelled => (
            "Платформа на этом состоянии на человеке не стоит".into(),
            String::new(),
        ),
    };
    (demand, verb.replace("{id}", id))
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

/// Shape-contract ledger of the command plane (R4, ux/contract-stability).
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

#[allow(dead_code)] // consumed by the contract test (R4)
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
    ("inbox_view", "task.list"),
    ("inbox_view", "question.list"),
    // The human-wait section's click presentations (REQ-GATE-04): all reads.
    ("inbox_view", "task.status"),
    ("inbox_view", "artifact.list"),
    ("inbox_view", "artifact.cat_text"),
    ("inbox_view", "task.report"),
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
/// (registry.rs:78-83) — the UI re-sorts nothing.
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
/// CANCELLED tokens as stored (types.rs:472-477).
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
/// questions (ratified OQ-GATE-1/2). Pure composition of published reads
/// (task.list + question.list + task.status + artifact.list +
/// artifact.cat_text + task.report); membership is decided by the exact
/// stored state tokens and recomputed on every load — nothing is cached
/// (REQ-GATE-07; REQ-UI-09's question-inbox precedent at commands.rs).
#[tauri::command]
pub fn inbox_view(state: State<AppState>) -> Result<WaitSnapshot, UiError> {
    let api = state.api();
    let mut waiting: Vec<WaitRow> = Vec::new();
    let mut human_stopped: Vec<WaitRow> = Vec::new();
    let mut notes: Vec<WaitNote> = Vec::new();
    for task in api.task_list()? {
        // Membership is the stored state token, never a UI guess
        // (REQ-GATE-02); the two groups never mix (OQ-GATE-2).
        match wait_group_of(&task.state) {
            Some(WaitGroup::Platform) => waiting.push(wait_row(&api, task)?),
            Some(WaitGroup::StoppedByHuman) => human_stopped.push(wait_row(&api, task)?),
            None => notes.extend(reference_notes(&api, &task)?),
        }
    }
    // REQ-GATE-08 ordering (longest-waiting first) and the badge are applied
    // by the composer below, on stored data only.
    Ok(build_wait_snapshot(waiting, human_stopped, notes))
}

/// Assemble the section from already-grouped rows: ordering by wait age
/// (REQ-GATE-08) and the badge live in this ONE place, so the AC3 invariant
/// `badge == waiting.len() + human_stopped.len() + notes.len()` (the
/// ratified OQ-GATE-1 shows the notes as rows, so they are counted) is
/// exercised by `badge_counts_every_section_row` rather than restated.
fn build_wait_snapshot(
    mut waiting: Vec<WaitRow>,
    mut human_stopped: Vec<WaitRow>,
    mut notes: Vec<WaitNote>,
) -> WaitSnapshot {
    sort_by_wait_age(&mut waiting);
    sort_by_wait_age(&mut human_stopped);
    notes.sort_by(|a, b| {
        a.task
            .updated_at
            .cmp(&b.task.updated_at)
            .then(a.question.created_at.cmp(&b.question.created_at))
    });
    let badge = waiting.len() + human_stopped.len() + notes.len();
    WaitSnapshot {
        waiting,
        human_stopped,
        notes,
        badge,
    }
}

/// One wait row, composed from published reads only: the demand copy
/// keyed by the stored token (REQ-GATE-03/06), the task's stored OPEN
/// registry questions, and the click-presentation of its gate type
/// (REQ-GATE-04) — spec for APPROVAL, report for QA.
fn wait_row(api: &Api, task: Task) -> Result<WaitRow, UiError> {
    let open_questions = open_questions_of(api, &task)?;
    let (demand, cli_hint) = wait_demand_for(&task.state, &short_id(&task.id), &open_questions);
    let spec = if task.state == TaskState::Approval {
        approval_spec(api, &task)?
    } else {
        None
    };
    let (report, report_note) = if task.state == TaskState::Qa {
        qa_report(api, &task)
    } else {
        (None, None)
    };
    Ok(WaitRow {
        task,
        demand,
        cli_hint,
        open_questions,
        spec,
        report,
        report_note,
    })
}

/// The ONE composition of registry questions: `question.list` filtered to
/// the stored OPEN token, plus the caller's predicate (R2, mt/dry). The
/// wait row wants all OPEN questions, the reference group only the
/// NON-blocking ones — one query shape, two predicates.
fn open_questions_where(
    api: &Api,
    task: &Task,
    keep: impl Fn(&Question) -> bool,
) -> Result<Vec<Question>, UiError> {
    Ok(api
        .question_list(&task.id.to_string())?
        .into_iter()
        .filter(|q| q.status == QuestionStatus::Open && keep(q))
        .collect())
}

/// Platform registry questions of the task, stored OPEN token only
/// (question.list composition, REQ-GATE-03 AC2).
fn open_questions_of(api: &Api, task: &Task) -> Result<Vec<Question>, UiError> {
    open_questions_where(api, task, |_| true)
}

/// QA presentation: the CURRENT quality report (registry.rs:155-161), or
/// the platform's own text when there is none — an honest fact, never an
/// empty hole (REQ-UI-07 AC2 precedent).
fn qa_report(api: &Api, task: &Task) -> (Option<QualityReportView>, Option<String>) {
    match api.task_report(&task.id.to_string()) {
        Ok(view) => (Some(view), None),
        Err(e) => (None, Some(UiError::from(e).message)),
    }
}

/// Reference-group rows (ratified OQ-GATE-1): stored OPEN NON-blocking
/// questions of a task the platform does NOT stand on — a note on an
/// active task (workflow.rs:747-748), never a wait row (it is a separate
/// group and the badge counts rows of the same snapshot). Terminal tasks
/// carry no live demand, so their leftovers stay out (is_note_visible).
fn reference_notes(api: &Api, task: &Task) -> Result<Vec<WaitNote>, UiError> {
    if !is_note_visible(&task.state) {
        return Ok(Vec::new());
    }
    Ok(open_questions_where(api, task, |q| !q.blocking)?
        .into_iter()
        .map(|question| WaitNote {
            task: task.clone(),
            question,
        })
        .collect())
}

fn sort_by_wait_age(rows: &mut [WaitRow]) {
    rows.sort_by(|a, b| a.task.updated_at.cmp(&b.task.updated_at));
}

/// The APPROVAL presentation (REQ-GATE-04/05): the CURRENT
/// REQUIREMENTS_SPEC via task.status.current_artifacts, its body via
/// artifact.cat_text; ratified OQ-GATE-3 — without a CURRENT spec the
/// row shows the LAST spec artifact marked "not current, approve is
/// impossible"; with no spec at all the frontend states that fact.
/// Parsing the JSON extracts the human paragraph and the document's
/// open_questions; the stored bytes are never rewritten.
fn approval_spec(api: &Api, task: &Task) -> Result<Option<SpecPresentation>, UiError> {
    let status: StatusView = api.task_status(&task.id.to_string())?;
    let chosen = status
        .current_artifacts
        .into_iter()
        .find(|a| a.kind == ArtifactKind::RequirementsSpec);
    let (artifact, current) = match chosen {
        Some(a) => (a, true),
        None => {
            let last = api
                .artifact_list(&task.id.to_string())?
                .into_iter()
                .filter(|a| a.kind == ArtifactKind::RequirementsSpec)
                .max_by_key(|a| (a.seq, a.created_at));
            match last {
                Some(a) => (a, false),
                None => return Ok(None),
            }
        }
    };
    let body: Result<String, String> = match api.artifact_text(&artifact.id.to_string()) {
        Ok(text) => Ok(text),
        // Binary spec content is the honest binary fact with its size —
        // raw bytes are never rendered (ArtifactContent precedent above).
        Err(ApiError::NotUtf8) => Err(format!(
            "артефакт бинарный (не UTF-8, {} байт)",
            artifact.size
        )),
        Err(e) => return Err(e.into()),
    };
    let body = match &body {
        Ok(text) => Ok(text.as_str()),
        Err(reason) => Err(reason.as_str()),
    };
    Ok(Some(spec_presentation(&artifact, body, task, current)))
}

#[cfg(test)]
mod tests {
    use super::*;

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

    /// R4 (ts/coverage-static): the serde output of the ipc command core is
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

    /// REQ-GATE-02 AC2 + ratified OQ-GATE-2: membership is the exact
    /// stored state tokens — the platform's own wait set (APPROVAL,
    /// WAITING_FOR_INPUT, QA, FAILED), the separate operator-stopped set
    /// (PAUSED/BLOCKED), and nothing else. An ACTIVE task carrying only a
    /// non-blocking OPEN question creates no wait row (workflow.rs:747-748).
    ///
    /// R3: the table below names EVERY TaskState variant, so a new variant
    /// cannot be added without a membership decision here (the exhaustive
    /// `wait_group_of` match already fails the build; this pins the
    /// expected verdict per variant).
    #[test]
    fn wait_membership_is_the_state_guard_not_a_guess() {
        use WaitGroup::{Platform, StoppedByHuman};
        let table: [(TaskState, Option<WaitGroup>); 13] = [
            (TaskState::Draft, None),
            (TaskState::Analysis, None),
            (TaskState::Review, None),
            (TaskState::Approval, Some(Platform)),
            (TaskState::Implementation, None),
            (TaskState::Qa, Some(Platform)),
            (TaskState::Rework, None),
            (TaskState::WaitingForInput, Some(Platform)),
            (TaskState::Paused, Some(StoppedByHuman)),
            (TaskState::Blocked, Some(StoppedByHuman)),
            (TaskState::Failed, Some(Platform)),
            (TaskState::Accepted, None),
            (TaskState::Cancelled, None),
        ];
        for (state, expected) in table {
            assert_eq!(wait_group_of(&state), expected, "{state:?}");
        }
        // The reference group never speaks for terminal tasks.
        assert!(!is_note_visible(&TaskState::Accepted));
        assert!(!is_note_visible(&TaskState::Cancelled));
        assert!(is_note_visible(&TaskState::Implementation));
    }

    /// REQ-GATE-03 AC1 + REQ-GATE-06 AC2: every wait type carries human
    /// wording of the demand and the literal CLI command; the tokens of
    /// the three platform gate types stay distinguishable. The WAITING hint
    /// must be runnable as printed: a real question id when one is parked,
    /// the empty-rework ratification when there is none (R6).
    #[test]
    fn wait_demand_names_the_operation_and_the_cli() {
        // Ратифицированная «дословная команда» = фактический синтаксис CLI:
        // rationale/text — --long-опции (crates/devai-cli/src/main.rs,
        // Approve/Reject/Accept/Return/Answer), не позиционные аргументы.
        let (demand, hint) = wait_demand_for(&TaskState::Approval, "01a1b0ea", &[]);
        assert!(demand.contains("спецификацию"), "{demand}");
        assert!(hint.contains("task approve 01a1b0ea --rationale"), "{hint}");
        assert!(hint.contains("task reject 01a1b0ea --rationale"), "{hint}");

        // Parked question: the real id, not a literal placeholder.
        let parked = question_fixture("01a1b0ea-0000-4000-8000-0000000000bb", true);
        let (_, hint) = wait_demand_for(&TaskState::WaitingForInput, "x", &[parked]);
        assert!(
            hint.contains("question answer 01a1b0ea-0000-4000-8000-0000000000bb --text"),
            "{hint}"
        );
        assert!(!hint.contains("<вопрос-id>"), "{hint}");

        // A stray non-blocking note beside the parked blocking question must
        // not steal the printed id: only the blocking one is answerable.
        let note = question_fixture("01a1b0ea-0000-4000-8000-0000000000cc", false);
        let parked = question_fixture("01a1b0ea-0000-4000-8000-0000000000bb", true);
        let (_, hint) = wait_demand_for(&TaskState::WaitingForInput, "x", &[note, parked]);
        assert!(
            hint.contains("question answer 01a1b0ea-0000-4000-8000-0000000000bb --text"),
            "{hint}"
        );

        // No OPEN question = the empty-rework gate: the real escape command.
        let (demand, hint) = wait_demand_for(&TaskState::WaitingForInput, "x", &[]);
        assert!(demand.contains("пустого реворка"), "{demand}");
        assert!(
            hint.contains("task ratify-empty-rework x --rationale"),
            "{hint}"
        );

        let (demand, hint) = wait_demand_for(&TaskState::Qa, "abc123", &[]);
        assert!(demand.contains("quality-отчёт"), "{demand}");
        assert!(hint.contains("task accept abc123 --rationale"), "{hint}");
        assert!(hint.contains("task return abc123 --rationale"), "{hint}");
        let (_, hint) = wait_demand_for(&TaskState::Failed, "f1", &[]);
        assert!(hint.contains("task resume f1"), "{hint}");

        // A non-wait state has no demand and no command — honest emptiness,
        // not a placeholder token dressed as a command (R1).
        let (demand, hint) = wait_demand_for(&TaskState::Implementation, "i1", &[]);
        assert!(demand.contains("не стоит"), "{demand}");
        assert!(hint.is_empty(), "{hint}");
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

    /// Badge discipline (REQ-GATE-08 AC3): the badge is the number of rows
    /// of the section on the same snapshot — waiting + human-stopped + the
    /// ratified OQ-GATE-1 reference notes (they are shown rows, not a
    /// hidden bucket). Asserted through `build_wait_snapshot` itself, the
    /// same function `inbox_view` ends with, so a badge formula that
    /// dropped the notes would fail here.
    #[test]
    fn badge_counts_every_section_row() {
        let now = now_fixture();
        let snapshot = build_wait_snapshot(
            vec![WaitRow {
                task: task_fixture("a", "", now),
                demand: "w".into(),
                cli_hint: "h".into(),
                open_questions: vec![],
                spec: None,
                report: None,
                report_note: None,
            }],
            vec![WaitRow {
                task: task_fixture("b", "", now),
                demand: "w".into(),
                cli_hint: "h".into(),
                open_questions: vec![],
                spec: None,
                report: None,
                report_note: None,
            }],
            vec![WaitNote {
                task: task_fixture("c", "", now),
                question: Question {
                    id: devai_core::types::new_id(),
                    task_id: devai_core::types::new_id(),
                    execution_id: None,
                    text: "заметка".into(),
                    context: None,
                    options: vec![],
                    blocking: false,
                    status: QuestionStatus::Open,
                    answer: None,
                    answered_by: None,
                    answered_at: None,
                    created_at: now,
                },
            }],
        );
        // 1 waiting + 1 human-stopped + 1 note.
        assert_eq!(snapshot.badge, 3);
        assert_eq!(
            snapshot.badge,
            snapshot.waiting.len() + snapshot.human_stopped.len() + snapshot.notes.len()
        );
    }

    /// R3 (ux/contract-stability): the two shapes the wait section
    /// publishes are pinned exactly like `ui_error`/`artifact_*` — a
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

    /// R4 (ux/contract-stability): a breaking command shape is a RECORDED,
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
            let (demand, cli_hint) = wait_demand_for(&state, id, &questions);
            actual.insert(
                token.to_string(),
                serde_json::json!({ "demand": demand, "cli_hint": cli_hint }),
            );
        }
        assert_eq!(
            serde_json::Value::Object(actual),
            fixture["rows"],
            "row texts drifted from the pinned snapshot (R4)"
        );
    }

    /// R5 (arch/no-magic): `frontend/src/states.ts` mirrors EVERY stored
    /// TaskState token of Rust, and the view compares through that shared
    /// const instead of carrying raw literals — a Rust token rename fails
    /// HERE; a raw literal in the template fails the second half.
    #[test]
    fn states_ts_mirrors_rust_task_state_tokens() {
        let crate_dir = env!("CARGO_MANIFEST_DIR");
        let ts = std::fs::read_to_string(format!("{crate_dir}/frontend/src/states.ts"))
            .expect("frontend/src/states.ts must exist (R5)");
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
}
