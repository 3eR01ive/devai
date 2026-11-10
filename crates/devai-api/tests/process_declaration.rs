//! The process declaration is DATA, and the platform WALKS it (REQ-AR-09 AC1).
//!
//! The audit's live check was: change `config/workflow/process.yaml` in a
//! fresh home and see `task start` still go DRAFT → ANALYSIS. These tests pin
//! the opposite: the file drives the walk, and a declaration that cannot
//! express what the code does is refused at initialisation instead of being
//! silently outvoted by the code (QA R1/R4 on task 01a1b7dc).

use devai_api::{Api, TaskCreateReq};
use devai_core::types::*;
use tempfile::TempDir;

/// The factory declaration, verbatim, as the starting point of every edit.
fn factory_text() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../config/workflow/process.yaml"
    ))
    .unwrap()
}

/// A home directory whose `config/workflow/process.yaml` is the given text.
fn home_dir_with(declaration: &str) -> TempDir {
    let dir = TempDir::new().unwrap();
    let wf = dir.path().join("config").join("workflow");
    std::fs::create_dir_all(&wf).unwrap();
    std::fs::write(wf.join("process.yaml"), declaration).unwrap();
    dir
}

/// A home whose declaration is accepted: the API opens it and the store is
/// seeded.
fn home_with(declaration: &str) -> (TempDir, Api) {
    let dir = home_dir_with(declaration);
    let api = Api::open(dir.path(), "declaration-tester").unwrap();
    api.seed_config(None).unwrap();
    (dir, api)
}

/// The typed refusal the INIT doors give for `declaration`: the API will not
/// even open the home, and the store-level loader says the same thing.
fn init_refusal(declaration: &str) -> String {
    let dir = home_dir_with(declaration);
    // `devai init` = open the home, seed the configuration: the PROCESS
    // declaration is validated there (REQ-AR-16), before any stage can run.
    let api = Api::open(dir.path(), "declaration-tester").unwrap();
    let open = match api.seed_config(None) {
        Ok(_) => panic!("a diverging declaration must not pass initialisation"),
        Err(e) => e.to_string(),
    };
    let loader =
        devai_store::config::load_process_policy(&dir.path().join("config").join("workflow"))
            .unwrap_err()
            .to_string();
    assert!(
        open.contains(&loader),
        "the door and the loader refuse for the same reason: {open} / {loader}"
    );
    loader
}

fn task(api: &Api, title: &str) -> String {
    api.task_create(TaskCreateReq {
        title: title.into(),
        project_id: None,
        description: "declaration".into(),
        parent: Vec::new(),
        blocked_by: Vec::new(),
        idempotency_key: None,
    })
    .unwrap()
    .task
    .id
    .to_string()
}

/// REQ-AR-09 AC1: editing the DECLARATION changes the walk — no code edit.
#[test]
fn editing_the_declaration_moves_the_task_where_the_data_says() {
    let text = factory_text().replace(
        "{ from: DRAFT, on: human_start, to: ANALYSIS }",
        "{ from: DRAFT, on: human_start, to: REVIEW }",
    );
    let (_dir, api) = home_with(&text);
    let id = task(&api, "declaration drives start");
    api.task_start(&id).unwrap();
    assert_eq!(
        api.task_get(&id).unwrap().state,
        TaskState::Review,
        "the declaration, not a code constant, chose the target of `start`"
    );
}

/// The same for a custody hold: its ADDRESS is declared too.
#[test]
fn editing_the_declared_hold_address_changes_where_a_block_lands() {
    let text = factory_text().replace("    block: BLOCKED", "    block: PAUSED");
    let (_dir, api) = home_with(&text);
    let id = task(&api, "declared hold");
    api.task_start(&id).unwrap();
    api.task_block(&id, Some("hold".into())).unwrap();
    assert_eq!(
        api.task_get(&id).unwrap().state,
        TaskState::Paused,
        "`holds.block` is data: the same command parks where the file says"
    );
}

/// REQ-AR-09/16: the CODE needs an edge, the declaration does not have it —
/// `devai init` refuses, naming both sides. The code never quietly wins.
#[test]
fn a_declaration_missing_a_needed_edge_is_refused_at_init() {
    let text = factory_text().replace("    - { from: QA, on: human_accepted, to: ACCEPTED }\n", "");
    let err = init_refusal(&text);
    assert!(
        err.contains("human_accepted") && err.contains("QA"),
        "the refusal names the state and the condition: {err}"
    );
}

/// The reverse direction: a state the code can be in with no declared row is
/// a divergence too — refused, not absorbed into an executor guess.
#[test]
fn a_state_without_a_declared_row_is_refused_at_init() {
    let text = factory_text().replace(
        "    - state: CANCELLED\n      executor: none\n      outcomes: []\n",
        "",
    );
    let err = init_refusal(&text);
    assert!(
        err.contains("CANCELLED"),
        "the refusal names the undeclared state: {err}"
    );
}

/// The factory declaration itself is valid — the baseline every edit above
/// starts from.
#[test]
fn the_factory_declaration_is_valid() {
    assert!(devai_store::config::factory_process_policy().is_ok());
    // And its names are the ones the code emits: the operator-facing walk is
    // DRAFT -> ANALYSIS with the shipped bytes.
    let id_task = Task::new("t", "need", None, chrono::Utc::now());
    let m = devai_core::workflow::start(
        devai_store::config::ProcessPolicy::factory(),
        &id_task,
        "tester",
        chrono::Utc::now(),
    )
    .unwrap();
    assert_eq!(m.task.state, TaskState::Analysis);
}
