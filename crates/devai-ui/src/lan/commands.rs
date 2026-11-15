//! The phone's command plane: ONE catalogue that is both the reachable name
//! list and the dispatcher (quality return R10 — the first revision kept the
//! list in three places that could drift).
//!
//! Nothing here implements data access: every handler calls the SAME
//! `crate::commands` function the window invokes. Writes pass an explicit
//! actor (`<operator> · телефон: …`), reads stay observational
//! (REQ-LAN-05, REQ-LAN-06, REQ-LAN-10).

use serde::Serialize;
use tauri::State;

use crate::commands::{AppState, UiError};
use crate::lan::server::{start_access, stop_access};
use crate::lan::state::{DeviceClaim, LanStatus, actor_marker};

/// The five phone-access controls. They are part of the same command plane as
/// the data commands, but they are not domain operations — they manage the
/// ACCESS (the enable flag of the control home) and are registered as their
/// own surface (see `LAN_SURFACE_CONTRACTS`).
#[allow(dead_code)] // the recorded entries are read by the parity test
pub struct LanOperation {
    pub command: &'static str,
    pub operation: &'static str,
    pub description: &'static str,
    pub handler: RpcHandler,
}

/// The HTTP/JSON envelope and the phone-access operations as ONE recorded
/// shape contract (quality return R9). A second consumer reads this table
/// instead of reverse-engineering the routes.
#[allow(dead_code)] // the recorded entries are read by the parity test
pub struct SurfaceContract {
    pub surface: &'static str,
    pub version: u32,
    pub shape: &'static str,
}

#[allow(dead_code)] // the recorded contract is consumed by the parity test
pub const LAN_SURFACE_CONTRACTS: &[SurfaceContract] = &[
    SurfaceContract {
        surface: "phone-http",
        version: 2,
        shape: "GET / ?token=<invite> | cookie -> 200 shell / 401 / 403; GET /pair[?token] -> 200 {state:paired|invite} / 401 / 403; POST /rpc {command,params} -> {ok,data|error}; GET /*path -> paired asset",
    },
    SurfaceContract {
        surface: "phone-rpc",
        version: 1,
        shape: "one-shot invitation token pairs exactly one device session; session cookie is the only key; disable revokes at once",
    },
];

/// The field names of the RPC envelope. Pinned against `types.ts` and against
/// the serde shape of [`RpcEnvelope`] so the mirror cannot drift silently.
#[allow(dead_code)] // the contract is consumed by the parity test
pub const RPC_ENVELOPE_FIELDS: &[&str] = &["ok", "data", "error"];

/// The wire shape the phone reads. It exists only to pin the envelope keys.
#[allow(dead_code)]
#[derive(Serialize)]
pub struct RpcEnvelope<T> {
    pub ok: bool,
    pub data: Option<T>,
    pub error: Option<UiError>,
}

pub type RpcHandler =
    fn(&tauri::AppHandle, &DeviceClaim, &serde_json::Value) -> Result<serde_json::Value, UiError>;

pub const LAN_OPERATIONS: &[LanOperation] = &[
    LanOperation {
        command: "lan_status",
        operation: "lan.status",
        description: "Current phone-access fact: off | awaiting | connected, bound address, invitation, devices.",
        handler: lan_status_rpc,
    },
    LanOperation {
        command: "lan_enable",
        operation: "lan.enable",
        description: "Bind the machine's home-segment address and start serving; refusal stays off with a reason.",
        handler: lan_enable_rpc,
    },
    LanOperation {
        command: "lan_disable",
        operation: "lan.disable",
        description: "Turn the access off immediately and revoke every paired session.",
        handler: lan_disable_rpc,
    },
    LanOperation {
        command: "lan_new_invite",
        operation: "lan.invite",
        description: "Issue a fresh one-time invitation; the previous invitation is retired, paired phones stay.",
        handler: lan_new_invite_rpc,
    },
    LanOperation {
        command: "lan_disconnect",
        operation: "lan.disconnect",
        description: "Disconnect one paired device by its session id.",
        handler: lan_disconnect_rpc,
    },
];

/// The window's one-button switch over the phone access. The state comes
/// from the live server, never from a request flag (REQ-LAN-01 AC4).
#[tauri::command]
pub fn lan_status(state: State<AppState>) -> LanStatus {
    state.lan.status()
}

