//! The async launch surface on a REAL home (REQ-AR-04/06/08/11/12).
//!
//! The detached SPAWN itself is covered by `devai-runtime`'s
//! `async_launch.rs` (own session + handshake). Here the platform's side of
//! the contract is pinned:
//! * the supervisor opens the execution row, hands the caller the id only
//!   after the row exists, and drives the chain to the gate;
//! * every execution of the chain carries one run number;
//! * a second launch is `already_running` with the owner's id and the first
//!   run is untouched;
//! * launching a HUMAN state is refused with a reason;
//! * a human decision that lands in a role state starts the run itself.

use std::path::Path;
use std::sync::Mutex;

use devai_api::{
    Api, AskReq, ReviseReq, RunLaunch, RunReq, SessionKind, SupervisedRunReq, TaskCreateReq,
};
use devai_core::types::*;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

fn home() -> (TempDir, Api) {
    let dir = TempDir::new().unwrap();
    let api = Api::open(dir.path(), "async-tester").unwrap();
    api.seed_config(None).unwrap();
    (dir, api)
}

fn task(api: &Api, title: &str) -> String {
    api.task_create(TaskCreateReq {
        title: title.into(),
        project_id: None,
        description: "async launch".into(),
        parent: Vec::new(),
        blocked_by: Vec::new(),
        idempotency_key: None,
    })
    .unwrap()
    .task
    .id
    .to_string()
}

fn ack_text(p: &Path) -> String {
    std::fs::read_to_string(p).unwrap_or_default()
}

#[test]
fn the_supervisor_opens_the_row_hands_a_handle_and_drives_the_chain() {
    let (dir, api) = home();
    let id = task(&api, "supervised");
    api.task_start(&id).unwrap(); // DRAFT -> ANALYSIS (no role ran yet)
    let ack = dir.path().join("ack");
    let run = new_id().to_string();
    let report = api
        .task_run_supervised(&SupervisedRunReq {
            task_id: id.clone(),
            agent: "mock".into(),
            workspace: None,
            force_reaudit: false,
            run_id: run.clone(),
            ack: ack.clone(),
        })
        .unwrap()
        .expect("the supervisor drives the chain");

    // The caller got the handle AFTER the row existed (REQ-AR-08).
    let ack = ack_text(&ack);
    assert!(ack.starts_with("started "), "handshake: {ack:?}");
    assert!(ack.contains(&run), "handshake names the run: {ack:?}");

    // The mock chain reached the human approval gate.
    assert_eq!(report.gate, devai_runtime::Gate::WaitingApproval);

    // ONE run number for every execution of the chain (REQ-AR-04/07), and
    // nothing is left RUNNING after the driver returns.
    let rows = api.exec_list(Some(&id)).unwrap();
    assert!(
        rows.len() >= 2,
        "the chain ran more than one stage: {rows:?}"
    );
    for r in &rows {
        assert_eq!(
            r.run_id.map(|x| x.to_string()),
            Some(run.clone()),
            "execution {} must carry the run number",
            r.id
        );
        assert_ne!(r.status, ExecutionStatus::Running);
    }
    // The first stage really was the analyst's.
    assert_eq!(rows[0].role_slug, "requirements-analyst");
}

#[test]
fn a_second_launch_is_a_typed_refusal_naming_the_owner_and_touches_nothing() {
    let (dir, api) = home();
    let id = task(&api, "busy");
    api.task_start(&id).unwrap();
    let first = api
        .exec_start(devai_api::ExecStartReq {
            id: id.clone(),
            role: "requirements-analyst".into(),
            agent: Some("mock".into()),
            inputs: Vec::new(),
            run_id: None,
        })
        .unwrap()
        .execution;

    let ack = dir.path().join("ack");
    let out = api
        .task_run_supervised(&SupervisedRunReq {
            task_id: id.clone(),
            agent: "mock".into(),
            workspace: None,
            force_reaudit: false,
            run_id: new_id().to_string(),
            ack: ack.clone(),
        })
        .unwrap();
    assert!(out.is_none(), "nothing is driven over a live owner");

    let ack = ack_text(&ack);
    assert!(ack.starts_with("busy "), "handshake: {ack:?}");
    assert!(
        ack.contains(&first.id.to_string()),
        "the refusal names the execution that owns the task: {ack:?}"
    );
    // The first run was not signalled, killed or touched.
    let row = api.exec_get(&first.id.to_string()).unwrap();
    assert_eq!(row.status, ExecutionStatus::Running);
}

