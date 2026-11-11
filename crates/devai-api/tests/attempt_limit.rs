//! The attempt limit of a role (REQ-AL-01..08) on a REAL home.
//!
//! What is pinned here:
//! * five completed launches of one role without a shift stop the task with
//!   ONE stored question (role, attempts, limit, two options) and start no
//!   sixth run;
//! * the four previous launches are admitted — the rule never fires early;
//! * the old silent round brake is gone: the stop is a QUESTION, not a park
//!   without an answer;
//! * the human answer is the only reset: «продолжить» zeroes the counter and
//!   returns the task to its stage, «остановить» leaves it in human custody;
//! * the counter is visible in `task.status` / `task.progress` below the
//!   limit too, and a human-only state carries none;
//! * a failed launch spends no attempt, and a declaration with `0` means
//!   «без предела».

use std::path::Path;

use chrono::Utc;
use devai_api::{
    Api, AskReq, AttemptLimitView, RunLaunch, RunReq, SupervisedRunReq, TaskCreateReq,
};
use devai_core::types::*;
use devai_store::Store;
use tempfile::TempDir;

fn home() -> (TempDir, Api) {
    let dir = TempDir::new().unwrap();
    let api = Api::open(dir.path(), "attempt-tester").unwrap();
    api.seed_config(None).unwrap();
    (dir, api)
}

fn task(api: &Api) -> String {
    let id = api
        .task_create(TaskCreateReq {
            title: "attempt limit".into(),
            project_id: None,
            description: "five attempts without progress".into(),
            parent: Vec::new(),
            blocked_by: Vec::new(),
            idempotency_key: None,
        })
        .unwrap()
        .task
        .id
        .to_string();
    // DRAFT -> ANALYSIS (no role ran yet: the row count stays ours).
    api.task_start(&id).unwrap();
    id
}

/// One COMPLETED stage launch of `role` with no artifact and no state change:
/// exactly an attempt without progress. Written through the STORE (owner
/// unset), because a row opened by the test process would read as a live
/// owner — the platform's own rows are owned by the runner that exits.
fn run_once(store: &Store, task_id: Id, role: &str) {
    let e = RoleExecution {
        id: new_id(),
        task_id,
        role_slug: role.to_string(),
        role_version: 1,
        agent_name: Some("mock".into()),
        status: ExecutionStatus::Running,
        inputs: Vec::new(),
        summary: None,
        error: None,
        started_at: Utc::now(),
        finished_at: None,
        workspace_revision: None,
        attempt: 1,
        parent_execution: None,
        failure_class: None,
        owner_pid: None,
        owner_host: None,
        owner_process_start: None,
        bundle: None,
        run_id: None,
    };
    store.start_execution(&e).unwrap();
    store
        .finish_execution(
            &e,
            ExecutionStatus::Completed,
            Some("no shift"),
            None,
            Utc::now(),
        )
        .unwrap();
}

fn failed_once(store: &Store, task_id: Id, role: &str) {
    let e = RoleExecution {
        id: new_id(),
        task_id,
        role_slug: role.to_string(),
        role_version: 1,
        agent_name: Some("mock".into()),
        status: ExecutionStatus::Running,
        inputs: Vec::new(),
        summary: None,
        error: None,
        started_at: Utc::now(),
        finished_at: None,
        workspace_revision: None,
        attempt: 1,
        parent_execution: None,
        failure_class: None,
        owner_pid: None,
        owner_host: None,
        owner_process_start: None,
        bundle: None,
        run_id: None,
    };
    store.start_execution(&e).unwrap();
    store
        .finish_execution(
            &e,
            ExecutionStatus::Failed,
            None,
            Some("provider refused"),
            Utc::now(),
        )
        .unwrap();
}

fn seed(api: &Api, home: &Path, id: &str, n: usize) {
    let store = Store::open(home).unwrap();
    let tid = api.task_get(id).unwrap().id;
    for _ in 0..n {
        run_once(&store, tid, "requirements-analyst");
    }
}