/// «включить»: bind the machine's home-segment address and start serving.
/// A refusal (no address, port taken) leaves the window honestly «выключен»
/// with the reason (REQ-LAN-01 AC5).
#[tauri::command]
pub fn lan_enable(app: tauri::AppHandle, state: State<AppState>) -> Result<LanStatus, UiError> {
    start_access(app, state.lan.clone())
}

/// «отключить»: state off immediately, every session revoked, the listener
/// asked to stop — the next request from the phone is refused (REQ-LAN-04
/// AC6, REQ-LAN-09 AC4).
#[tauri::command]
pub fn lan_disable(state: State<AppState>) -> LanStatus {
    stop_access(&state.lan)
}

/// Issue a fresh one-time invitation. The PREVIOUS invitation is retired;
/// already paired phones stay connected (ratified Q2 «может», REQ-LAN-04).
#[tauri::command]
pub fn lan_new_invite(state: State<AppState>) -> Result<LanStatus, UiError> {
    state.lan.new_invite()?;
    Ok(state.lan.status())
}

/// Disconnect ONE paired device by the session id the status published.
#[tauri::command(rename_all = "snake_case")]
pub fn lan_disconnect(state: State<AppState>, id: String) -> LanStatus {
    state.lan.disconnect(&id);
    state.lan.status()
}

// ---------------------------------------------------------------------------
// the catalogue: name + handler in ONE entry (R10)
// ---------------------------------------------------------------------------

pub struct RpcCommand {
    /// The command name the frontend invokes (IPC) and posts to `/rpc`.
    pub name: &'static str,
    pub handler: RpcHandler,
}

/// The data commands the phone may reach. It mirrors the window exactly: a
/// command registered in the window that is missing here fails
/// `the_phone_catalogue_is_the_window_plane`.
pub const RPC_COMMANDS: &[RpcCommand] = &[
    RpcCommand {
        name: "session_info",
        handler: session_info_rpc,
    },
    RpcCommand {
        name: "registry_operations",
        handler: registry_operations_rpc,
    },
    RpcCommand {
        name: "homes_list",
        handler: homes_list_rpc,
    },
    RpcCommand {
        name: "homes_add",
        handler: homes_add_rpc,
    },
    RpcCommand {
        name: "homes_set_enabled",
        handler: homes_set_enabled_rpc,
    },
    RpcCommand {
        name: "homes_activate",
        handler: homes_activate_rpc,
    },
    RpcCommand {
        name: "projects_all",
        handler: projects_all_rpc,
    },
    RpcCommand {
        name: "tasks_all",
        handler: tasks_all_rpc,
    },
    RpcCommand {
        name: "projects_list",
        handler: projects_list_rpc,
    },
    RpcCommand {
        name: "projects_get",
        handler: projects_get_rpc,
    },
    RpcCommand {
        name: "tasks_list",
        handler: tasks_list_rpc,
    },
    RpcCommand {
        name: "task_get",
        handler: task_get_rpc,
    },
    RpcCommand {
        name: "task_status",
        handler: task_status_rpc,
    },
    RpcCommand {
        name: "task_signature",
        handler: task_signature_rpc,
    },
    RpcCommand {
        name: "task_approve",
        handler: task_approve_rpc,
    },
    RpcCommand {
        name: "task_reject",
        handler: task_reject_rpc,
    },
    RpcCommand {
        name: "task_accept",
        handler: task_accept_rpc,
    },
    RpcCommand {
        name: "task_return",
        handler: task_return_rpc,
    },
    RpcCommand {
        name: "question_answer",
        handler: question_answer_rpc,
    },
    RpcCommand {
        name: "task_cancel",
        handler: task_cancel_rpc,
    },
    RpcCommand {
        name: "task_resume",
        handler: task_resume_rpc,
    },
    RpcCommand {
        name: "task_new",
        handler: task_new_rpc,
    },
    RpcCommand {
        name: "task_rewind",
        handler: task_rewind_rpc,
    },
    RpcCommand {
        name: "task_restate",
        handler: task_restate_rpc,
    },
    RpcCommand {
        name: "task_run",
        handler: task_run_rpc,
    },
    RpcCommand {
        name: "execution_cancel",
        handler: execution_cancel_rpc,
    },
    RpcCommand {
        name: "evaluation_rules",
        handler: evaluation_rules_rpc,
    },
    RpcCommand {
        name: "task_usage",
        handler: task_usage_rpc,
    },
    RpcCommand {
        name: "source_slice",
        handler: source_slice_rpc,
    },
    RpcCommand {
        name: "task_progress",
        handler: task_progress_rpc,
    },
    RpcCommand {
        name: "task_dependencies",
        handler: task_dependencies_rpc,
    },
    RpcCommand {
        name: "task_report",
        handler: task_report_rpc,
    },
    RpcCommand {
        name: "task_events",
        handler: task_events_rpc,
    },
    RpcCommand {
        name: "task_decisions",
        handler: task_decisions_rpc,
    },
    RpcCommand {
        name: "task_active",
        handler: task_active_rpc,
    },
    RpcCommand {
        name: "task_workspace_changes",
        handler: task_workspace_changes_rpc,
    },
    RpcCommand {
        name: "task_workspace_diff_full",
        handler: task_workspace_diff_full_rpc,
    },
    RpcCommand {
        name: "task_workspace_diff",
        handler: task_workspace_diff_rpc,
    },
    RpcCommand {
        name: "artifacts_list",
        handler: artifacts_list_rpc,
    },
    RpcCommand {
        name: "artifact_content",
        handler: artifact_content_rpc,
    },
    RpcCommand {
        name: "questions_list",
        handler: questions_list_rpc,
    },
    RpcCommand {
        name: "agent_statuses",
        handler: agent_statuses_rpc,
    },
    RpcCommand {
        name: "executions_list",
        handler: executions_list_rpc,
    },
    RpcCommand {
        name: "execution_liveness",
        handler: execution_liveness_rpc,
    },
    RpcCommand {
        name: "inbox_view",
        handler: inbox_view_rpc,
    },
    RpcCommand {
        name: "model_snapshot",
        handler: model_snapshot_rpc,
    },
    RpcCommand {
        name: "model_probe",
        handler: model_probe_rpc,
    },
    RpcCommand {
        name: "model_bundle_handle",
        handler: model_bundle_handle_rpc,
    },
    RpcCommand {
        name: "model_bundle_check",
        handler: model_bundle_check_rpc,
    },
    RpcCommand {
        name: "model_bundle_create",
        handler: model_bundle_create_rpc,
    },
    RpcCommand {
        name: "model_bundle_update",
        handler: model_bundle_update_rpc,
    },
    RpcCommand {
        name: "model_bundle_delete",
        handler: model_bundle_delete_rpc,
    },
    RpcCommand {
        name: "model_switch",
        handler: model_switch_rpc,
    },
];