#[test]
fn a_human_executed_state_is_refused_with_its_reason() {
    let (dir, api) = home();
    let id = task(&api, "human gate");
    api.task_run_inline(RunReq {
        id: id.clone(),
        agent: "mock".into(),
        workspace: None,
        force_reaudit: false,
    })
    .unwrap();
    assert_eq!(
        api.task_get(&id).unwrap().state,
        TaskState::Approval,
        "the inline driver stopped at the human gate"
    );
    let ack = dir.path().join("ack");
    let out = api
        .task_run_supervised(&SupervisedRunReq {
            task_id: id.clone(),
            agent: "mock".into(),
            workspace: None,
            force_reaudit: false,
            run_id: new_id().to_string(),
            ack: ack.clone(),
        })
        .unwrap();
    assert!(out.is_none());
    let ack = ack_text(&ack);
    assert!(ack.starts_with("refused "), "handshake: {ack:?}");
    assert!(
        ack.contains("APPROVAL"),
        "the reason names the state: {ack:?}"
    );
}

/// A stub runner: it records its argv and answers the handshake. Used to pin
/// the CLIENTS of launch without spawning the real platform runner.
fn stub_runner(dir: &Path, answer: &'static str) -> std::path::PathBuf {
    let script = dir.join("stub-runner.sh");
    let marker = dir.join("argv");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\n\
             echo \"$@\" >> {marker}\n\
             ack=\"\"; run=\"\"; prev=\"\"\n\
             for a in \"$@\"; do\n\
               [ \"$prev\" = \"--ack\" ] && ack=\"$a\"\n\
               [ \"$prev\" = \"--run\" ] && run=\"$a\"\n\
               prev=\"$a\"\n\
             done\n\
             printf '{answer} %s 01890000-0000-7000-8000-000000000009\\n' \"$run\" > \"$ack\"\n",
            marker = marker.display(),
            answer = answer
        ),
    )
    .unwrap();
    use std::os::unix::fs::PermissionsExt;
    let mut perms = std::fs::metadata(&script).unwrap().permissions();
    perms.set_mode(0o755);
    std::fs::set_permissions(&script, perms).unwrap();
    script
}

#[test]
fn task_run_returns_the_typed_already_running_outcome_with_the_owner_id() {
    let _guard = ENV_LOCK.lock().unwrap();
    let (dir, api) = home();
    let id = task(&api, "already running");
    api.task_start(&id).unwrap();
    let script = stub_runner(dir.path(), "busy");
    unsafe { std::env::set_var("DEVAI_RUNNER", &script) };
    let o = api
        .task_run(RunReq {
            id: id.clone(),
            agent: "mock".into(),
            workspace: None,
            force_reaudit: false,
        })
        .unwrap();
    unsafe { std::env::remove_var("DEVAI_RUNNER") };
    assert_eq!(o.outcome, RunLaunch::AlreadyRunning);
    assert_eq!(
        o.execution_id.as_deref(),
        Some("01890000-0000-7000-8000-000000000009")
    );
    assert!(o.reason.is_some());
    // The caller learned the run number it asked with.
    assert!(!o.run_id.is_empty());
}

#[test]
fn repeated_launch_failures_are_counted_events_and_the_threshold_parks_the_task() {
    // REQ-AR-13: a launch that DID NOT happen is not «the role ran» — it is an
    // event with the class and the consecutive counter, and the declared
    // threshold parks the task instead of retrying silently.
    let _guard = ENV_LOCK.lock().unwrap();
    let (dir, api) = home();
    let id = task(&api, "no runner");
    api.task_start(&id).unwrap();
    // `Api::open` never auto-launches, so all attempts are explicit ones.
    unsafe { std::env::remove_var("DEVAI_RUNNER") };
    // Point the runner resolution at a path that cannot exist: a spawn-class
    // launch failure, deterministic on every host.
    unsafe { std::env::set_var("DEVAI_RUNNER", dir.path().join("no-such-runner")) };
    for attempt in 1..=2 {
        let e = api
            .task_run(RunReq {
                id: id.clone(),
                agent: "mock".into(),
                workspace: None,
                force_reaudit: false,
            })
            .unwrap_err();
        assert_eq!(e.code(), devai_api::ErrorCode("launch_failed"), "{e}");
        let events = api.task_events(&id, None).unwrap();
        let failed: Vec<&WorkflowEvent> = events
            .iter()
            .filter(|ev| ev.kind == event_kind::EXECUTION_LAUNCH_FAILED)
            .collect();
        assert_eq!(failed.len(), attempt, "one event per attempt");
        assert_eq!(
            failed.last().unwrap().payload["consecutive"],
            attempt,
            "the counter is consecutive, not reset"
        );
    }
    // Third failure: the declared brake (3) parks the task, and the event says
    // so; the task is no longer in a role state, so nothing retries.
    let _ = api.task_run(RunReq {
        id: id.clone(),
        agent: "mock".into(),
        workspace: None,
        force_reaudit: false,
    });
    unsafe { std::env::remove_var("DEVAI_RUNNER") };
    let events = api.task_events(&id, None).unwrap();
    let last = events
        .iter()
        .find(|ev| ev.kind == event_kind::EXECUTION_LAUNCH_FAILED && ev.payload["parked"] == true)
        .expect("the parking launch failure is an event");
    assert_eq!(last.payload["consecutive"], 3);
    assert!(
        events
            .iter()
            .any(|ev| ev.kind == event_kind::PROCESS_PARKED),
        "a tripped brake records the park"
    );
    assert_eq!(
        api.task_get(&id).unwrap().state,
        TaskState::Blocked,
        "the task is parked, not left in a role state to retry"
    );
}

