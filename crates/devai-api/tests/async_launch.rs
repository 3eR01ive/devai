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

use devai_api::{
    Api, AskReq, ReviseReq, RunLaunch, RunReq, SessionKind, SupervisedRunReq, TaskCreateReq,
};
use devai_core::types::*;
use tempfile::TempDir;

// The runner a launch uses is DATA of the handle (`Api::with_runner`), not
// process-global state: every test points ITS OWN handle at its own stub, so
// nothing here mutates the environment the rest of the binary runs in (rework
// R2, ts/test-independence: isolation by structure, not by a lock).

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
    api.task_start(&id).unwrap();
        api.statement_approve_for_tests(&id).unwrap(); // DRAFT -> ANALYSIS (no role ran yet)
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
            r.execution.run_id.map(|x| x.to_string()),
            Some(run.clone()),
            "execution {} must carry the run number",
            r.execution.id
        );
        assert_ne!(r.execution.status, ExecutionStatus::Running);
        // REQ-LV-06 AC4: a finished row is NOT checked — no liveness mark.
        assert_eq!(r.liveness, None, "finished rows carry no verdict");
        assert_eq!(r.liveness_line, None);
    }
    // The first stage really was the analyst's.
    assert_eq!(rows[0].execution.role_slug, "requirements-analyst");
}

#[test]
fn a_second_launch_is_a_typed_refusal_naming_the_owner_and_touches_nothing() {
    let (dir, api) = home();
    let id = task(&api, "busy");
    api.task_start(&id).unwrap();
        api.statement_approve_for_tests(&id).unwrap();
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
    let (dir, api) = home();
    let id = task(&api, "already running");
    api.task_start(&id).unwrap();
        api.statement_approve_for_tests(&id).unwrap();
    let script = stub_runner(dir.path(), "busy");
    let api = api.with_runner(Some(script));
    let o = api
        .task_run(RunReq {
            id: id.clone(),
            agent: "mock".into(),
            workspace: None,
            force_reaudit: false,
        })
        .unwrap();
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
    let (dir, api) = home();
    let id = task(&api, "no runner");
    api.task_start(&id).unwrap();
        api.statement_approve_for_tests(&id).unwrap();
    // `Api::open` never auto-launches, so all attempts are explicit ones.
    // Point the runner resolution at a path that cannot exist: a spawn-class
    // launch failure, deterministic on every host.
    let api = api.with_runner(Some(dir.path().join("no-such-runner")));
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
    let dir = TempDir::new().unwrap();
    // A LIVE session (the CLI/window door) is allowed to launch by itself.
    let api = Api::open_session(dir.path(), "operator", SessionKind::Human).unwrap();
    api.seed_config(None).unwrap();
    let script = stub_runner(dir.path(), "started");
    let api = api.with_runner(Some(script));
    let id = task(&api, "autostart");
    // `task start` enters ANALYSIS, a ROLE state: the platform must start the
    // run without a second command (REQ-AR-11/12).
    api.task_start(&id).unwrap();
        api.statement_approve_for_tests(&id).unwrap();

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
    let dir = TempDir::new().unwrap();
    let api = Api::open_session(dir.path(), "operator", SessionKind::Human).unwrap();
    api.seed_config(None).unwrap();
    let script = stub_runner(dir.path(), "started");
    let api = api.with_runner(Some(script));
    let id = task(&api, "answer resumes");
    api.task_start(&id).unwrap();
        api.statement_approve_for_tests(&id).unwrap(); // enters ANALYSIS, launches (1)
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

    assert_eq!(api.task_get(&id).unwrap().state, TaskState::Analysis);
    let argv = std::fs::read_to_string(&marker).unwrap_or_default();
    assert!(
        argv.contains("__supervise") && argv.contains(&id),
        "the answer resumed the stage through a platform runner: {argv:?}"
    );
}

