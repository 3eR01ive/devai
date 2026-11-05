//! devai-ui — the desktop operator surface of devai, built on Tauri v2.
//!
//! Architecture (per the approved spec and the operator's ratified answers):
//!   * Rust process: opens ONE store-home through `Api::open` (bootstrap,
//!     ratified OQ-10) and publishes read-only Tauri commands, one per
//!     published registry operation (`commands.rs`). The webview has no
//!     other door to data — REQ-API-08 dogfood: no store access.
//!   * Frontend: Vue + TypeScript (OQ-UI-2, operator's override), served
//!     from `frontend/dist` (built by Vite before `cargo build`).
//!   * The API is an in-process library (OQ-1 ratified); no daemon exists
//!     in v1, so the commands call `devai-api` directly.
//!   * Read-only iteration (OQ-UI-6): answering questions, approvals and
//!     every other gate action stay in the CLI.

mod commands;
mod session;

use std::sync::Mutex;

fn main() {
    // REQ-UI-01: resolve the home through the ratified GUI chain
    // (DEVAI_HOME, else ~/.devai) and open it ONLY through Api::open.
    // A broken/absent home is a startup failure with the platform's own
    // text (U-5: SchemaVersion mismatch and friends render verbatim).
    let session = session::Session::open_env().unwrap_or_else(|msg| {
        eprintln!("devai-ui: {msg}");
        std::process::exit(2);
    });

    let state = commands::AppState {
        api: Mutex::new(session.api),
        session: session.info,
    };

    tauri::Builder::default()
        .manage(state)
        .invoke_handler(tauri::generate_handler![
            commands::session_info,
            commands::registry_operations,
            commands::projects_list,
            commands::projects_get,
            commands::tasks_list,
            commands::task_get,
            commands::task_status,
            commands::task_progress,
            commands::task_dependencies,
            commands::task_report,
            commands::task_events,
            commands::task_decisions,
            commands::task_active,
            commands::artifacts_list,
            commands::artifact_content,
            commands::questions_list,
            commands::executions_list,
            commands::execution_liveness,
            commands::inbox_view,
        ])
        .run(tauri::generate_context!())
        .expect("devai-ui failed to run");
}
