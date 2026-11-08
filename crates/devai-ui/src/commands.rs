//! The command plane of devai-ui: every data the window renders is one
//! published READ operation of the API registry (REQ-UI-02). The frontend
//! never receives anything else — there is no store handle reachable from
//! this crate, and no domain verdict is re-derived here: typed views are
//! passed through exactly as `devai-api` produced them
//! (store-is-cache / no read-time rederivation).
//!
//! The UI is read-only for DOMAIN data except the TWO signature mutations
//! (REQ-SIGN-05): `task_approve` / `task_reject` are the operator's signature
//! acts and call the SAME `Api` methods as the CLI — one mutation, one
//! event/decision, no separate window path. Every other gate action stays in
//! the CLI (OQ-UI-6 narrowed exactly to these two). The homes REGISTRY plane
//! is a separate, allowed class of platform operations (REQ-HOME-06):
//! `home.add`, `home.set_enabled`, `home.activate` write ONLY the
//! registry file of the control home and never touch any home's domain
//! rows. The mapping tables pin the split: `COMMAND_OPERATIONS` maps the
//! data commands to published Read operations plus the two signature
//! mutations listed in `SIGNATURE_MUTATION_COMMANDS`, and
//! `COMMAND_REGISTRY_OPERATIONS` maps the management commands to the
//! home-plane Registry operations — no other Mutation is reachable.

use serde::Serialize;
use tauri::State;

use chrono::Utc;
use devai_api::{ActiveSnapshot, DependenciesView, ProgressView, QualityReportView, StatusView};
use devai_api::{Api, ApiError, HomeRecord, HomeRecordsView, OpDescriptor};
use devai_api::{BodyError, SignatureView, SpecPresentation, spec_presentation};
use devai_api::{InboxSnapshotView, InboxTaskFacts, SpecBodyView, SpecFacts};
use devai_api::{WorkspaceChangesView, WorkspaceDiffView};
use devai_core::inbox::{
    InboxWaitPolicy, WHEN_NO_QUESTION, WHEN_PARKED_QUESTION, WaitGroup, WaitStatePolicy,
};
use devai_core::types::{
    Artifact, Decision, Project, Question, QuestionStatus, RoleExecution, Task, TaskState, short_id,
};

use crate::session::{HomeFailure, SessionInfo};

/// Shared state: the open `Api` session behind a mutex (the API takes
/// `&self`; the store is WAL — concurrent readers with the engine are the
/// ratified shape, REQ-UI-12), the homes registry of the control home
/// (REQ-HOME-01), plus the facts for the status bar.
pub struct AppState {
    pub api: std::sync::Mutex<Api>,
    pub homes: std::sync::Mutex<devai_api::Homes>,
    pub session: std::sync::Mutex<SessionInfo>,
}

impl AppState {
    fn api(&self) -> std::sync::MutexGuard<'_, Api> {
        // Poisoned only if a command panicked mid-call; the reads leave no
        // mutation, so the handle is still sound.
        self.api.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn homes(&self) -> std::sync::MutexGuard<'_, devai_api::Homes> {
        self.homes.lock().unwrap_or_else(|p| p.into_inner())
    }

    fn session(&self) -> std::sync::MutexGuard<'_, SessionInfo> {
        self.session.lock().unwrap_or_else(|p| p.into_inner())
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
}

// The spec/review parse rules are DEVAI-API's (REQ-SIGN-12): the window
// imports the projections, it never re-declares them. `SpecPresentation`
// keeps the wait-row shape it always published.

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

/// The spec parse lives in `devai_api::signature` (REQ-SIGN-12) — one
/// module, one rule set, shared by the wait row and the signature card.
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
pub const COMMAND_CONTRACTS: &[CommandContract] = &[
    CommandContract {
        command: "inbox_view",
        version: 2,
        breaking: "v2 (2026-11-06): inbox_view заменил InboxEntry[] на WaitSnapshot (REQ-GATE-01..10) — прежней формы больше нет; frontend/src/types.ts зеркалит v2.",
    },
    // The signature card's surface ships at its first recorded shape: no
    // breaking revision yet, so v1 with an empty note.
    CommandContract {
        command: "task_signature",
        version: 1,
        breaking: "",
    },
    CommandContract {
        command: "task_approve",
        version: 1,
        breaking: "",
    },
    CommandContract {
        command: "task_reject",
        version: 1,
        breaking: "",
    },
];

/// The (command → registry operation) mapping. Every data command is
/// backed by a published Read operation; `session_info` is bootstrap state
/// (`Api::open`, the ratified OQ-10 exception), not an operation.
///
/// A new UI command touches FIVE places — the published table in
/// `devai-api/registry.rs`, the two COMMAND_* tables here, `generate_handler!`
/// in main.rs, and the frontend pair (api.ts + types.ts). None of them can
/// half-land silently: the CLI's `match OpClass` is compile-guarded,
/// `frontend_invokes_are_exactly_mapped_reads` guards the frontend→table
/// direction, `every_published_home_operation_is_wired_into_the_ui` guards
/// the registry→table direction (mt/change-isolation: the checklist is
/// explicit and every entry has a mechanical keeper).
#[allow(dead_code)] // consumed by the tests below + mirrors OpsView's `used` list
pub const COMMAND_OPERATIONS: &[(&str, &str)] = &[
    ("registry_operations", "api.registry"),
    ("projects_list", "project.list"),
    ("projects_get", "project.get"),
    ("tasks_list", "task.list"),
    ("task_get", "task.get"),
    ("task_status", "task.status"),
    // The signable surface of the APPROVAL card: ONE read for the whole
    // section (REQ-SIGN-07), registered beside task.status.
    ("task_signature", "task.signature"),
    // The two signature mutations (REQ-SIGN-05 AC4): the SAME platform acts
    // as `devai task approve/reject` — the window invents no path of its own.
    ("task_approve", "task.approve"),
    ("task_reject", "task.reject"),
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
    // Merged multi-home reads (REQ-HOME-04/05): live attributed views over
    // the enabled homes of the registry, published as home-plane Reads.
    ("homes_list", "home.list"),
    ("projects_all", "home.projects_all"),
    ("tasks_all", "home.tasks_all"),
];