fn seed_failed(api: &Api, home: &Path, id: &str, n: usize) {
    let store = Store::open(home).unwrap();
    let tid = api.task_get(id).unwrap().id;
    for _ in 0..n {
        failed_once(&store, tid, "requirements-analyst");
    }
}

/// The supervisor's refusal handshake: `None` means the launch did not
/// happen; the ack carries the reason the operator reads.
fn supervised(api: &Api, home: &Path, task_id: &str) -> Option<String> {
    let ack = home.join(format!("ack-{}", new_id()));
    let out = api
        .task_run_supervised(&SupervisedRunReq {
            task_id: task_id.to_string(),
            agent: "mock".into(),
            workspace: None,
            force_reaudit: false,
            run_id: new_id().to_string(),
            ack: ack.clone(),
        })
        .unwrap();
    if out.is_some() {
        return None;
    }
    Some(std::fs::read_to_string(&ack).unwrap_or_default())
}

fn event_count(api: &Api, task_id: &str, kind: &str) -> usize {
    api.task_events(task_id, None)
        .unwrap()
        .iter()
        .filter(|e| e.kind == kind)
        .count()
}

#[test]
fn raising_or_lowering_the_limit_never_resumes_a_stopped_task() {
    let (dir, api) = home();
    let id = task(&api);
    seed(&api, dir.path(), &id, 5);
    assert!(supervised(&api, dir.path(), &id).is_some());
    assert_eq!(
        api.task_status(&id).unwrap().task.state,
        TaskState::WaitingForInput
    );

    // The declaration now says 10: the counter reads «5 из 10», but the task
    // does NOT come back to life on its own (REQ-AL-04 AC5) — only a human
    // answer moves it.
    let path = dir.path().join("config/workflow/process.yaml");
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(
        &path,
        text.replace(
            "max_attempts_without_progress: 5",
            "max_attempts_without_progress: 10",
        ),
    )
    .unwrap();

    let status = api.task_status(&id).unwrap();
    assert_eq!(status.task.state, TaskState::WaitingForInput);
    assert_eq!(status.open_questions.len(), 1, "the question stayed open");
    let view = status.attempt_limit.unwrap();
    assert_eq!(view.attempts, 5, "the count is history, not the limit");
    assert_eq!(view.line, "попыток без прогресса: 5 из 10");

    // Lowering it again changes nothing either, and a plain launch starts
    // nothing: the held task is not released by data.
    std::fs::write(
        &path,
        text.replace(
            "max_attempts_without_progress: 5",
            "max_attempts_without_progress: 2",
        ),
    )
    .unwrap();
    let refused = api
        .task_run(RunReq {
            id: id.clone(),
            agent: "mock".into(),
            workspace: None,
            force_reaudit: false,
        })
        .unwrap();
    assert_eq!(refused.outcome, RunLaunch::Refused);
    let after = api.task_status(&id).unwrap();
    assert_eq!(after.task.state, TaskState::WaitingForInput);
    assert_eq!(after.attempt_limit.unwrap().attempts, 5);
    assert_eq!(api.exec_list(Some(&id)).unwrap().len(), 5);
}

#[test]
fn an_already_open_question_waits_instead_of_a_second_one() {
    let (dir, api) = home();
    let id = task(&api);
    // A NON-blocking question is open while the stage is still ACTIVE: exactly
    // the case where the limit would otherwise add its own question
    // (REQ-AL-05 AC4).
    let asked = api
        .question_ask(AskReq {
            id: id.clone(),
            text: "уточните приоритет".into(),
            context: None,
            options: Vec::new(),
            blocking: false,
            execution: None,
        })
        .unwrap();
    assert!(!asked.parked);
    seed(&api, dir.path(), &id, 5);

    let ack = supervised(&api, dir.path(), &id).expect("nothing launches at the limit");
    assert!(ack.starts_with("refused "), "{ack:?}");
    assert!(
        ack.contains("уже открыт вопрос"),
        "the refusal names the reason: {ack:?}"
    );

    let status = api.task_status(&id).unwrap();
    assert_eq!(
        status.task.state,
        TaskState::Analysis,
        "the limit waits for the open question instead of stopping the task"
    );
    assert_eq!(status.open_questions.len(), 1, "no second question");
    assert_eq!(
        status.open_questions[0].id, asked.question.id,
        "the open question is the pre-existing one"
    );
    assert_eq!(
        event_count(&api, &id, event_kind::ATTEMPT_LIMIT_TRIPPED),
        0,
        "the brake did not trip while a question is already open"
    );
    assert_eq!(api.exec_list(Some(&id)).unwrap().len(), 5);
}