#[test]
fn revising_the_spec_starts_the_reviewer_itself() {
    let dir = TempDir::new().unwrap();
    let api = Api::open_session(dir.path(), "operator", SessionKind::Human).unwrap();
    api.seed_config(None).unwrap();
    let script = stub_runner(dir.path(), "started");
    let api = api.with_runner(Some(script));
    let id = task(&api, "revise launches review");
    api.task_start(&id).unwrap();
        api.statement_approve_for_tests(&id).unwrap(); // launches (1)
    let marker = dir.path().join("argv");
    std::fs::write(&marker, "").unwrap();
    api.task_revise(ReviseReq {
        id: id.clone(),
        name: "spec.md".into(),
        content: br#"{"requirements":[],"human_paragraph":"revised"}"#.to_vec(),
        reason: Some("operator edited".into()),
    })
    .unwrap();

    assert_eq!(api.task_get(&id).unwrap().state, TaskState::Review);
    let argv = std::fs::read_to_string(&marker).unwrap_or_default();
    assert!(
        argv.contains("__supervise") && argv.contains(&id),
        "the revision started the reviewer through a platform runner: {argv:?}"
    );
}

/// REQ-AR-12 AC1 from THE OPERATOR'S OWN SURFACE: the window signs gates, and
/// its write handle must therefore be a live platform session — signing the
/// requirements gate starts the implementation run itself (QA R4 on task
/// 01a1b7dc: `write_api` opened the bootstrap door and the task sat in
/// IMPLEMENTATION with no run).
#[test]
fn signing_the_gate_from_the_window_starts_the_implementation_itself() {
    let dir = TempDir::new().unwrap();
    // EXACTLY the door `AppState::write_api` opens for a signature.
    let api = Api::open_for_window(dir.path(), "operator").unwrap();
    api.seed_config(None).unwrap();
    let script = stub_runner(dir.path(), "started");
    let api = api.with_runner(Some(script));
    let id = task(&api, "window signature");
    api.task_start(&id).unwrap();
        api.statement_approve_for_tests(&id).unwrap(); // DRAFT -> ANALYSIS (launch 1)
    api.task_revise(ReviseReq {
        id: id.clone(),
        name: "spec.md".into(),
        content: br#"{"requirements":[],"human_paragraph":"window signed"}"#.to_vec(),
        reason: None,
    })
    .unwrap(); // -> REVIEW (launch 2)
    api.task_review(&id, Verdict::Approved, None).unwrap(); // -> APPROVAL
    assert_eq!(api.task_get(&id).unwrap().state, TaskState::Approval);

    // Forget the earlier launches; only the SIGNATURE may write here.
    let marker = dir.path().join("argv");
    std::fs::write(&marker, "").unwrap();
    api.task_approve(&id, Some("ok".into())).unwrap();

    assert_eq!(
        api.task_get(&id).unwrap().state,
        TaskState::Implementation,
        "the signature moved the task to the role state"
    );
    let argv = std::fs::read_to_string(&marker).unwrap_or_default();
    assert!(
        argv.contains("__supervise") && argv.contains(&id),
        "the window's signature started the implementation through a platform runner: {argv:?}"
    );
}

/// R5: the settled outcome is written BEST EFFORT — a role's answer is never
/// lost because a log could not be written — but the failure is not silence:
/// the store carries the event with the path and the reason, so a `--wait`
/// caller falling back to the row listing can see WHY (`supervisor.log` gets
/// the same line, because the runner's stderr is redirected there).
#[test]
fn an_outcome_that_cannot_be_written_leaves_a_trace_in_the_store() {
    let (dir, api) = home();
    let id = task(&api, "unwritable outcome");
    api.task_start(&id).unwrap();
        api.statement_approve_for_tests(&id).unwrap();
    // The run's evidence directory is deliberately never created, so the
    // outcome file cannot land (a real runner creates it; here it cannot).
    let run = new_id().to_string();
    let report = api
        .task_run_supervised(&SupervisedRunReq {
            task_id: id.clone(),
            agent: "mock".into(),
            workspace: None,
            force_reaudit: false,
            run_id: run.clone(),
            ack: dir.path().join("ack"),
        })
        .unwrap();
    assert!(report.is_some(), "the chain itself still ran");

    let events = api.task_events(&id, None).unwrap();
    let trace = events
        .iter()
        .find(|ev| ev.kind == event_kind::EXECUTION_EVIDENCE_UNWRITTEN)
        .expect("the unwritten outcome is a stored fact, not silence");
    assert_eq!(trace.payload["run_id"], run, "the event names the run");
    let path = trace.payload["path"].as_str().unwrap();
    assert!(path.ends_with("outcome.json"), "the path is named: {path}");
    assert!(
        path.contains(&run),
        "the path is the run's own evidence file: {path}"
    );
    assert!(
        !trace.payload["reason"].as_str().unwrap().is_empty(),
        "the reason is recorded: {trace:?}"
    );
}