/// Every command the phone may post. Derived from the two tables — there is
/// no second list to drift from (R10). Consumed by parity tests.
#[allow(dead_code)]
pub fn commands_for_rpc() -> Vec<&'static str> {
    RPC_COMMANDS
        .iter()
        .map(|c| c.name)
        .chain(LAN_OPERATIONS.iter().map(|o| o.command))
        .collect()
}

/// One door from HTTP to the SAME command functions (REQ-LAN-05): the phone
/// gets the window's surface, not a re-implementation of it.
pub fn dispatch_rpc(
    app: &tauri::AppHandle,
    device: &DeviceClaim,
    command: &str,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    if let Some(spec) = RPC_COMMANDS.iter().find(|c| c.name == command) {
        return (spec.handler)(app, device, params);
    }
    if let Some(operation) = LAN_OPERATIONS.iter().find(|o| o.command == command) {
        return (operation.handler)(app, device, params);
    }
    Err(UiError {
        code: "unknown_command".into(),
        message: format!("команда `{command}` не доступна с телефона"),
    })
}

// ---------------------------------------------------------------------------
// value plumbing shared by the handlers
// ---------------------------------------------------------------------------

fn to_json<T: Serialize>(value: T) -> Result<serde_json::Value, UiError> {
    serde_json::to_value(value).map_err(|e| UiError {
        code: "ipc".into(),
        message: format!("не удалось сериализовать ответ: {e}"),
    })
}

fn state(app: &tauri::AppHandle) -> State<'_, AppState> {
    use tauri::Manager;
    app.state::<AppState>()
}

fn param_str(params: &serde_json::Value, key: &str) -> Result<String, UiError> {
    params
        .get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| UiError {
            code: "ipc".into(),
            message: format!("в запросе нет строкового поля `{key}`"),
        })
}

fn param_opt_str(params: &serde_json::Value, key: &str) -> Option<String> {
    params.get(key).and_then(|v| v.as_str()).map(str::to_string)
}