/// The ONE allowlist of signature MUTATIONS reachable from the window
/// (REQ-SIGN-05 AC4): exactly two pairs, no Read row, no third pair. The
/// data plane stays read-only; these two are the only exception (the
/// operator signs in the window) and they call the same API methods the CLI
/// calls.
#[allow(dead_code)] // consumed by the plane tests below
pub const SIGNATURE_MUTATION_COMMANDS: &[(&str, &str)] = &[
    ("task_approve", "task.approve"),
    ("task_reject", "task.reject"),
];

/// The registry MANAGEMENT mapping (REQ-HOME-05: reading domain records
/// and managing the registry are separate classes and never mix in one
/// operation). These commands call Registry-class operations whose single
/// writable target is `homes.json` of the control home — no command here
/// mutates any home's domain rows, and none reaches a Mutation-class op.
#[allow(dead_code)] // consumed by the separation test below
pub const COMMAND_REGISTRY_OPERATIONS: &[(&str, &str)] = &[
    ("homes_add", "home.add"),
    ("homes_set_enabled", "home.set_enabled"),
    ("homes_activate", "home.activate"),
];

// ---------------------------------------------------------------------
// session / registry
// ---------------------------------------------------------------------

/// Bootstrap facts for the title/status bar (REQ-UI-01 AC3, plus the
/// homes-cycle facts: where the registry lives and whether the session is
/// in a degraded observation open).
#[tauri::command]
pub fn session_info(state: State<AppState>) -> SessionInfo {
    state.session().clone()
}

/// `api.registry` — the published operation contract, shown in the UI so
/// every panel stays comparable to a registry row (REQ-UI-02 AC2).
#[tauri::command]
pub fn registry_operations(state: State<AppState>) -> Result<Vec<OpDescriptor>, UiError> {
    Ok(state.api().api_registry())
}

// ---------------------------------------------------------------------
// homes registry (REQ-HOME-01…09)
// ---------------------------------------------------------------------

/// `home.list` — the stored registry with live-probed attributes
/// (availability, schema rank, version/read cause). Absent homes stay
/// visible with their path; nothing is invented (REQ-HOME-07).
#[tauri::command]
pub fn homes_list(state: State<AppState>) -> Result<Vec<HomeRecord>, UiError> {
    Ok(state.homes().records()?)
}

/// `home.add` — register an existing devai home. Writes only the registry
/// record into the control home; a missing directory or a directory
/// without a store refuses with the platform's typed error and the window
/// keeps running (REQ-HOME-03 AC3).
#[tauri::command(rename_all = "snake_case")]
pub fn homes_add(state: State<AppState>, path: String) -> Result<HomeRecord, UiError> {
    Ok(state.homes().add(std::path::Path::new(&path))?)
}

/// `home.set_enabled` — include/exclude a home without restarting the
/// process (REQ-HOME-03). Removal from the view IS disabling; the record
/// and the home's data on disk survive (ratified OQ-HOME-3).
///
/// Ratified OQ-HOME-8: disabling the ACTIVE home is allowed and its
/// outcome is defined by the platform (next enabled record, or no active
/// home at all). This command therefore also follows the stored marker:
/// the session reopens the home the registry now names. When no home
/// remains active the session DEGRADES to observing the control home and
/// reports the honest `home_none_active` fact — and the control home may
/// itself be a record the operator has switched off (it is the home the
/// registry file lives in, not necessarily an enabled record), so "the
/// handle follows the registry" is the guarantee, not "the handle always
/// points at an enabled home".
#[tauri::command(rename_all = "snake_case")]
pub fn homes_set_enabled(
    state: State<AppState>,
    path: String,
    enabled: bool,
) -> Result<Vec<HomeRecord>, UiError> {
    let records = state.homes().set_enabled(&path, enabled)?;
    let (actor, control_home) = {
        let s = state.session();
        (s.actor.clone(), s.control_home.clone())
    };
    let stored_active = state.homes().active_entry().map(|e| e.path.clone());
    follow_active(&state, stored_active, &actor, &control_home);
    Ok(records)
}

/// Rebind the session handle to the stored active home after a registry
/// write changed it (OQ-HOME-8 handover). The handle the window reads
/// through must be the home the registry names, without a restart:
///   * unchanged active home → nothing to do;
///   * a new active home → reopen it as an OBSERVATION (REQ-HOME-09: no
///     migration by the act of watching a home);
///   * no active home → observe the control home and mark the session with
///     the `home_none_active` code so the window says so plainly and the
///     operator gets the enable/add affordances of the homes view.
/// A reopen refusal degrades to the control home with its own typed cause
/// (the REQ-HOME-07 shape) instead of leaving a dead handle behind.
///
/// Three units, ≤40 lines each (R2 quality return, cl/short-units): pick
/// the handle (`reopen_active`) → apply it to the session
/// (`apply_session_home`).
fn follow_active(state: &State<AppState>, active: Option<String>, actor: &str, control_home: &str) {
    let current = state.session().home.clone();
    if active.as_deref() == Some(current.as_str()) {
        return;
    }
    let control = std::path::PathBuf::from(control_home);
    let (api, failure) = reopen_active(&control, active.as_deref(), actor);
    apply_session_home(state, api, failure, control_home);
}

/// The handle for the stored active marker: the active home observed
/// read-only, or — when there is no readable active home — the control
/// home plus the typed degradation the window shows. The failure never
/// hides the registry write that already committed.
fn reopen_active(
    control: &std::path::Path,
    active: Option<&str>,
    actor: &str,
) -> (Result<Api, UiError>, Option<HomeFailure>) {
    let on_disk = active.filter(|p| {
        std::path::Path::new(p)
            .join(devai_api::DB_FILE_NAME)
            .exists()
    });
    match on_disk {
        Some(path) => match Api::open_observed(std::path::Path::new(path), actor) {
            Ok(api) => (Ok(api), None),
            Err(e) => {
                let failure = HomeFailure {
                    code: devai_api::codes::home_failure_code(e.code().0),
                    home: path.to_string(),
                    message: format!("cannot open home {path}: {e}"),
                };
                (observe_control(control, actor), Some(failure))
            }
        },
        None => {
            let failure = match active {
                // ONE source with the startup path (R3 quality return): a
                // directory without a store is `home_no_store`, a vanished
                // path is `home_unavailable` — the tokens cannot drift
                // between bootstrap and the post-registry rebind.
                Some(path) => crate::session::gone_failure(std::path::Path::new(path)),
                None => no_active_failure(control),
            };
            (observe_control(control, actor), Some(failure))
        }
    }
}

