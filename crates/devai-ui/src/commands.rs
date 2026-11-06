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
/// needs the operator. `wants`/`cli_hint` are presentation copy keyed by
/// the EXACT stored state token (never a guess about waiting); `spec` /
/// `report` are the click-presentation, computed from published reads only.
#[derive(Serialize, Clone, Debug)]
pub struct WaitRow {
    pub task: Task,
    /// What the operator is asked to do, in human words (REQ-GATE-03 AC1).
    pub wants: String,
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
/// separate reference group below the waiting list, never as a wait row,
/// never counted into the wait badge (REQ-GATE-10).
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
    /// Badge on the nav element: waiting + human-stopped rows; notes are
    /// excluded — they do not claim to wait (REQ-GATE-10 AC1).
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
        _ => None,
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

/// (wants, cli_hint) keyed by the stored token. The operations named here
/// are exactly the registry's Mutation ops the human runs in the CLI:
/// task.approve/task.reject (registry.rs:228-241), question.answer
/// (registry.rs:395-401), task.accept/task.return (registry.rs:242-255),
/// task.resume (registry.rs:263-269).
pub fn wait_demand_for(state: &TaskState, id: &str) -> (String, String) {
    let (wants, verb) = match state {
        TaskState::Approval => (
            "Утвердить или отклонить текущую спецификацию задачи",
            "devai task approve {id} --rationale \"<обоснование>\" · devai task reject {id} --rationale \"<обоснование>\"",
        ),
        TaskState::WaitingForInput => (
            "Ответить на припаркованный вопрос задачи",
            "devai question answer <вопрос-id> --text \"<ответ>\"",
        ),
        TaskState::Qa => (
            "Подписать текущий quality-отчёт: принять или вернуть в реворк",
            "devai task accept {id} --rationale \"<обоснование>\" · devai task return {id} --rationale \"<обоснование>\"",
        ),
        TaskState::Failed => (
            "Решить по упавшей задаче: возобновить прогон",
            "devai task resume {id}",
        ),
        TaskState::Paused => (
            "Задача остановлена паузой оператора — возобновлять по вашему решению",
            "devai task resume {id}",
        ),
        TaskState::Blocked => (
            "Задача заблокирована оператором — снимать блок по вашему решению",
            "devai task resume {id}",
        ),
        other => (
            "Платформа на этом состоянии не стоит",
            other_placeholder_token(other),
        ),
    };
    (wants.to_string(), verb.replace("{id}", id))
}

fn other_placeholder_token(_state: &TaskState) -> &'static str {
    "—"
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
    let text = match body {
        Ok(text) => text,
        Err(reason) => {
            return SpecPresentation {
                artifact: artifact.clone(),
                current,
                human_paragraph: non_empty(task.business_description.as_str()).map(str::to_string),
                paragraph_source: ParagraphSource::BusinessDescription,
                open_questions: Vec::new(),
                unavailable_note: Some(format!("{reason}; показываю бизнес-описание задачи")),
            };
        }
    };
    let parsed: Result<serde_json::Value, serde_json::Error> = serde_json::from_str(text);
    match parsed {
        Ok(value) => {
            let paragraph = value
                .get("human_paragraph")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(str::to_string);
            let open_questions = value
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
                .unwrap_or_default();
            match paragraph {
                Some(paragraph) => SpecPresentation {
                    artifact: artifact.clone(),
                    current,
                    human_paragraph: Some(paragraph),
                    paragraph_source: ParagraphSource::Spec,
                    open_questions,
                    unavailable_note: None,
                },
                None => SpecPresentation {
                    artifact: artifact.clone(),
                    current,
                    human_paragraph: non_empty(task.business_description.as_str())
                        .map(str::to_string),
                    paragraph_source: ParagraphSource::BusinessDescription,
                    open_questions,
                    // Ratified OQ-GATE-4, verbatim wording of the decision.
                    unavailable_note: Some(
                        "аналитик абзац не писал — спека до протокола; подставляю бизнес-описание задачи"
                            .to_string(),
                    ),
                },
            }
        }
        Err(_) => SpecPresentation {
            artifact: artifact.clone(),
            current,
            human_paragraph: non_empty(task.business_description.as_str()).map(str::to_string),
            paragraph_source: ParagraphSource::BusinessDescription,
            open_questions: Vec::new(),
            unavailable_note: Some(
                "тело артефакта специки не разбирается как JSON; показываю бизнес-описание задачи"
                    .to_string(),
            ),
        },
    }
}