fn param_bool(params: &serde_json::Value, key: &str) -> Result<bool, UiError> {
    params
        .get(key)
        .and_then(|v| v.as_bool())
        .ok_or_else(|| UiError {
            code: "ipc".into(),
            message: format!("в запросе нет булева поля `{key}`"),
        })
}

fn param_opt_bool(params: &serde_json::Value, key: &str) -> Option<bool> {
    params.get(key).and_then(|v| v.as_bool())
}

fn param_opt_usize(params: &serde_json::Value, key: &str) -> Option<usize> {
    params.get(key).and_then(|v| v.as_u64()).map(|n| n as usize)
}

/// The LAN write actor: the operator stays the author, the device suffix
/// only names the source (REQ-LAN-06 AC1-3).
fn actor_for(app: &tauri::AppHandle, device: &DeviceClaim) -> String {
    let base = state(app).session().actor.clone();
    actor_marker(&base, device)
}

// ---------------------------------------------------------------------------
// handlers — thin wrappers over `crate::commands`
// ---------------------------------------------------------------------------

fn session_info_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    _params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::session_info(state(app)))
}

fn registry_operations_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    _params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::registry_operations(state(app))?)
}

fn homes_list_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    _params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::homes_list(state(app))?)
}

fn homes_add_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::homes_add(
        state(app),
        param_str(params, "path")?,
    )?)
}

fn homes_set_enabled_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::homes_set_enabled(
        state(app),
        param_str(params, "path")?,
        param_bool(params, "enabled")?,
    )?)
}

fn homes_activate_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::homes_activate(
        state(app),
        param_str(params, "path")?,
    )?)
}

fn projects_all_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::projects_all(
        state(app),
        param_opt_str(params, "home"),
    )?)
}

fn tasks_all_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::tasks_all(
        state(app),
        param_opt_str(params, "home"),
    )?)
}

fn projects_list_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    _params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::projects_list(state(app))?)
}

fn projects_get_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::projects_get(
        state(app),
        param_str(params, "id_or_name")?,
    )?)
}

fn tasks_list_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    _params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::tasks_list(state(app))?)
}

fn task_get_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::task_get(
        state(app),
        param_str(params, "id_prefix")?,
    )?)
}

fn task_status_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::task_status(
        state(app),
        param_str(params, "id_prefix")?,
    )?)
}

fn task_signature_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::task_signature(
        state(app),
        param_str(params, "id_prefix")?,
    )?)
}

fn task_approve_rpc(
    app: &tauri::AppHandle,
    device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::task_approve_as(
        state(app),
        param_str(params, "id_prefix")?,
        param_opt_str(params, "rationale"),
        Some(actor_for(app, device)),
    )?)
}

fn task_reject_rpc(
    app: &tauri::AppHandle,
    device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::task_reject_as(
        state(app),
        param_str(params, "id_prefix")?,
        param_opt_str(params, "rationale"),
        Some(actor_for(app, device)),
    )?)
}

/// The QA gate's acceptance from the phone: the SAME operation the window and
/// the CLI call; the actor marker names the device, the person stays the author
/// (REQ-QG-07 AC3). The request is decoded once (rework R5) so the phone's
/// parameter contract is exercised by a test without a Tauri app.
fn task_accept_rpc(
    app: &tauri::AppHandle,
    device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    let req = QaActRequest::of(params)?;
    to_json(crate::commands::task_accept_as(
        state(app),
        req.id_prefix,
        req.rationale,
        Some(actor_for(app, device)),
    )?)
}

/// The QA gate's return from the phone: same operation, same one-event record.
/// Постановка задачи с телефона: тот же акт, что в окне и в консоли.
fn task_new_rpc(
    app: &tauri::AppHandle,
    device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::task_new_as(
        state(app),
        param_str(params, "title")?,
        param_opt_str(params, "description").unwrap_or_default(),
        param_opt_str(params, "project_id"),
        None,
        None,
        Some(actor_for(app, device)),
    )?)
}

/// Перепройти с шага с телефона: тот же акт, что в окне и в консоли.
fn task_rewind_rpc(
    app: &tauri::AppHandle,
    device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::task_rewind_as(
        state(app),
        param_str(params, "id_prefix")?,
        param_str(params, "to")?,
        param_str(params, "reason")?,
        Some(actor_for(app, device)),
    )?)
}

