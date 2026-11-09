//! devai-ui — the operator surface of devai, built on Tauri v2 (desktop + Android).
//!
//! Точка входа `run()` живёт в библиотеке, а не в бинаре: мобильной сборке Tauri
//! нужен lib-таргет с `mobile_entry_point` (без него `cargo tauri android build`
//! падает «no library targets found»). Бинарь `main.rs` — тонкая обёртка.
//!
//! Architecture (per the approved spec and the operator's ratified answers):
//!   * Rust process: opens the ACTIVE store-home as an OBSERVATION
//!     (`Api::open_observed`; the migrating bootstrap is used only for a
//!     home that does not exist yet — REQ-HOME-09) and holds the homes
//!     registry of the control home (REQ-HOME-01); publishes read-only
//!     Tauri commands, one per published registry operation (`commands.rs`).
//!     The webview has no other door to data — REQ-API-08 dogfood: no
//!     store access.
//!   * Frontend: Vue + TypeScript (OQ-UI-2, operator's override), served
//!     from `frontend/dist` (built by Vite before `cargo build`).
//!   * The API is an in-process library (OQ-1 ratified); no daemon exists
//!     in v1, so the commands call `devai-api` directly.
//!   * Read-only for domain data (OQ-UI-6): answering questions, approvals
//!     and every other gate action stay in the CLI. Homes-registry
//!     management (add/enable/activate) is the allowed platform plane
//!     whose single write target is the registry file of the control home
//!     (REQ-HOME-06); a broken home is no longer a startup death — the
//!     window opens, the home renders unavailable with its path (REQ-HOME-07
//!     replaces the old exit-2-on-bad-home behavior for observed homes).

mod commands;
mod session;

use std::sync::Mutex;
use tauri::Manager;

/// REQ-UI-01 + REQ-HOME-02: resolve the launch home through the unchanged
/// ratified chain (DEVAI_HOME, else ~/.devai) and auto-register it in the
/// homes registry. The session then opens the STORED ACTIVE home as an
/// OBSERVATION (`Api::open_observed` — read-only, no migration ladder, no
/// `user_version` bump); the migrating `Api::open` is used for exactly one
/// case, a home that carries no store yet (first launch, REQ-HOME-09).
fn bootstrap() -> session::Session {
    // A stored active home that cannot be opened degrades the session to
    // control-home observation with a typed cause in the status bar
    // (REQ-HOME-07), never a silent re-assignment; only an unopenable
    // CONTROL home is a startup error.
    // R3 quality return: the bootstrap refusal is typed (code + home +
    // message), so the process exit prints the machine code first — an
    // operator/consumer reads the KIND without parsing the sentence.
    session::Session::open_env().unwrap_or_else(|failure| {
        eprintln!("devai-ui: [{}] {}", failure.code, failure.message);
        std::process::exit(2);
    })
}

/// The read-only command surface: one handler per published registry
/// operation plus the homes-registry plane (REQ-HOME-05).
fn handlers() -> impl Fn(tauri::ipc::Invoke) -> bool + Send + Sync + 'static {
    tauri::generate_handler![
        commands::session_info,
        commands::registry_operations,
        commands::homes_list,
        commands::homes_add,
        commands::homes_set_enabled,
        commands::homes_activate,
        commands::projects_all,
        commands::tasks_all,
        commands::projects_list,
        commands::projects_get,
        commands::tasks_list,
        commands::task_get,
        commands::task_status,
        commands::task_signature,
        commands::task_approve,
        commands::task_reject,
        commands::task_progress,
        commands::task_dependencies,
        commands::task_report,
        commands::task_events,
        commands::task_decisions,
        commands::task_active,
        commands::task_workspace_changes,
        commands::task_workspace_diff,
        commands::artifacts_list,
        commands::artifact_content,
        commands::questions_list,
        commands::executions_list,
        commands::execution_liveness,
        commands::inbox_view,
        commands::model_snapshot,
        commands::model_probe,
        commands::model_switch,
    ]
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            // На Android у процесса нет $HOME: цепочка DEVAI_HOME → ~/.devai
            // упирается в относительный путь, дом не открывается — и окно
            // закрывалось сразу после запуска (08.11, живой APK). Дом платформы
            // на телефоне живёт в каталоге данных приложения; десктоп не
            // затронут — там решение принимает своя цепочка.
            #[cfg(mobile)]
            let session = {
                // Без env-мутаций: путь дома приходит явно (open_in_app_dir).
                let dir = app
                    .path()
                    .app_data_dir()
                    .unwrap_or_else(|_| std::path::PathBuf::from("."));
                session::Session::open_in_app_dir(dir.join("home")).unwrap_or_else(|failure| {
                    eprintln!("devai-ui: [{}] {}", failure.code, failure.message);
                    std::process::exit(2);
                })
            };
            #[cfg(not(mobile))]
            let session = bootstrap();
            app.manage(commands::AppState {
                api: Mutex::new(session.api),
                homes: Mutex::new(session.homes),
                session: Mutex::new(session.info),
            });
            Ok(())
        })
        // Native folder picker for the homes registry (REQ-HOME-03, R1:
        // the operator lifted the "plugin out of cycle" justification —
        // the dialog is IN). Permission: `dialog:allow-open` in
        // capabilities/default.json; the frontend calls the plugin's own
        // JS API, never a data command (the api.ts door stays the only
        // path to platform data).
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(handlers())
        .run(tauri::generate_context!())
        .expect("devai-ui failed to run");
}