#[test]
fn five_attempts_without_progress_stop_the_task_with_one_stored_question() {
    let (dir, api) = home();
    let id = task(&api);
    seed(&api, dir.path(), &id, 5);
    assert_eq!(
        api.exec_list(Some(&id)).unwrap().len(),
        5,
        "the fixture really ran five stage launches"
    );

    let ack = supervised(&api, dir.path(), &id).expect("the sixth run must not start");
    assert!(ack.starts_with("refused "), "handshake: {ack:?}");
    assert!(
        ack.contains("5 из 5"),
        "the refusal speaks the status numbers: {ack:?}"
    );

    let status = api.task_status(&id).unwrap();
    assert_eq!(status.task.state, TaskState::WaitingForInput);
    assert_eq!(
        status.task.return_state,
        Some(TaskState::Analysis),
        "the interrupted stage is remembered for a resume"
    );
    assert_eq!(status.open_questions.len(), 1, "ONE honest question");
    let q = &status.open_questions[0];
    assert!(q.blocking);
    assert_eq!(
        q.options,
        vec!["продолжить".to_string(), "остановить".to_string()]
    );
    assert!(q.text.contains("requirements-analyst"), "{}", q.text);
    assert!(
        q.text.contains("5"),
        "the count is in the question: {}",
        q.text
    );
    assert!(
        q.context.as_deref().unwrap_or_default().contains("limit=5"),
        "context: {:?}",
        q.context
    );

    let view: AttemptLimitView = status
        .attempt_limit
        .expect("the counter rides on the status");
    assert_eq!(view.role, "requirements-analyst");
    assert_eq!(view.attempts, 5);
    assert_eq!(view.limit, Some(5));
    assert_eq!(view.line, "попыток без прогресса: 5 из 5");

    // No sixth execution row: the stop happened BEFORE the launch.
    assert_eq!(api.exec_list(Some(&id)).unwrap().len(), 5);
    assert_eq!(event_count(&api, &id, event_kind::ATTEMPT_LIMIT_TRIPPED), 1);
    // The old silent brake wrote no question and is gone from the flow.
    assert_eq!(event_count(&api, &id, event_kind::PROCESS_PARKED), 0);

    // A repeated launch is refused with the same reason and asks nothing new.
    let refused = api
        .task_run(RunReq {
            id: id.clone(),
            agent: "mock".into(),
            workspace: None,
            force_reaudit: false,
        })
        .unwrap();
    assert_eq!(refused.outcome, RunLaunch::Refused);
    assert!(
        refused
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("5 из 5"),
        "the plain launch speaks the status numbers: {:?}",
        refused.reason
    );
    assert_eq!(api.task_status(&id).unwrap().open_questions.len(), 1);
    assert_eq!(api.exec_list(Some(&id)).unwrap().len(), 5);
}

#[test]
fn four_attempts_are_admitted_and_a_human_gate_carries_no_counter() {
    let (dir, api) = home();
    let id = task(&api);
    seed(&api, dir.path(), &id, 4);
    let before = api.task_status(&id).unwrap().attempt_limit.unwrap();
    assert_eq!(before.line, "попыток без прогресса: 4 из 5");

    let out = api
        .task_run_supervised(&SupervisedRunReq {
            task_id: id.clone(),
            agent: "mock".into(),
            workspace: None,
            force_reaudit: false,
            run_id: new_id().to_string(),
            ack: dir.path().join("ack-fourth"),
        })
        .unwrap();
    let report = out.expect("the fifth launch is admitted");
    assert_eq!(report.gate, devai_runtime::Gate::WaitingApproval);
    let status = api.task_status(&id).unwrap();
    assert_eq!(status.task.state, TaskState::Approval);
    assert!(
        status.attempt_limit.is_none(),
        "APPROVAL is executed by a human — no role counter exists there"
    );
}