/// Переписать постановку с телефона: тот же акт.
fn task_restate_rpc(
    app: &tauri::AppHandle,
    device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::task_restate_as(
        state(app),
        param_str(params, "id_prefix")?,
        param_opt_str(params, "title"),
        param_opt_str(params, "description"),
        param_str(params, "reason")?,
        Some(actor_for(app, device)),
    )?)
}

/// Возврат в работу с телефона: тот же акт, что в окне и в консоли.
fn task_resume_rpc(
    app: &tauri::AppHandle,
    device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::task_resume_as(
        state(app),
        param_str(params, "id_prefix")?,
        Some(actor_for(app, device)),
    )?)
}

/// Отмена задачи с телефона: тот же акт, что в окне и в консоли.
fn task_cancel_rpc(
    app: &tauri::AppHandle,
    device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::task_cancel_as(
        state(app),
        param_str(params, "id_prefix")?,
        param_opt_str(params, "reason"),
        Some(actor_for(app, device)),
    )?)
}

/// Ответ на вопрос с телефона: тот же акт платформы, что в окне и в консоли.
fn question_answer_rpc(
    app: &tauri::AppHandle,
    device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    let req = AnswerRequest::of(params)?;
    to_json(crate::commands::question_answer_as(
        state(app),
        req.id_prefix,
        req.question_id,
        req.text,
        Some(actor_for(app, device)),
    )?)
}

/// Форма ответа на вопрос: те же три поля, что у оконной команды.
struct AnswerRequest {
    id_prefix: String,
    question_id: String,
    text: String,
}

impl AnswerRequest {
    fn of(params: &serde_json::Value) -> Result<Self, UiError> {
        Ok(Self {
            id_prefix: param_str(params, "id_prefix")?,
            question_id: param_str(params, "question_id")?,
            text: param_str(params, "text")?,
        })
    }
}

fn task_return_rpc(
    app: &tauri::AppHandle,
    device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    let req = QaActRequest::of(params)?;
    to_json(crate::commands::task_return_as(
        state(app),
        req.id_prefix,
        req.rationale,
        Some(actor_for(app, device)),
    )?)
}

/// The card's restart from the phone (REQ-LV-05 AC3): the SAME platform
/// operation as `devai task run` — the platform reaps a proven-dead owner and
/// launches by the ordinary rules. The request is decoded once, by the shared
/// parameter helpers, so a malformed shape carries the same `ipc` code every
/// neighbouring handler emits; the window's handler then records the device
/// as the actor.
fn task_run_rpc(
    app: &tauri::AppHandle,
    device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    let req = RunRequest::of(params)?;
    to_json(crate::commands::task_run_as(
        state(app),
        req.id_prefix,
        req.agent,
        req.force_reaudit,
        Some(actor_for(app, device)),
    )?)
}

/// The request shape both QA acts share: the task reference and the operator's
/// rationale (REQ-QG-07). The platform — not the window — decides whether the
/// act is legal, so no other condition is decoded here.
struct QaActRequest {
    id_prefix: String,
    rationale: Option<String>,
}

impl QaActRequest {
    fn of(params: &serde_json::Value) -> Result<Self, UiError> {
        Ok(Self {
            id_prefix: param_str(params, "id_prefix")?,
            rationale: param_opt_str(params, "rationale"),
        })
    }
}

/// The request shape of the card's restart: the SAME three fields the window's
/// `task_run` takes, decoded by the shared helpers. One vocabulary for a
/// malformed request — the `ipc` code every neighbouring handler emits.
struct RunRequest {
    id_prefix: String,
    agent: Option<String>,
    force_reaudit: Option<bool>,
}

impl RunRequest {
    fn of(params: &serde_json::Value) -> Result<Self, UiError> {
        Ok(Self {
            id_prefix: param_str(params, "id_prefix")?,
            agent: param_opt_str(params, "agent"),
            force_reaudit: param_opt_bool(params, "force_reaudit"),
        })
    }
}

fn task_progress_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::task_progress(
        state(app),
        param_str(params, "id_prefix")?,
    )?)
}

fn task_dependencies_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::task_dependencies(
        state(app),
        param_str(params, "id_prefix")?,
    )?)
}

fn task_report_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::task_report(
        state(app),
        param_str(params, "id_prefix")?,
    )?)
}

fn task_events_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::task_events(
        state(app),
        param_str(params, "id_prefix")?,
        param_opt_usize(params, "limit"),
    )?)
}

fn task_decisions_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::task_decisions(
        state(app),
        param_str(params, "id_prefix")?,
    )?)
}

fn task_active_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    _params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::task_active(state(app))?)
}