/// The degradation fact of a registry with NO active record (every home is
/// switched off, ratified OQ-HOME-8): one source with the startup bootstrap
/// (`crate::session::no_active_failure`), so the two tokens cannot drift.
fn no_active_failure(control: &std::path::Path) -> HomeFailure {
    crate::session::no_active_failure(control)
}

/// Observe the control home (no migration of a watched home, REQ-HOME-09);
/// the bootstrap open is the last resort for a control home that cannot be
/// observed read-only.
fn observe_control(control: &std::path::Path, actor: &str) -> Result<Api, UiError> {
    Api::open_observed(control, actor)
        .or_else(|_| Api::open(control, actor))
        .map_err(UiError::from)
}

/// Install the chosen handle as THE session handle and record the
/// degradation fact it came with (none when the stored active home opened).
fn apply_session_home(
    state: &State<AppState>,
    api: Result<Api, UiError>,
    failure: Option<HomeFailure>,
    control_home: &str,
) {
    match api {
        Ok(api) => {
            let info_home = api.home().display().to_string();
            *state.api() = api;
            let mut info = state.session();
            info.home = info_home;
            info.opened_at = Utc::now();
            info.degraded_from = failure;
        }
        Err(e) => {
            // The control home itself refused observation: the registry
            // write already committed, so the window keeps running with a
            // typed error instead of a panic.
            state.session().degraded_from = Some(HomeFailure {
                code: devai_api::codes::HOMES_REGISTRY_UNAVAILABLE,
                home: control_home.to_string(),
                message: e.message,
            });
        }
    }
}

/// `home.activate` — make an enabled home the single stored active home
/// in one click, no process restart (REQ-HOME-06 AC1). The whole
/// validate→re-probe→reopen→commit transaction is the API plane's
/// `Api::activate_home` — platform mechanics live with the platform
/// (report v1, da/architecture-consistency); this command only wires the
/// outcome into the session state. Returns the fresh session facts; the
/// frontend resets the view context to projects afterwards (ratified
/// OQ-HOME-5).
#[tauri::command(rename_all = "snake_case")]
pub fn homes_activate(state: State<AppState>, path: String) -> Result<SessionInfo, UiError> {
    let (actor, control_home) = {
        let s = state.session();
        (s.actor.clone(), s.control_home.clone())
    };
    let api = {
        let mut homes = state.homes();
        Api::activate_home(&mut homes, &path, &actor)?
    };
    let info = SessionInfo {
        home: api.home().display().to_string(),
        actor,
        opened_at: Utc::now(),
        control_home,
        degraded_from: None,
    };
    *state.api() = api;
    *state.session() = info.clone();
    Ok(info)
}

/// `home.projects_all` — the merged projects view over the enabled homes
/// (REQ-HOME-04): one attributed block per home, records read live from
/// the source home, a dead home localized to its own block. `home`
/// narrows the view to a single registered home on request.
#[tauri::command(rename_all = "snake_case")]
pub fn projects_all(
    state: State<AppState>,
    home: Option<String>,
) -> Result<Vec<HomeRecordsView<Project>>, UiError> {
    Ok(state.homes().projects_all(home.as_deref())?)
}