#[test]
fn continue_is_the_only_reset_and_it_returns_the_task_to_its_stage() {
    let (dir, api) = home();
    let id = task(&api);
    seed(&api, dir.path(), &id, 5);
    assert!(supervised(&api, dir.path(), &id).is_some());
    let qid = api.task_status(&id).unwrap().open_questions[0].id;

    let answered = api.question_answer(&qid.to_string(), "продолжить").unwrap();
    assert_eq!(answered.from_state, TaskState::WaitingForInput);
    assert_eq!(answered.to_state, TaskState::Analysis);

    let status = api.task_status(&id).unwrap();
    assert_eq!(status.task.state, TaskState::Analysis);
    assert_eq!(status.open_questions.len(), 0);
    let view = status.attempt_limit.unwrap();
    assert_eq!(view.attempts, 0, "the human reset the counter");
    assert_eq!(view.line, "попыток без прогресса: 0 из 5");

    assert_eq!(event_count(&api, &id, event_kind::ATTEMPT_LIMIT_RESET), 1);
    let decision = api
        .task_decisions(&id)
        .unwrap()
        .into_iter()
        .find(|d| d.kind == decision_kind::ATTEMPT_LIMIT_ANSWER)
        .expect("the answer is a stored decision");
    assert_eq!(decision.actor, "attempt-tester");
    assert_eq!(decision.payload["answer"], "продолжить");
    assert_eq!(decision.payload["attempt"], 5);
    assert_eq!(decision.payload["limit"], 5);
    assert_eq!(decision.refs, vec![qid.to_string()]);
    let q = api
        .task_status(&id)
        .unwrap()
        .open_questions
        .into_iter()
        .next();
    assert!(q.is_none(), "the answered question no longer blocks");
}

#[test]
fn stop_keeps_the_task_in_human_custody_without_cancelling_it() {
    let (dir, api) = home();
    let id = task(&api);
    seed(&api, dir.path(), &id, 5);
    assert!(supervised(&api, dir.path(), &id).is_some());
    let qid = api.task_status(&id).unwrap().open_questions[0].id;

    let answered = api.question_answer(&qid.to_string(), "остановить").unwrap();
    assert_eq!(answered.to_state, TaskState::Blocked);
    let status = api.task_status(&id).unwrap();
    assert_eq!(status.task.state, TaskState::Blocked);
    assert!(
        !status.task.state.is_terminal(),
        "a stop is neither a cancel nor a failure"
    );
    assert_eq!(status.task.return_state, Some(TaskState::Analysis));
    // No reset: the counter still reads five of five.
    let view = status.attempt_limit.unwrap();
    assert_eq!(view.attempts, 5);
    assert_eq!(view.line, "попыток без прогресса: 5 из 5");
    assert_eq!(event_count(&api, &id, event_kind::ATTEMPT_LIMIT_RESET), 0);
    assert_eq!(event_count(&api, &id, event_kind::ATTEMPT_LIMIT_STOPPED), 1);
    let decision = api
        .task_decisions(&id)
        .unwrap()
        .into_iter()
        .find(|d| d.kind == decision_kind::ATTEMPT_LIMIT_ANSWER)
        .expect("the answer is a stored decision");
    assert_eq!(decision.payload["answer"], "остановить");

    // The same question cannot be answered twice, and a plain launch from the
    // held state starts nothing (no new rows, no second question).
    let err = api
        .question_answer(&qid.to_string(), "остановить")
        .unwrap_err();
    assert!(err.to_string().contains("already answered"), "{err}");
    let ack = supervised(&api, dir.path(), &id).expect("a held task launches nothing");
    assert!(ack.starts_with("refused "), "{ack:?}");
    let refused = api
        .task_run(RunReq {
            id: id.clone(),
            agent: "mock".into(),
            workspace: None,
            force_reaudit: false,
        })
        .unwrap();
    assert_eq!(refused.outcome, RunLaunch::Refused);
    assert!(
        refused
            .reason
            .as_deref()
            .unwrap_or_default()
            .contains("5 из 5"),
        "the stop speaks the same numbers: {:?}",
        refused.reason
    );
    assert_eq!(api.exec_list(Some(&id)).unwrap().len(), 5);
    assert_eq!(
        api.task_status(&id).unwrap().open_questions.len(),
        0,
        "no second question after the answer"
    );
}