#[test]
fn entering_a_role_state_after_a_human_act_starts_the_run_itself() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    // A LIVE session (the CLI/window door) is allowed to launch by itself.
    let api = Api::open_session(dir.path(), "operator", SessionKind::Human).unwrap();
    api.seed_config(None).unwrap();
    let script = stub_runner(dir.path(), "started");
    unsafe { std::env::set_var("DEVAI_RUNNER", &script) };
    let id = task(&api, "autostart");
    // `task start` enters ANALYSIS, a ROLE state: the platform must start the
    // run without a second command (REQ-AR-11/12).
    api.task_start(&id).unwrap();
    unsafe { std::env::remove_var("DEVAI_RUNNER") };

    let argv = std::fs::read_to_string(dir.path().join("argv")).unwrap_or_default();
    assert!(
        argv.contains("__supervise") && argv.contains(&id),
        "the platform started the runner for the task itself: {argv:?}"
    );
}

/// REQ-AR-11 AC1 for the OTHER human doors into a role state: answering the
/// question a role parked on, and publishing a revised spec. Both land the
/// task back in a stage a role executes — the platform starts its run itself,
/// a second command is not the operator's job.
#[test]
fn answering_a_blocking_question_starts_the_resumed_stage_itself() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let api = Api::open_session(dir.path(), "operator", SessionKind::Human).unwrap();
    api.seed_config(None).unwrap();
    let script = stub_runner(dir.path(), "started");
    unsafe { std::env::set_var("DEVAI_RUNNER", &script) };
    let id = task(&api, "answer resumes");
    api.task_start(&id).unwrap(); // enters ANALYSIS, launches (1)
    let asked = api
        .question_ask(AskReq {
            id: id.clone(),
            text: "which way?".into(),
            context: None,
            options: vec![],
            blocking: true,
            execution: None,
        })
        .unwrap();
    assert_eq!(api.task_get(&id).unwrap().state, TaskState::WaitingForInput);
    // Forget the `task start` launch; only the answer's launch may land here.
    let marker = dir.path().join("argv");
    std::fs::write(&marker, "").unwrap();
    api.question_answer(&asked.question.id.to_string(), "this way")
        .unwrap();
    unsafe { std::env::remove_var("DEVAI_RUNNER") };

    assert_eq!(api.task_get(&id).unwrap().state, TaskState::Analysis);
    let argv = std::fs::read_to_string(&marker).unwrap_or_default();
    assert!(
        argv.contains("__supervise") && argv.contains(&id),
        "the answer resumed the stage through a platform runner: {argv:?}"
    );
}

#[test]
fn revising_the_spec_starts_the_reviewer_itself() {
    let _guard = ENV_LOCK.lock().unwrap();
    let dir = TempDir::new().unwrap();
    let api = Api::open_session(dir.path(), "operator", SessionKind::Human).unwrap();
    api.seed_config(None).unwrap();
    let script = stub_runner(dir.path(), "started");
    unsafe { std::env::set_var("DEVAI_RUNNER", &script) };
    let id = task(&api, "revise launches review");
    api.task_start(&id).unwrap(); // launches (1)
    let marker = dir.path().join("argv");
    std::fs::write(&marker, "").unwrap();
    api.task_revise(ReviseReq {
        id: id.clone(),
        name: "spec.md".into(),
        content: br#"{"requirements":[],"human_paragraph":"revised"}"#.to_vec(),
        reason: Some("operator edited".into()),
    })
    .unwrap();
    unsafe { std::env::remove_var("DEVAI_RUNNER") };

    assert_eq!(api.task_get(&id).unwrap().state, TaskState::Review);
    let argv = std::fs::read_to_string(&marker).unwrap_or_default();
    assert!(
        argv.contains("__supervise") && argv.contains(&id),
        "the revision started the reviewer through a platform runner: {argv:?}"
    );
}