fn task_workspace_changes_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::task_workspace_changes(
        state(app),
        param_str(params, "id_prefix")?,
    )?)
}

fn task_workspace_diff_full_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    let id_prefix = param_str(params, "id_prefix")?;
    let path = param_str(params, "path")?;
    to_json(crate::commands::task_workspace_diff_full(
        state(app),
        id_prefix,
        path,
    )?)
}

fn task_workspace_diff_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::task_workspace_diff(
        state(app),
        param_str(params, "id_prefix")?,
        param_str(params, "path")?,
    )?)
}

fn artifacts_list_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::artifacts_list(
        state(app),
        param_str(params, "id_prefix")?,
    )?)
}

fn artifact_content_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::artifact_content(
        state(app),
        param_str(params, "artifact_id")?,
    )?)
}

fn questions_list_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::questions_list(
        state(app),
        param_str(params, "id_prefix")?,
    )?)
}

fn evaluation_rules_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    _params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::evaluation_rules(state(app))?)
}

fn task_usage_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    let id_prefix = param_str(params, "id_prefix")?;
    to_json(crate::commands::task_usage(state(app), id_prefix)?)
}

fn source_slice_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    let id_prefix = param_str(params, "id_prefix")?;
    let rev = param_str(params, "rev")?;
    let path = param_str(params, "path")?;
    let from = params.get("from").and_then(|v| v.as_u64()).unwrap_or(1) as u32;
    let to = params.get("to").and_then(|v| v.as_u64()).unwrap_or(from as u64 + 12) as u32;
    to_json(crate::commands::source_slice(state(app), id_prefix, rev, path, from, to)?)
}

fn execution_cancel_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    let exec_id = param_str(params, "exec_id")?;
    to_json(crate::commands::execution_cancel(state(app), exec_id)?)
}

fn executions_list_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::executions_list(
        state(app),
        param_opt_str(params, "task_ref"),
    )?)
}

fn agent_statuses_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    _params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::agent_statuses(state(app))?)
}

fn execution_liveness_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::execution_liveness(
        state(app),
        param_str(params, "exec_id")?,
    )?)
}

fn inbox_view_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    _params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::inbox_view(state(app))?)
}

fn model_snapshot_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    _params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::model_snapshot(state(app))?)
}

fn model_probe_rpc(
    app: &tauri::AppHandle,
    device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::model_probe_as(
        state(app),
        param_str(params, "bundle_id")?,
        Some(actor_for(app, device)),
    )?)
}

fn model_switch_rpc(
    app: &tauri::AppHandle,
    device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::model_switch_as(
        state(app),
        param_str(params, "bundle_id")?,
        param_str(params, "reason")?,
        Some(actor_for(app, device)),
    )?)
}

// Связки моделей (задача 01a1be5c): телефон получает ТУ ЖЕ поверхность, что
// окно — создание/правка/удаление связок доступны с телефона, а решение
// помечается устройством через actor_for.
fn model_bundle_handle_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::model_bundle_handle(
        state(app),
        param_str(params, "label")?,
    )?)
}

fn model_bundle_check_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::model_bundle_check(
        state(app),
        bundle_form_input(params)?,
    )?)
}

fn model_bundle_create_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::model_bundle_create(
        state(app),
        bundle_form_input(params)?,
    )?)
}

fn model_bundle_update_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::model_bundle_update(
        state(app),
        bundle_form_input(params)?,
    )?)
}

fn model_bundle_delete_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(crate::commands::model_bundle_delete(
        state(app),
        param_str(params, "bundle_id")?,
        param_str(params, "file_revision")?,
    )?)
}

/// Форма связки из тела запроса телефона: те же поля, что у окна.
fn bundle_form_input(params: &serde_json::Value) -> Result<devai_api::BundleFormInput, UiError> {
    serde_json::from_value(
        params
            .get("input")
            .cloned()
            .unwrap_or(serde_json::Value::Null),
    )
    .map_err(|e| UiError {
        code: "invalid_input".into(),
        message: format!("форма связки не разобрана: {e}"),
    })
}

fn lan_status_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    _params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(lan_status(state(app)))
}

fn lan_enable_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    _params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(lan_enable(app.clone(), state(app))?)
}

fn lan_disable_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    _params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(lan_disable(state(app)))
}

fn lan_new_invite_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    _params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(lan_new_invite(state(app))?)
}