#[test]
fn an_answer_outside_the_stored_options_is_refused_and_the_question_stays_open() {
    let (dir, api) = home();
    let id = task(&api);
    seed(&api, dir.path(), &id, 5);
    assert!(supervised(&api, dir.path(), &id).is_some());
    let qid = api.task_status(&id).unwrap().open_questions[0].id;

    let err = api
        .question_answer(&qid.to_string(), "давай ещё")
        .unwrap_err();
    assert!(
        err.to_string().contains("продолжить") && err.to_string().contains("остановить"),
        "the refusal names the stored options: {err}"
    );
    let status = api.task_status(&id).unwrap();
    assert_eq!(status.task.state, TaskState::WaitingForInput);
    assert_eq!(status.open_questions.len(), 1);
    assert_eq!(status.attempt_limit.unwrap().attempts, 5);
}

#[test]
fn failed_launches_spend_no_attempt() {
    let (dir, api) = home();
    let id = task(&api);
    seed(&api, dir.path(), &id, 4);
    // Two FAILED executions of the same role: the role ran, the launch did
    // not produce an attempt (no progress, but no launch either).
    seed_failed(&api, dir.path(), &id, 2);
    let view = api.task_status(&id).unwrap().attempt_limit.unwrap();
    assert_eq!(view.attempts, 4, "failed rows are not attempts");

    // The fifth completed launch is therefore still admitted.
    let out = api
        .task_run_supervised(&SupervisedRunReq {
            task_id: id.clone(),
            agent: "mock".into(),
            workspace: None,
            force_reaudit: false,
            run_id: new_id().to_string(),
            ack: dir.path().join("ack-after-failures"),
        })
        .unwrap();
    assert!(
        out.is_some(),
        "four attempts plus failed rows stay below five"
    );
}

#[test]
fn zero_means_no_limit_and_a_broken_value_is_a_loud_declaration_error() {
    let (dir, api) = home();
    let path = dir.path().join("config/workflow/process.yaml");
    let text = std::fs::read_to_string(&path).unwrap();

    // 0 = «без предела»: six attempts do not stop anything.
    std::fs::write(
        &path,
        text.replace(
            "max_attempts_without_progress: 5",
            "max_attempts_without_progress: 0",
        ),
    )
    .unwrap();
    let id = task(&api);
    seed(&api, dir.path(), &id, 6);
    let view = api.task_status(&id).unwrap().attempt_limit.unwrap();
    assert_eq!(view.limit, None);
    assert_eq!(view.line, "попыток без прогресса: 6 — без предела");
    assert_eq!(
        api.task_status(&id).unwrap().task.state,
        TaskState::Analysis
    );

    // A negative value is refused loudly, naming the key.
    std::fs::write(
        &path,
        text.replace(
            "max_attempts_without_progress: 5",
            "max_attempts_without_progress: -1",
        ),
    )
    .unwrap();
    let err = api.task_status(&id).unwrap_err().to_string();
    assert!(
        err.contains("max_attempts_without_progress"),
        "the refusal names the key: {err}"
    );

    // A non-numeric value is just as loud.
    std::fs::write(
        &path,
        text.replace(
            "max_attempts_without_progress: 5",
            "max_attempts_without_progress: пять",
        ),
    )
    .unwrap();
    let err = api.task_status(&id).unwrap_err().to_string();
    assert!(
        err.contains("max_attempts_without_progress"),
        "the refusal names the key: {err}"
    );
}