/// `home.tasks_all` — the merged tasks plane, same discipline as
/// `projects_all`; same-named tasks from different homes stay distinct
/// rows in distinct blocks (REQ-HOME-04 AC3).
#[tauri::command(rename_all = "snake_case")]
pub fn tasks_all(
    state: State<AppState>,
    home: Option<String>,
) -> Result<Vec<HomeRecordsView<Task>>, UiError> {
    Ok(state.homes().tasks_all(home.as_deref())?)
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

/// `task.signature` — the one read the APPROVAL signature section renders
/// (REQ-SIGN-07): spec, requirements, document questions, reviewer block,
/// previous-version diff, the two rights and the newest signature fact.
#[tauri::command(rename_all = "snake_case")]
pub fn task_signature(state: State<AppState>, id_prefix: String) -> Result<SignatureView, UiError> {
    Ok(state.api().task_signature(&id_prefix)?)
}

/// `task.approve` — the SAME platform act as `devai task approve`
/// (REQ-SIGN-05): the API writes the decision row; the window only calls it
/// and never writes the store itself. Author is the session actor the API
/// handle was opened with (`session.rs`).
#[tauri::command(rename_all = "snake_case")]
pub fn task_approve(
    state: State<AppState>,
    id_prefix: String,
    rationale: Option<String>,
) -> Result<SignatureView, UiError> {
    let api = state.api();
    api.task_approve(&id_prefix, rationale)?;
    // The card is re-read from the store through the SAME read the section
    // renders — no local state is patched up (REQ-SIGN-04 AC6).
    Ok(api.task_signature(&id_prefix)?)
}

/// `task.reject` — same act as `devai task reject`: one event and one
/// `requirements_rejection` decision row with the operator's rationale.
#[tauri::command(rename_all = "snake_case")]
pub fn task_reject(
    state: State<AppState>,
    id_prefix: String,
    rationale: Option<String>,
) -> Result<SignatureView, UiError> {
    let api = state.api();
    api.task_reject(&id_prefix, rationale)?;
    Ok(api.task_signature(&id_prefix)?)
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
    } = facts;
    let open_questions: Vec<Question> = questions
        .into_iter()
        .filter(|q| q.status == QuestionStatus::Open)
        .collect();
    let Some(demand) = wait_demand_for(policy, &task.state, &short_id(&task.id), &open_questions)?
    else {
        return Ok(None);
    };
    // QA-строка без CURRENT-отчёта подписывать нечего: платформа ещё не судила
    // (наблюдение оператора 07.11: «ждёт подписи», а отчёта нет). Вместо
    // требования подписи показываем факт и честное действие — прогнать аудит.
    let (demand_text, cli_hint) = if task.state == TaskState::Qa && report.is_none() {
        (
            "QA ещё не прогонялся: подписывать нечего, гейт ждёт аудита".to_string(),
            format!("devai task run {}", short_id(&task.id)),
        )
    } else {
        (demand.demand, demand.cli_hint)
    };
    Ok(Some(WaitRow {
        spec: spec.as_ref().map(|spec| spec_of(&task, spec)),
        task,
        demand: demand_text,
        cli_hint,
        open_questions,
        report,
        report_note,
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
        // Rework R8: the read refusal is a TYPED value, not a formatted
        // string — the API words the cause, the window only forwards it.
        SpecBodyView::Binary { size } => spec_presentation(
            &facts.artifact,
            Err(&BodyError::NotText { size: *size }),
            task,
            facts.current,
        ),
        SpecBodyView::Error { message } => spec_presentation(
            &facts.artifact,
            Err(&BodyError::Unreadable {
                message: message.clone(),
            }),
            task,
            facts.current,
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use devai_api::ParagraphSource;
    use devai_core::types::ArtifactKind;

    /// REQ-SIGN-05 AC4 (was REQ-UI-02 AC2): every data command's source is a
    /// published registry entry. The data plane stays Read-only; the ONLY
    /// Mutation-class commands reachable from the window are the two
    /// signature acts in `SIGNATURE_MUTATION_COMMANDS` — (a) every pair
    /// resolves to a published operation, (b) Read outside the allowlist and
    /// Mutation inside it, (c) the set of Mutation commands equals the
    /// allowlist exactly.
    #[test]
    fn every_command_operation_is_a_registered_operation() {
        let ops = devai_api::operations();
        let by_name: std::collections::HashMap<&str, &OpDescriptor> =
            ops.iter().map(|o| (o.name, o)).collect();
        assert!(!COMMAND_OPERATIONS.is_empty());
        let in_allowlist = |cmd: &str| SIGNATURE_MUTATION_COMMANDS.iter().any(|(c, _)| *c == cmd);
        for (cmd, op) in COMMAND_OPERATIONS {
            let d = by_name
                .get(op)
                .unwrap_or_else(|| panic!("`{cmd}` calls `{op}`: not a published operation"));
            if in_allowlist(cmd) {
                assert_eq!(
                    d.class,
                    devai_api::OpClass::Mutation,
                    "`{cmd}` is a listed signature mutation but `{op}` is not a Mutation"
                );
            } else {
                assert_eq!(
                    d.class,
                    devai_api::OpClass::Read,
                    "`{cmd}` calls `{op}` — a non-Read operation is forbidden in the data plane (OQ-UI-6)"
                );
            }
        }
        // The allowlist is exactly the mutation commands of the table — no
        // third pair, no Read row smuggled into it, no mutation outside it.
        let mutation_cmds: Vec<&str> = COMMAND_OPERATIONS
            .iter()
            .filter(|(_, op)| {
                by_name
                    .get(op)
                    .is_some_and(|d| d.class == devai_api::OpClass::Mutation)
            })
            .map(|(cmd, _)| *cmd)
            .collect();
        let allow: Vec<&str> = SIGNATURE_MUTATION_COMMANDS
            .iter()
            .map(|(c, _)| *c)
            .collect();
        assert_eq!(
            mutation_cmds, allow,
            "the signature allowlist is exactly two commands"
        );
        for (cmd, op) in SIGNATURE_MUTATION_COMMANDS {
            assert!(
                by_name
                    .get(op)
                    .is_some_and(|d| d.class == devai_api::OpClass::Mutation),
                "`{cmd}` must name a Mutation operation, got `{op}`"
            );
        }
    }

    /// R3 quality return: the post-registry rebind states why the active home
    /// could not be read with the SAME verdict the startup path uses
    /// (`session::gone_failure`) — `home_no_store` when the folder is there
    /// without a store, `home_unavailable` when the path vanished — instead
    /// of collapsing both into "not on disk" with one code.
    #[test]
    fn the_rebind_never_states_a_missing_store_as_a_vanished_path() {
        let dir = tempfile::TempDir::new().unwrap();
        let control = dir.path().join("control");
        std::fs::create_dir_all(&control).unwrap();
        let recorded = dir.path().join("project/.devai");
        let path = recorded.display().to_string();

        std::fs::create_dir_all(&recorded).unwrap();
        let (_, failure) = reopen_active(&control, Some(&path), "tester");
        let failure = failure.expect("a storeless home degrades, it is not opened");
        assert_eq!(failure.code, devai_api::codes::HOME_NO_STORE);
        assert_eq!(failure.home, path, "the failure names the home it is about");
        assert!(
            failure.message.contains("devai init"),
            "the operator gets the way out: {}",
            failure.message
        );

        std::fs::remove_dir_all(&recorded).unwrap();
        let (_, failure) = reopen_active(&control, Some(&path), "tester");
        let failure = failure.expect("a vanished home degrades, it is not created");
        assert_eq!(failure.code, devai_api::codes::HOME_UNAVAILABLE);
        assert!(failure.message.contains("not on disk"));
        assert!(
            !recorded.exists(),
            "the rebind created a home over a vanished record"
        );

        let none = no_active_failure(&control);
        assert_eq!(none.code, devai_api::codes::HOME_NONE_ACTIVE);
    }

    /// REQ-HOME-05/06: the registry-management commands map onto the
    /// published Registry-class home plane ONLY. No domain Mutation hides
    /// in the management table, and no data command reaches it — reading
    /// home records and managing the registry never mix in one operation.
    #[test]
    fn registry_management_commands_are_the_home_plane_only() {
        let ops = devai_api::operations();
        let by_name: std::collections::HashMap<&str, &OpDescriptor> =
            ops.iter().map(|o| (o.name, o)).collect();
        assert!(!COMMAND_REGISTRY_OPERATIONS.is_empty());
        for (cmd, op) in COMMAND_REGISTRY_OPERATIONS {
            let d = by_name
                .get(op)
                .unwrap_or_else(|| panic!("`{cmd}` calls `{op}`: not a published operation"));
            assert_eq!(
                d.class,
                devai_api::OpClass::Registry,
                "`{cmd}` calls `{op}` — management commands may only use the Registry class"
            );
            assert!(
                op.starts_with("home."),
                "`{cmd}`: the Registry class exists only in the homes plane"
            );
        }
        for (cmd, op) in COMMAND_OPERATIONS {
            let d = *by_name
                .get(op)
                .unwrap_or_else(|| panic!("`{cmd}` calls `{op}`: not published"));
            assert_ne!(
                d.class,
                devai_api::OpClass::Registry,
                "`{cmd}` mixes registry management into a data read (`{op}`)"
            );
        }
    }

    /// mt/change-isolation + arch/registry-in-data (report v1): the OTHER
    /// direction is guarded too. Every published `home.*` operation must
    /// have a UI command mapping — if the registry plane grows a row and
    /// the surface does not follow, this fails at `cargo test`, so the
    /// five-place checklist of a new operation can never half-land
    /// (the frontend→table direction is guarded by
    /// `frontend_invokes_are_exactly_mapped_reads`).
    #[test]
    fn every_published_home_operation_is_wired_into_the_ui() {
        let ops = devai_api::operations();
        let home_ops: Vec<&str> = ops
            .iter()
            .filter(|o| o.name.starts_with("home."))
            .map(|o| o.name)
            .collect();
        assert!(
            !home_ops.is_empty(),
            "the homes plane vanished from the registry"
        );
        for name in home_ops {
            let mapped = COMMAND_OPERATIONS
                .iter()
                .chain(COMMAND_REGISTRY_OPERATIONS.iter())
                .any(|(_, op)| *op == name);
            assert!(
                mapped,
                "published `{name}` has no UI command mapping — registry grew without the surface following"
            );
        }
    }

    /// ux/contract-stability (report v1): the `class` token dictionary is
    /// pinned ACROSS the wire. Every `OpClass` variant serializes to its
    /// lowercase token and every token must appear in the `types.ts`
    /// union — a class added on the Rust side without extending the TS
    /// type fails here, not silently at runtime (the homes cycle added
    /// `registry` exactly this way).
    #[test]
    fn op_class_tokens_are_pinned_across_the_wire() {
        let crate_dir = env!("CARGO_MANIFEST_DIR");
        let types_ts =
            std::fs::read_to_string(format!("{crate_dir}/frontend/src/types.ts")).unwrap();
        for class in [
            devai_api::OpClass::Read,
            devai_api::OpClass::Mutation,
            devai_api::OpClass::Registry,
        ] {
            let token = serde_json::to_value(class).unwrap();
            let token = token.as_str().expect("class serializes to a string token");
            assert!(
                types_ts.contains(&format!("\"{token}\"")),
                "class token `{token}` is not in the types.ts union — the CLI/API/TS wire dictionary drifted"
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

        // Homes plane (REQ-HOME-01/05): the registry record and a merged
        // view block are part of the frontend contract — same pin.
        let ts = chrono::DateTime::parse_from_rfc3339("2026-11-06T12:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc);
        let rec = devai_api::HomeRecord {
            path: "/home/op/project/.devai".into(),
            project: "tripsales".into(),
            project_source: devai_api::ProjectSource::Store,
            project_error: None,
            added_at: ts,
            enabled: true,
            active: false,
            available: true,
            schema_version: Some(8),
            version_note: None,
            read_error: None,
        };
        assert_eq!(serde_json::to_value(&rec).unwrap(), fixture["home_record"]);

        let block: devai_api::HomeRecordsView<devai_core::types::Project> =
            devai_api::HomeRecordsView {
                snapshot_at: ts,
                home_path: "/home/op/project/.devai".into(),
                home_project: "tripsales".into(),
                is_active: false,
                available: false,
                version_note: None,
                error: Some(devai_api::HomeIssue::new(
                    devai_api::codes::HOME_NO_STORE,
                    std::path::Path::new("/home/op/project/.devai"),
                    "no store",
                )),
                records: Vec::new(),
            };
        assert_eq!(
            serde_json::to_value(&block).unwrap(),
            fixture["home_projects_view"]
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
    fn frontend_invokes_are_exactly_mapped_operations() {
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
            let mapped = COMMAND_OPERATIONS.iter().any(|(c, _)| *c == name)
                || COMMAND_REGISTRY_OPERATIONS.iter().any(|(c, _)| *c == name);
            assert!(
                mapped,
                "api.ts invokes `{name}` with no COMMAND_OPERATIONS/COMMAND_REGISTRY_OPERATIONS mapping"
            );
            assert!(
                main_rs.contains(&format!("commands::{name},")),
                "`{name}` is not registered in the invoke_handler of main.rs"
            );
        }
        // REQ-SIGN-05 AC4: the frontend's Mutation commands are EXACTLY the
        // two signature acts, and task_signature is a Read.
        let ops = devai_api::operations();
        let class_of = |name: &str| {
            COMMAND_OPERATIONS
                .iter()
                .find(|(c, _)| *c == name)
                .and_then(|(_, op)| ops.iter().find(|o| o.name == *op))
                .map(|d| d.class)
        };
        assert_eq!(
            class_of("task_signature"),
            Some(devai_api::OpClass::Read),
            "task_signature must back the signature card as a Read"
        );
        let mut mutations: Vec<&str> = COMMAND_OPERATIONS
            .iter()
            .filter(|(cmd, _)| matches!(class_of(cmd), Some(devai_api::OpClass::Mutation)))
            .map(|(cmd, _)| *cmd)
            .collect();
        mutations.sort_unstable();
        assert_eq!(
            mutations,
            vec!["task_approve", "task_reject"],
            "the window's only mutations are the two signature acts"
        );
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

    /// REQ-SIGN-12 AC1/AC4: the spec parse has ONE home — `devai-api`. The
    /// window neither declares the types nor parses the body; a second
    /// definition in devai-ui fails here (grep test, not a convention).
    #[test]
    fn the_spec_parse_has_one_home_in_devai_api() {
        let crate_dir = env!("CARGO_MANIFEST_DIR");
        let ui = std::fs::read_to_string(format!("{crate_dir}/src/commands.rs")).unwrap();
        // Production half only: this very test names the forbidden symbols.
        let production = ui.split("#[cfg(test)]").next().unwrap();
        for forbidden in [
            "struct SpecPresentation",
            "enum ParagraphSource",
            "fn spec_presentation",
            "fn document_presentation",
            "fn fallback_presentation",
            "fn spec_paragraph",
            "fn spec_open_questions",
        ] {
            assert!(
                !production.contains(forbidden),
                "devai-ui declares `{forbidden}` — the parse lives in devai-api (REQ-SIGN-12)"
            );
        }
        // The one module that DOES own the rule set (rework R5: the module is
        // split by responsibility, so the grep spans every file of it) — and
        // each definition exists EXACTLY once, which is the uniqueness proof
        // REQ-SIGN-12 AC4 asks for.
        let module_dir = format!("{crate_dir}/../devai-api/src/signature");
        let mut api = String::new();
        for entry in std::fs::read_dir(&module_dir).unwrap() {
            let path = entry.unwrap().path();
            if path.extension().and_then(|e| e.to_str()) == Some("rs") {
                api.push_str(&std::fs::read_to_string(&path).unwrap());
                api.push('\n');
            }
        }
        api.push_str(
            &std::fs::read_to_string(format!("{crate_dir}/../devai-api/src/signature.rs")).unwrap(),
        );
        // The CURRENT-spec selector lives in `inbox.rs` (the read plane calls
        // it, never a private copy) — include it in the count below.
        api.push_str(
            &std::fs::read_to_string(format!("{crate_dir}/../devai-api/src/inbox.rs")).unwrap(),
        );
        for definition in [
            "pub fn parse_spec_body",
            "pub fn spec_presentation",
            "pub struct SpecPresentation",
        ] {
            assert_eq!(
                api.matches(definition).count(),
                1,
                "`{definition}` must be defined exactly once in devai-api"
            );
        }
        // One CURRENT-spec selector (rework R6) and no surviving copy of the
        // parser name the move deleted.
        assert_eq!(
            api.matches("fn pick_spec_artifact").count(),
            1,
            "the CURRENT-spec selector must exist exactly once"
        );
        assert_eq!(
            api.matches("fn spec_open_questions").count(),
            0,
            "the old per-panel question parser is gone, not copied"
        );
        // No JSON parsing of the spec body in the window.
        assert!(
            !production.contains("serde_json::from_str"),
            "devai-ui parses the spec JSON itself — the frontend never parses (REQ-SIGN-12 AC2)"
        );
    }

    /// REQ-SIGN-02 AC4 / ASM-16: the section's words live in its OWN module;
    /// the shared StateChip and the TaskState mirror stay untouched and are
    /// not the section's source of words.
    #[test]
    fn signature_labels_are_the_sections_own_and_the_chip_is_untouched() {
        let crate_dir = env!("CARGO_MANIFEST_DIR");
        let section = std::fs::read_to_string(format!(
            "{crate_dir}/frontend/src/components/SignatureSection.vue"
        ))
        .unwrap();
        assert!(
            section.contains("signature_labels"),
            "the section must take its words from signature_labels.ts"
        );
        let chip =
            std::fs::read_to_string(format!("{crate_dir}/frontend/src/components/StateChip.vue"))
                .unwrap();
        assert!(
            chip.contains("{{ token }}"),
            "StateChip prints the stored token as-is (StateChip.vue:20)"
        );
        assert!(
            !chip.contains("signature_labels"),
            "StateChip must not become the section's dictionary (ASM-16)"
        );
        let states =
            std::fs::read_to_string(format!("{crate_dir}/frontend/src/states.ts")).unwrap();
        assert!(
            !states.contains("signature_labels"),
            "states.ts is a TaskState mirror, not the section's word dictionary"
        );
        // Rework R0: the words are ONE constant of the section's own module
        // (`signature_labels.ts`), not a data table. This test ties that
        // constant's token sets to the CLOSED Rust types: growing an enum
        // fails here until the word lands in the module.
        let labels =
            std::fs::read_to_string(format!("{crate_dir}/frontend/src/signature_labels.ts"))
                .unwrap();
        let tokens_of = |constant: &str| -> std::collections::BTreeSet<String> {
            let head = format!("export const {constant}: Record<string, string> = {{");
            let start = labels
                .find(&head)
                .unwrap_or_else(|| panic!("no dictionary `{constant}` in signature_labels.ts"));
            let body = &labels[start + head.len()..];
            let end = body.find("};").expect("the dictionary block is not closed");
            body[..end]
                .lines()
                .filter_map(|line| {
                    let (key, rest) = line.trim().split_once(':')?;
                    let word = rest.trim().trim_end_matches(',').trim().trim_matches('"');
                    assert!(!word.is_empty(), "{constant}: нет слова для `{key}`");
                    Some(key.trim().to_string())
                })
                .collect()
        };
        let serde_token = |value: &str| value.to_string();

        let statuses: std::collections::BTreeSet<String> = devai_core::types::ArtifactStatus::ALL
            .iter()
            .map(|s| s.as_str().to_string())
            .collect();
        assert_eq!(tokens_of("ARTIFACT_STATUS_LABELS"), statuses);

        let wire = |v: &serde_json::Value| {
            v.as_str()
                .expect("closed sets are string tokens")
                .to_string()
        };
        let spec_sources: std::collections::BTreeSet<String> = ParagraphSource::ALL
            .iter()
            .map(|v| serde_token(&wire(&serde_json::to_value(*v).unwrap())))
            .collect();
        assert_eq!(tokens_of("PARAGRAPH_SOURCE_LABELS"), spec_sources);

        let review_sources: std::collections::BTreeSet<String> =
            devai_api::ReviewParagraphSource::ALL
                .iter()
                .map(|v| serde_token(&wire(&serde_json::to_value(*v).unwrap())))
                .collect();
        assert_eq!(tokens_of("REVIEW_PARAGRAPH_SOURCE_LABELS"), review_sources);

        let verdicts: std::collections::BTreeSet<String> = devai_core::types::Verdict::ALL
            .iter()
            .map(|v| v.as_str().to_ascii_lowercase())
            .filter(|v| v != "returned")
            .collect();
        assert_eq!(tokens_of("REVIEW_VERDICT_LABELS"), verdicts);

        let denials: std::collections::BTreeSet<String> = devai_api::DENIAL_CODES
            .iter()
            .map(|c| c.to_string())
            .collect();
        assert_eq!(tokens_of("DENIAL_LABELS"), denials);

        // Rework R3: the signature act and its source are CLOSED sets too —
        // their words are pinned to the Rust enums, not to the module's own
        // self-check, so a third act cannot print as a raw token.
        let kinds: std::collections::BTreeSet<String> = devai_api::SignatureKind::ALL
            .iter()
            .map(|v| wire(&serde_json::to_value(*v).unwrap()))
            .collect();
        assert_eq!(tokens_of("SIGNATURE_KIND_LABELS"), kinds);

        let sources: std::collections::BTreeSet<String> = devai_api::SignatureSource::ALL
            .iter()
            .map(|v| wire(&serde_json::to_value(*v).unwrap()))
            .collect();
        assert_eq!(tokens_of("SIGNATURE_SOURCE_LABELS"), sources);
    }

    /// REQ-SIGN-07: the signature answer's serialized fields exist in the
    /// mirrored `types.ts` interface (snake_case mirror — serde carries no
    /// rename_all), so a Rust-side rename fails here.
    #[test]
    fn signature_view_shape_parity_with_types_ts() {
        let now = now_fixture();
        let view = SignatureView {
            task_id: "01a1b790-0000-4000-8000-000000000001".into(),
            state: TaskState::Approval,
            snapshot_at: now,
            may_approve: false,
            may_approve_denied: Some("no_spec".into()),
            may_reject: true,
            may_reject_denied: None,
            spec: None,
            review: None,
            review_note: Some("отчёта ревьювера нет".into()),
            previous: None,
            previous_note: Some("прошлой версии нет".into()),
            signature: None,
        };
        let value = serde_json::to_value(&view).unwrap();
        let ts = std::fs::read_to_string(format!(
            "{}/frontend/src/types.ts",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        let block = ts_interface_block(&ts, "SignatureView");
        for field in value.as_object().unwrap().keys() {
            assert!(
                block.contains(&format!("{field}:")),
                "types.ts `SignatureView` has no field `{field}` the Rust view serializes"
            );
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
            // v1 = the first recorded shape (no break yet, empty note);
            // a BREAKING revision starts at v2 and must say what changed.
            if contract.version >= 2 {
                assert!(
                    !contract.breaking.trim().is_empty(),
                    "{}: a breaking revision must say what changed",
                    contract.command
                );
            } else {
                assert_eq!(
                    contract.version, 1,
                    "{}: versions start at 1",
                    contract.command
                );
                assert!(
                    contract.breaking.trim().is_empty(),
                    "{}: a first shape carries no breaking note",
                    contract.command
                );
            }
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
    // ------------------------------------------------------------------
    // R2 quality return (mt/change-isolation): the ONE declarative record
    // schema and the parity test over every site that spells it.
    // ------------------------------------------------------------------

    /// The key set of a serialized record must be exactly the declared one.
    fn assert_schema_keys(schema: &[(&str, &str)], v: &serde_json::Value, what: &str) {
        let mut declared: Vec<&str> = devai_api::schema_names(schema);
        declared.sort_unstable();
        let obj = v
            .as_object()
            .unwrap_or_else(|| panic!("{what}: not an object"));
        let mut actual: Vec<&str> = obj.keys().map(String::as_str).collect();
        actual.sort_unstable();
        assert_eq!(
            declared, actual,
            "{what}: serde output and the declarative schema drifted — a field landed in one site only"
        );
    }

    /// Fields of a TS interface in declaration order: `(name, type)`.
    /// Comment lines are skipped; every other line must be `name: type;`.
    fn ts_interface(src: &str, iface: &str) -> Vec<(String, String)> {
        let head = format!("export interface {iface}");
        let start = src
            .find(&head)
            .unwrap_or_else(|| panic!("types.ts has no `{head}` interface"));
        let body = &src[start + head.len()..];
        let body = &body[..body.find("\n}").expect("interface closes")];
        let mut out = Vec::new();
        for line in body.lines() {
            let line = line.trim();
            if line.is_empty() || line.starts_with("//") || line == "{" {
                continue;
            }
            let (name, ty) = line
                .split_once(':')
                .unwrap_or_else(|| panic!("{iface}: not a field line: {line}"));
            out.push((
                name.trim().to_string(),
                ty.trim().trim_end_matches(';').to_string(),
            ));
        }
        out
    }

    fn assert_schema_matches_ts(schema: &[(&str, &str)], src: &str, iface: &str) {
        let declared: Vec<(String, String)> = schema
            .iter()
            .map(|(n, t)| (n.to_string(), t.to_string()))
            .collect();
        assert_eq!(
            declared,
            ts_interface(src, iface),
            "types.ts `{iface}` and the declarative schema drifted"
        );
    }

    /// Every declared field must be referenced by the view layer that
    /// renders it (`b.<field>` / `h.<field>`), so a schema field cannot
    /// exist only on the wire and silently never reach the window.
    fn assert_fields_rendered(schema: &[(&str, &str)], sources: &[&str], var: &str) {
        for (name, _) in schema {
            let needle = format!("{var}.{name}");
            assert!(
                sources.iter().any(|s| s.contains(&needle)),
                "`{needle}` is declared in the record schema but no view reads it — the attribute never reaches the operator"
            );
        }
    }

    /// R5 quality return: `SessionInfo` is a wire shape too. It is described
    /// once (`session::SESSION_INFO_SCHEMA`), pinned in the IPC fixture and
    /// mirrored by `types.ts` — the same discipline the home records have, so
    /// `session_info` and `homes_activate` cannot drift from the frontend.
    #[test]
    fn session_info_schema_is_pinned_on_the_wire_fixture_and_types() {
        use crate::session::SESSION_INFO_SCHEMA;
        let info = SessionInfo {
            home: "/h/.devai".into(),
            actor: "op".into(),
            opened_at: Utc::now(),
            control_home: "/c/.devai".into(),
            degraded_from: None,
        };
        let json = serde_json::to_value(&info).unwrap();
        assert_schema_keys(SESSION_INFO_SCHEMA, &json, "SessionInfo");

        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ipc_shapes.json")).unwrap();
        let mut declared = devai_api::schema_names(SESSION_INFO_SCHEMA);
        declared.sort_unstable();
        let mut pinned: Vec<&str> = fixture["session_info"]
            .as_object()
            .expect("ipc_shapes.json pins the session_info shape")
            .keys()
            .map(String::as_str)
            .collect();
        pinned.sort_unstable();
        assert_eq!(
            declared, pinned,
            "the wire fixture must carry every session_info field (same names)"
        );

        let types_ts = std::fs::read_to_string(format!(
            "{}/frontend/src/types.ts",
            env!("CARGO_MANIFEST_DIR")
        ))
        .unwrap();
        assert_schema_matches_ts(SESSION_INFO_SCHEMA, &types_ts, "SessionInfo");
    }

    /// R2 quality return: the homes attributes are described ONCE
    /// (`devai_api::HOME_*_SCHEMA`) and checked against all five sites that
    /// spell them — the persisted `HomeEntry`, the wire `HomeRecord` (whose
    /// serde output IS the `ipc_shapes.json` fixture pinned by the test
    /// above), the published registry rows, the `types.ts` interfaces and
    /// the views. Adding an attribute to one site without the schema fails
    /// here; adding it to the schema without the TS type or the view usage
    /// fails right after.
    #[test]
    fn homes_record_schema_is_one_description_checked_on_every_site() {
        use devai_api::{
            HOME_ENTRY_SCHEMA, HOME_ISSUE_SCHEMA, HOME_RECORD_SCHEMA, HOME_VIEW_SCHEMA, HomeEntry,
            HomeIssue, HomeRecord, ProjectSource,
        };
        let crate_dir = env!("CARGO_MANIFEST_DIR");
        let read = |rel: &str| std::fs::read_to_string(format!("{crate_dir}/{rel}")).unwrap();
        let types_ts = read("frontend/src/types.ts");

        let ts = chrono::DateTime::parse_from_rfc3339("2026-11-06T12:00:00+00:00")
            .unwrap()
            .with_timezone(&Utc);

        // Site 1: the persisted descriptor (homes.json of the control home).
        let entry = HomeEntry {
            path: "/h/.devai".into(),
            project: "h".into(),
            project_source: ProjectSource::Store,
            project_error: None,
            added_at: ts,
            enabled: true,
            active: true,
        };
        assert_schema_keys(
            HOME_ENTRY_SCHEMA,
            &serde_json::to_value(&entry).unwrap(),
            "HomeEntry",
        );

        // Site 2: the wire record (its serde JSON is the frozen fixture).
        let record = HomeRecord {
            path: "/h/.devai".into(),
            project: "h".into(),
            project_source: ProjectSource::Unreadable,
            project_error: Some("no store".into()),
            added_at: ts,
            enabled: true,
            active: false,
            available: false,
            schema_version: None,
            version_note: None,
            read_error: Some(HomeIssue::new(
                devai_api::codes::HOME_NO_STORE,
                std::path::Path::new("/h/.devai"),
                "no store",
            )),
        };
        let record_json = serde_json::to_value(&record).unwrap();
        assert_schema_keys(HOME_RECORD_SCHEMA, &record_json, "HomeRecord");
        let fixture: serde_json::Value =
            serde_json::from_str(include_str!("../tests/fixtures/ipc_shapes.json")).unwrap();
        assert_eq!(
            record_json.as_object().unwrap().keys().collect::<Vec<_>>(),
            fixture["home_record"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            "the wire fixture must carry every schema field (same names)"
        );

        let issue_json = serde_json::to_value(&record.read_error.clone().unwrap()).unwrap();
        assert_schema_keys(HOME_ISSUE_SCHEMA, &issue_json, "HomeIssue");

        let block = devai_api::HomeRecordsView {
            snapshot_at: ts,
            home_path: "/h/.devai".into(),
            home_project: "h".into(),
            is_active: true,
            available: true,
            version_note: None,
            error: None,
            records: Vec::<devai_core::types::Project>::new(),
        };
        assert_schema_keys(
            HOME_VIEW_SCHEMA,
            &serde_json::to_value(&block).unwrap(),
            "HomeRecordsView",
        );

        // Site 3: the published registry rows declare these record types.
        let ops = devai_api::operations();
        let by_name: std::collections::HashMap<&str, &devai_api::OpDescriptor> =
            ops.iter().map(|o| (o.name, o)).collect();
        assert_eq!(by_name["home.list"].output, "Vec<HomeRecord>");
        assert_eq!(
            by_name["home.projects_all"].output,
            "Vec<HomeRecordsView<Project>>"
        );
        assert_eq!(
            by_name["home.tasks_all"].output,
            "Vec<HomeRecordsView<Task>>"
        );
        for (name, _) in HOME_RECORD_SCHEMA {
            assert!(
                by_name["home.list"].description.contains(name),
                "home.list must describe its `{name}` attribute"
            );
        }

        // Site 4: the TS interfaces are the frontend's view of the same
        // records (field name + type, same order).
        assert_schema_matches_ts(HOME_RECORD_SCHEMA, &types_ts, "HomeRecord");
        assert_schema_matches_ts(HOME_VIEW_SCHEMA, &types_ts, "HomeRecordsView<T>");
        assert_schema_matches_ts(HOME_ISSUE_SCHEMA, &types_ts, "HomeIssue");

        // Site 5: the views read every declared field.
        let homes_view = read("frontend/src/views/HomesView.vue");
        let homes_vm = read("frontend/src/homes-view.ts");
        assert_fields_rendered(HOME_RECORD_SCHEMA, &[&homes_view, &homes_vm], "h");
        let projects_view = read("frontend/src/views/ProjectsView.vue");
        let tasks_view = read("frontend/src/views/TasksView.vue");
        assert_fields_rendered(
            HOME_VIEW_SCHEMA,
            &[&projects_view, &tasks_view, &homes_vm],
            "b",
        );
    }
}