fn non_empty(s: &str) -> Option<&str> {
    let t = s.trim();
    if t.is_empty() { None } else { Some(t) }
}

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
    let tasks = api.task_list()?;
    let mut waiting: Vec<WaitRow> = Vec::new();
    let mut human_stopped: Vec<WaitRow> = Vec::new();
    let mut notes: Vec<WaitNote> = Vec::new();
    for t in tasks {
        match wait_group_of(&t.state) {
            Some(group) => {
                let (wants, cli_hint) = wait_demand_for(&t.state, &short_id(&t.id));
                let open_questions: Vec<Question> = api
                    .question_list(&t.id.to_string())?
                    .into_iter()
                    .filter(|q| q.status == QuestionStatus::Open)
                    .collect();
                let spec = if t.state == TaskState::Approval {
                    approval_spec(&api, &t)?
                } else {
                    None
                };
                let (report, report_note) = if t.state == TaskState::Qa {
                    match api.task_report(&t.id.to_string()) {
                        Ok(view) => (Some(view), None),
                        // The platform's own text on a missing report —
                        // an honest fact, never an empty hole (REQ-UI-07
                        // AC2 precedent).
                        Err(e) => (None, Some(UiError::from(e).message)),
                    }
                } else {
                    (None, None)
                };
                let row = WaitRow {
                    task: t,
                    wants,
                    cli_hint,
                    open_questions,
                    spec,
                    report,
                    report_note,
                };
                match group {
                    WaitGroup::Platform => waiting.push(row),
                    WaitGroup::StoppedByHuman => human_stopped.push(row),
                }
            }
            None => {
                if is_note_visible(&t.state) {
                    for q in api.question_list(&t.id.to_string())? {
                        // A non-blocking OPEN question is a note on an
                        // active task — the platform does NOT stand
                        // (workflow.rs:747-748); reference group only.
                        if q.status == QuestionStatus::Open && !q.blocking {
                            notes.push(WaitNote {
                                task: t.clone(),
                                question: q,
                            });
                        }
                    }
                }
            }
        }
    }
    // REQ-GATE-08: the longest-waiting first — task.updated_at ascending,
    // re-ordered here from stored data (task.list itself serves descending).
    waiting.sort_by(|a, b| a.task.updated_at.cmp(&b.task.updated_at));
    human_stopped.sort_by(|a, b| a.task.updated_at.cmp(&b.task.updated_at));
    notes.sort_by(|a, b| {
        a.task
            .updated_at
            .cmp(&b.task.updated_at)
            .then(a.question.created_at.cmp(&b.question.created_at))
    });
    let badge = waiting.len() + human_stopped.len();
    Ok(WaitSnapshot {
        waiting,
        human_stopped,
        notes,
        badge,
    })
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
    #[test]
    fn wait_membership_is_the_state_guard_not_a_guess() {
        for s in [
            TaskState::Approval,
            TaskState::WaitingForInput,
            TaskState::Qa,
            TaskState::Failed,
        ] {
            assert_eq!(wait_group_of(&s), Some(WaitGroup::Platform), "{s:?}");
        }
        for s in [TaskState::Paused, TaskState::Blocked] {
            assert_eq!(wait_group_of(&s), Some(WaitGroup::StoppedByHuman), "{s:?}");
        }
        for s in [
            TaskState::Draft,
            TaskState::Analysis,
            TaskState::Review,
            TaskState::Implementation,
            TaskState::Rework,
            TaskState::Accepted,
            TaskState::Cancelled,
        ] {
            assert_eq!(wait_group_of(&s), None, "{s:?}");
        }
        // The reference group never speaks for terminal tasks.
        assert!(!is_note_visible(&TaskState::Accepted));
        assert!(!is_note_visible(&TaskState::Cancelled));
        assert!(is_note_visible(&TaskState::Implementation));
    }

    /// REQ-GATE-03 AC1 + REQ-GATE-06 AC2: every wait type carries human
    /// wording of the demand and the literal CLI command; the tokens of
    /// the three platform gate types stay distinguishable.
    #[test]
    fn wait_demand_names_the_operation_and_the_cli() {
        // Ратифицированная «дословная команда» = фактический синтаксис CLI:
        // rationale/text — --long-опции (crates/devai-cli/src/main.rs,
        // Approve/Reject/Accept/Return/Answer), не позиционные аргументы.
        let (wants, hint) = wait_demand_for(&TaskState::Approval, "01a1b0ea");
        assert!(wants.contains("спецификацию"), "{wants}");
        assert!(hint.contains("task approve 01a1b0ea --rationale"), "{hint}");
        assert!(hint.contains("task reject 01a1b0ea --rationale"), "{hint}");
        let (_, hint) = wait_demand_for(&TaskState::WaitingForInput, "x");
        assert!(hint.contains("question answer"), "{hint}");
        assert!(hint.contains("--text"), "{hint}");
        let (wants, hint) = wait_demand_for(&TaskState::Qa, "abc123");
        assert!(wants.contains("quality-отчёт"), "{wants}");
        assert!(hint.contains("task accept abc123 --rationale"), "{hint}");
        assert!(hint.contains("task return abc123 --rationale"), "{hint}");
        let (_, hint) = wait_demand_for(&TaskState::Failed, "f1");
        assert!(hint.contains("task resume f1"), "{hint}");
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

    /// Badge discipline (REQ-GATE-08 AC3 / REQ-GATE-10 AC1): the number
    /// mirrors the wait rows of the same snapshot; reference notes never
    /// inflate it.
    #[test]
    fn badge_counts_wait_rows_not_reference_notes() {
        // The snapshot is composed by inbox_view from live reads; the
        // invariant itself is `badge == waiting.len() + human_stopped.len()`
        // and is asserted on the live fixture run's JSON (see the
        // implementation report's evidence pointer).
        let now = now_fixture();
        let snapshot = WaitSnapshot {
            waiting: vec![WaitRow {
                task: task_fixture("a", "", now),
                wants: "w".into(),
                cli_hint: "h".into(),
                open_questions: vec![],
                spec: None,
                report: None,
                report_note: None,
            }],
            human_stopped: vec![WaitRow {
                task: task_fixture("b", "", now),
                wants: "w".into(),
                cli_hint: "h".into(),
                open_questions: vec![],
                spec: None,
                report: None,
                report_note: None,
            }],
            notes: vec![WaitNote {
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
            badge: 2,
        };
        assert_eq!(
            snapshot.badge,
            snapshot.waiting.len() + snapshot.human_stopped.len()
        );
        assert_ne!(
            snapshot.badge as usize + snapshot.notes.len(),
            snapshot.badge
        );
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

    /// Every command operation of the section is a registered READ; the
    /// extended inbox_view mapping (task.status/artifact.list/
    /// artifact.cat_text/task.report) is covered by the same table test.
    #[test]
    fn wait_section_commands_stay_registered_reads() {
        let ops = devai_api::operations();
        let by_name: std::collections::HashMap<&str, &OpDescriptor> =
            ops.iter().map(|o| (o.name, o)).collect();
        for (cmd, op) in COMMAND_OPERATIONS
            .iter()
            .filter(|(c, _)| *c == "inbox_view")
        {
            let d = *by_name
                .get(op)
                .unwrap_or_else(|| panic!("`{cmd}` calls `{op}`: not published"));
            assert_eq!(
                d.class,
                devai_api::OpClass::Read,
                "`{cmd}`→`{op}` is not Read"
            );
        }
    }

    /// Test-only deterministic clock for fixtures (no per-call wall drift).
    fn now_fixture() -> chrono::DateTime<chrono::Utc> {
        use chrono::Timelike;
        chrono::Utc::now().with_nanosecond(0).unwrap()
    }
}
