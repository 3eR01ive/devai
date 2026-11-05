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
use devai_core::types::{
    Artifact, Decision, Project, Question, QuestionStatus, RoleExecution, Task, TaskState,
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

/// One inbox row (REQ-UI-09): a task parked in WAITING_FOR_INPUT plus its
/// OPEN questions. Composition of published reads only
/// (task.list + question.list); the parking itself is platform data —
/// the UI never "guesses" that a task waits.
#[derive(Serialize, Clone, Debug)]
pub struct InboxEntry {
    pub task: Task,
    pub open_questions: Vec<Question>,
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
    ("artifacts_list", "artifact.list"),
    ("artifact_content", "artifact.cat_text"),
    ("artifact_content", "artifact.cat"),
    ("questions_list", "question.list"),
    ("executions_list", "exec.list"),
    ("execution_liveness", "exec.liveness"),
    ("inbox_view", "task.list"),
    ("inbox_view", "question.list"),
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

/// The human inbox (REQ-UI-09): WAITING_FOR_INPUT tasks of the home and
/// their OPEN questions, without opening each task. Pure composition of
/// the two published reads; states are matched by their exact tokens.
#[tauri::command]
pub fn inbox_view(state: State<AppState>) -> Result<Vec<InboxEntry>, UiError> {
    let api = state.api();
    let waiting: Vec<Task> = api
        .task_list()?
        .into_iter()
        .filter(|t| t.state == TaskState::WaitingForInput)
        .collect();
    let mut out = Vec::with_capacity(waiting.len());
    for t in waiting {
        let open_questions: Vec<Question> = api
            .question_list(&t.id.to_string())?
            .into_iter()
            .filter(|q| q.status == QuestionStatus::Open)
            .collect();
        out.push(InboxEntry {
            task: t,
            open_questions,
        });
    }
    Ok(out)
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
}