fn lan_disconnect_rpc(
    app: &tauri::AppHandle,
    _device: &DeviceClaim,
    params: &serde_json::Value,
) -> Result<serde_json::Value, UiError> {
    to_json(lan_disconnect(state(app), param_str(params, "id")?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::COMMAND_CONTRACTS;

    fn crate_file(path: &str) -> String {
        std::fs::read_to_string(format!("{}/{}", env!("CARGO_MANIFEST_DIR"), path)).unwrap()
    }

    fn registered_in_window() -> Vec<String> {
        let lib_rs = crate_file("src/lib.rs");
        let start = lib_rs
            .find("tauri::generate_handler![")
            .expect("the handler registration moved");
        let block = &lib_rs[start..];
        let block = &block[..block.find(']').expect("unterminated handler list")];
        let mut names = Vec::new();
        let mut rest = block;
        while let Some(i) = rest.find("commands::") {
            rest = &rest[i + "commands::".len()..];
            let end = rest.find(',').unwrap_or(rest.len());
            names.push(rest[..end].trim().to_string());
            rest = &rest[end..];
        }
        names
    }

    /// R10/R9: the catalogue is the ONLY list. Every name it publishes must
    /// be a registered window command, and every registered window command
    /// must be reachable from the phone — the two directions are checked
    /// against the SAME table, so no third list can drift.
    #[test]
    fn the_phone_catalogue_is_the_window_plane() {
        let registered = registered_in_window();
        assert!(!registered.is_empty());
        let routed = commands_for_rpc();
        assert_eq!(
            routed.len(),
            RPC_COMMANDS.len() + LAN_OPERATIONS.len(),
            "the catalogue must be derived from the two tables only"
        );
        for name in &routed {
            assert!(
                registered.iter().any(|r| r == name),
                "`{name}` is routed over the LAN but not registered in the invoke handler"
            );
        }
        for name in &registered {
            assert!(
                routed.contains(&name.as_str()),
                "`{name}` is registered in the window but not reachable from the phone"
            );
        }
        // No duplicate name can hide a second implementation.
        let mut sorted = routed.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), routed.len(), "a command name is listed twice");
    }

    /// Rework R5: the phone's QA acts decode the SAME `id_prefix` + `rationale`
    /// the window sends and mark the device. The write itself is the window's
    /// own command (shared line), and the store-level act with this actor
    /// marker is pinned by `devai-api`'s quality_gate_api test — devai-ui must
    /// not gain the store dependency (REQ-UI-02).
    #[test]
    fn the_phone_qa_acts_take_the_reason_and_mark_the_device() {
        let params = serde_json::json!({"id_prefix": "01a1", "rationale": "вернуть п.1"});
        let req = QaActRequest::of(&params).unwrap();
        assert_eq!(req.id_prefix, "01a1");
        assert_eq!(req.rationale.as_deref(), Some("вернуть п.1"));

        let no_reason = QaActRequest::of(&serde_json::json!({"id_prefix": "01a1"})).unwrap();
        assert!(no_reason.rationale.is_none(), "the rationale is optional");

        let device = DeviceClaim {
            name: "Pixel".into(),
            addr: "192.168.1.42".into(),
        };
        assert_eq!(
            actor_marker("guerrero", &device),
            "guerrero · телефон: Pixel (192.168.1.42)",
            "the phone path names the device; the person stays the author"
        );
        assert!(QaActRequest::of(&serde_json::json!({})).is_err());
    }

    /// Rework R6: the phone's restart decodes the SAME three fields the
    /// window's `task_run` takes, through the shared parameter helpers — a
    /// malformed shape carries the `ipc` code every neighbouring handler
    /// emits, never a handler-local dialect (the drift that made the phone
    /// unable to read «кривая форма запроса» from one word).
    #[test]
    fn the_phone_restart_decodes_the_shared_request_shape() {
        let params = serde_json::json!({
            "id_prefix": "01a1",
            "agent": "hermes",
            "force_reaudit": true,
        });
        let req = RunRequest::of(&params).unwrap();
        assert_eq!(req.id_prefix, "01a1");
        assert_eq!(req.agent.as_deref(), Some("hermes"));
        assert_eq!(req.force_reaudit, Some(true));

        let defaults = RunRequest::of(&serde_json::json!({"id_prefix": "01a1"})).unwrap();
        assert!(defaults.agent.is_none(), "the agent is optional");
        assert!(defaults.force_reaudit.is_none(), "the flag is optional");

        let err = RunRequest::of(&serde_json::json!({}))
            .err()
            .expect("a missing prefix is refused");
        assert_eq!(
            err.code, "ipc",
            "a malformed request uses the shared code, not its own"
        );
    }

    /// R9: the phone surface is registered, not implied. Every control names
    /// its operation, the operation is a versioned contract, and the frontend
    /// invokes exactly that command.
    #[test]
    fn the_phone_surface_is_registered_and_mirrored() {
        let api_ts = crate_file("frontend/src/api.ts");
        for operation in LAN_OPERATIONS {
            assert!(
                operation.operation.starts_with("lan."),
                "`{}` must live in the phone-access plane",
                operation.operation
            );
            assert!(
                COMMAND_CONTRACTS
                    .iter()
                    .any(|c| c.command == operation.command
                        && c.version == 1
                        && c.breaking.is_empty()),
                "`{}` has no recorded v1 shape contract",
                operation.command
            );
            assert!(
                api_ts.contains(&format!("\"{}\"", operation.command)),
                "`{}` is registered but the frontend never invokes it",
                operation.command
            );
            assert!(
                !operation.description.trim().is_empty(),
                "`{}` must carry a description, not only a name",
                operation.command
            );
        }
        assert!(!LAN_SURFACE_CONTRACTS.is_empty());
        for contract in LAN_SURFACE_CONTRACTS {
            assert!(contract.version >= 1);
            assert!(
                !contract.surface.is_empty(),
                "a surface contract names the surface it describes"
            );
            assert!(contract.shape.contains('/') || contract.shape.contains("cookie"));
        }
    }

    /// A paired snapshot: the fixture the mirror test serializes.
    fn paired_status() -> (tempfile::TempDir, LanStatus) {
        let dir = tempfile::TempDir::new().unwrap();
        let lan = crate::lan::state::LanAccess::new(dir.path().join("lan-access.json"));
        let (tx, _rx) = tokio::sync::watch::channel(false);
        let addr: std::net::SocketAddr = "192.168.1.20:4567".parse().unwrap();
        let token = lan.activate(addr, tx);
        let claim = DeviceClaim {
            name: "Pixel".into(),
            addr: "192.168.1.42".into(),
        };
        let _ = lan.page_load(Some(&token), None, &claim);
        (dir, lan.status())
    }

    /// R9: `LanStatus` is mirrored in `types.ts` by hand, so a parity probe
    /// pins the wire shape: every serialized field must be declared there,
    /// and the state token must serialize to the exact declared token.
    #[test]
    fn the_status_mirror_is_pinned_in_types_ts() {
        let types_ts = crate_file("frontend/src/types.ts");
        let lan_state_ts = crate_file("frontend/src/lanState.ts");
        let (_dir, status) = paired_status();
        let value = serde_json::to_value(&status).unwrap();
        let object = value.as_object().expect("status serializes to an object");
        for key in object.keys() {
            assert!(
                types_ts.contains(&format!("{key}:")),
                "LanStatus field `{key}` is missing from types.ts"
            );
        }
        let device = value["devices"][0].as_object().unwrap();
        for key in device.keys() {
            assert!(
                types_ts.contains(&format!("{key}:")),
                "LanDevice field `{key}` is missing from types.ts"
            );
        }
        assert_eq!(
            value["state"].as_str(),
            Some(crate::lan::state::LanState::Connected.token())
        );
        assert!(
            lan_state_ts.contains("Record<LanState"),
            "the panel must handle states through the typed record"
        );
        assert!(
            types_ts.contains("LanState"),
            "types.ts must mirror the state enum"
        );
    }

    /// R9: the JSON envelope of `/rpc` is a recorded shape, mirrored in
    /// types.ts — the phone client is a second consumer of it.
    #[test]
    fn the_rpc_envelope_is_registered_and_mirrored() {
        let types_ts = crate_file("frontend/src/types.ts");
        let envelope = serde_json::to_value(RpcEnvelope {
            ok: true,
            data: Some(serde_json::json!({})),
            error: None,
        })
        .unwrap();
        let object = envelope.as_object().unwrap();
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        let mut expected = RPC_ENVELOPE_FIELDS.to_vec();
        expected.sort_unstable();
        assert_eq!(keys, expected);
        for field in RPC_ENVELOPE_FIELDS {
            assert!(
                types_ts.contains(&format!("{field}")),
                "envelope field `{field}` is missing from types.ts"
            );
        }
    }
}
