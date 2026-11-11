//! WHY a task stands still, read out of its history (task 01a1ca39,
//! REQ-PK-01/02/04/06): the stop facts of the status response and the failed
//! launch that did not park anything — both folded from the append-only log
//! and worded from `config/workflow/park-reasons.yaml`.
//!
//! The history is written exactly as the producers write it (the same
//! `workflow::park*` mutations and the same store doors the brakes use), and
//! the assertions judge the PUBLISHED answer — the read the CLI and the window
//! consume — never an internal field.
//!
//! The runner a launch hands the role to is the handle's OWN (`with_runner`),
//! never the process-global `DEVAI_RUNNER`: these tests do not order
//! themselves against each other.

use chrono::Utc;
use devai_api::{Api, AskReq, RunReq, SupervisedRunReq, TaskCreateReq};
use devai_core::process::Process;
use devai_core::types::*;
use devai_core::workflow;
use devai_store::Store;
use tempfile::TempDir;

fn home() -> (TempDir, Api) {
    let dir = TempDir::new().unwrap();
    let api = Api::open(dir.path(), "pk-tester").unwrap();
    api.seed_config(None).unwrap();
    (dir, api)
}

fn task(api: &Api, title: &str) -> String {
    api.task_create(TaskCreateReq {
        title: title.into(),
        project_id: None,
        description: "остановка видна в данных".into(),
        parent: Vec::new(),
        blocked_by: Vec::new(),
        idempotency_key: None,
    })
    .unwrap()
    .task
    .id
    .to_string()
}

/// A real agent-failure park: the same mutation the engine commits at the
/// declared threshold, with the blocking question in the same transaction.
fn park_by_agent_failure(store: &Store, api: &Api, id: &str) -> String {
    let t = store.get_task(id).unwrap();
    let qid = new_id();
    let now = Utc::now();
    let m = workflow::park_agent_failure(
        Process::factory(),
        &t,
        qid,
        workflow::AgentFailureParking {
            agent: "hermes",
            class: "provider_model",
            series_count: 3,
            threshold: 3,
            first_at: now - chrono::Duration::minutes(2),
            last_at: now,
            tail: "404 Model not exist",
            action: devai_core::agent_failure::action("provider_model"),
            execution_id: None,
        },
        "platform",
        now,
    )
    .unwrap();
    store.commit(t.lock_version, &m).unwrap();
    api.task_status(id)
        .unwrap()
        .open_questions
        .iter()
        .find(|q| q.id == qid)
        .map(|q| q.text.clone())
        .expect("the parking question is stored")
}

/// REQ-PK-01/06: the parking by the agent-failure threshold is worded as an
/// AGENT failure (never as a launch failure), carries `3 из 3`, the time, the
/// platform as the actor, the class word from the declaration and the stored
/// question text VERBATIM.
#[test]
fn agent_failure_park_is_presented_with_counter_words_and_verbatim_reason() {
    let (dir, api) = home();
    let id = task(&api, "парковка по отказу агента");
    api.task_start(&id).unwrap();
    let store = Store::open(dir.path()).unwrap();
    let question_text = park_by_agent_failure(&store, &api, &id);

    let v = api.task_status(&id).unwrap();
    assert_eq!(v.task.state, TaskState::WaitingForInput);
    let s = v.stop.expect("a parked task names its stop");
    assert_eq!(s.kind, "agent_failure");
    assert_eq!(s.counter, Some(3));
    assert_eq!(s.limit, Some(3));
    assert_eq!(s.counter_text, "3 из 3");
    assert!(s.why.contains("отказ агента"), "{}", s.why);
    assert!(!s.action.trim().is_empty(), "the way out is named");
    assert_eq!(s.actor, "platform");
    assert_eq!(s.actor_word, "платформа");
    assert_eq!(s.failure_class.as_deref(), Some("provider_model"));
    assert_eq!(
        s.failure_class_word.as_deref(),
        Some("модель исчезла у провайдера"),
        "the class word is the declaration's, not the raw token"
    );
    assert_eq!(
        s.reason.as_deref(),
        Some(question_text.as_str()),
        "the stored reason is shown verbatim"
    );
    assert!(
        s.reason.as_deref().unwrap().contains("Что случилось"),
        "the question's own words ride the stop block"
    );
    // Not a launch failure — the two are never conflated (REQ-PK-06 AC3).
    assert!(v.launch_failure.is_none());

    // REQ-PK-04 AC2 and REQ-PK-07 AC2: two reads agree, and the read writes
    // nothing — neither the row version nor the event count moves.
    let events_before = store.events(v.task.id, None).unwrap().len();
    let second = api.task_status(&id).unwrap();
    let again = second.stop.expect("stable second read");
    assert_eq!(again.counter, s.counter);
    assert_eq!(again.counter_text, s.counter_text);
    assert_eq!(again.reason, s.reason);
    let after = api.task_get(&id).unwrap();
    assert_eq!(after.lock_version, v.task.lock_version, "no write on read");
    assert_eq!(after.updated_at, v.task.updated_at, "no write on read");
    assert_eq!(
        store.events(v.task.id, None).unwrap().len(),
        events_before,
        "no event is appended by a read"
    );
}

/// One execution ROW of the stage role — the input `launch_admission` counts
/// when it asks whether the rounds brake tripped. Written through the store
/// door the engine opens rows with.
fn stage_round(store: &Store, task: &Task, minutes_ago: i64) {
    let now = Utc::now();
    store
        .start_execution(&RoleExecution {
            id: new_id(),
            task_id: task.id,
            role_slug: "requirements-analyst".into(),
            role_version: 1,
            agent_name: Some("mock".into()),
            status: ExecutionStatus::Failed,
            inputs: Vec::new(),
            summary: None,
            error: Some("round without a new artifact".into()),
            started_at: now - chrono::Duration::minutes(minutes_ago),
            finished_at: Some(now),
            workspace_revision: None,
            attempt: 1,
            parent_execution: None,
            failure_class: None,
            owner_pid: None,
            owner_host: None,
            owner_process_start: None,
            bundle: None,
            run_id: None,
        })
        .unwrap();
}

/// REQ-PK-01/06 (rework R2, ts/coverage-static): a task parked by the ROUNDS
/// brake names that brake, its counter pair and the producer's stored reason
/// verbatim. The brake is DRIVEN, not simulated: `task_run_supervised` counts
/// the stage's execution rows, trips `launch_admission`'s brake, parks the
/// task and writes `process.parked` itself — a producer token that drifts
/// from the reader's vocabulary fails here.
#[test]
fn rounds_brake_park_is_presented_from_the_history() {
    let (dir, api) = home();
    let id = task(&api, "тормоз кругов");
    api.task_start(&id).unwrap();
    let store = Store::open(dir.path()).unwrap();
    let t = store.get_task(&id).unwrap();
    // Three rounds of the declared ANALYSIS role since the last artifact —
    // the declared budget — and no artifact at all.
    for n in 0..3 {
        stage_round(&store, &t, 3 - n);
    }

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
        .unwrap();
    assert!(
        report.is_none(),
        "the brake refuses the launch — nothing ran"
    );
    let ack = std::fs::read_to_string(&ack).unwrap_or_default();
    assert!(
        ack.contains("refused") && ack.contains("round(s)"),
        "the producer refuses through the ack and names the brake: {ack:?}"
    );

    let v = api.task_status(&id).unwrap();
    assert_eq!(v.task.state, TaskState::Blocked, "the brake parks the task");
    let s = v.stop.expect("the stop is visible");
    assert_eq!(s.kind, "rounds_without_shift");
    assert_eq!(s.counter, Some(3));
    assert_eq!(s.limit, Some(3));
    assert_eq!(s.counter_text, "3 из 3");
    assert!(s.why.contains("тормоз кругов"), "{}", s.why);
    assert!(!s.action.trim().is_empty());
    assert_eq!(s.actor_word, "платформа");
    assert!(
        s.reason
            .as_deref()
            .unwrap_or_default()
            .contains("round(s) of `requirements-analyst` without a new artifact"),
        "the producer's own text is shown verbatim: {:?}",
        s.reason
    );
    assert!(s.failure_class.is_none(), "a brake is not a class failure");
    assert!(v.launch_failure.is_none());
}

/// REQ-PK-06: a stop on the FIRST occurrence says «с первого раза», and a
/// HUMAN stop carries the human's name and the manual counter sentence — the
/// kinds are distinguishable in the answer itself.
#[test]
fn park_immediately_and_manual_stops_are_worded_as_themselves() {
    let (dir, api) = home();
    let store = Store::open(dir.path()).unwrap();

    // A class declared to park immediately: counter and limit are 1/1.
    let id = task(&api, "парковка сразу");
    api.task_start(&id).unwrap();
    let t = store.get_task(&id).unwrap();
    let now = Utc::now();
    let reason = "failure class `platform` is declared to park immediately — another round cannot \
                  fix it; engine error: boom";
    let m = workflow::park_on_launch_failure(Process::factory(), &t, Some(reason), "platform", now)
        .unwrap();
    store.commit(t.lock_version, &m).unwrap();
    store
        .insert_event(
            Some(t.id),
            event_kind::PROCESS_PARKED,
            "platform",
            serde_json::json!({
                "brake": "park_immediately",
                "counter": 1,
                "limit": 1,
                "reason": reason,
            }),
        )
        .unwrap();
    let s = api.task_status(&id).unwrap().stop.expect("the stop");
    assert_eq!(s.kind, "park_immediately");
    assert_eq!((s.counter, s.limit), (Some(1), Some(1)));
    assert_eq!(
        s.counter_text, "сработало с первого раза, порог 1",
        "1 из 1 must not read like an accident"
    );
    assert!(s.why.contains("паркующим сразу"), "{}", s.why);

    // A human block: the actor is the person, there is no counter, and the
    // reason is the person's own words.
    let manual = task(&api, "остановка рукой");
    api.task_start(&manual).unwrap();
    let t = store.get_task(&manual).unwrap();
    let m = workflow::block(
        Process::factory(),
        &t,
        Some("ждём ответа подрядчика"),
        "guerrero",
        Utc::now(),
    )
    .unwrap();
    store.commit(t.lock_version, &m).unwrap();
    let s = api
        .task_status(&manual)
        .unwrap()
        .stop
        .expect("the manual stop");
    assert_eq!(s.kind, "manual_block");
    assert_eq!(s.actor, "guerrero");
    assert_eq!(s.actor_word, "guerrero", "a human's name is not rewritten");
    assert_eq!((s.counter, s.limit), (None, None));
    assert_eq!(s.counter_text, "остановлено вручную, счётчика нет");
    assert_eq!(s.reason.as_deref(), Some("ждём ответа подрядчика"));
    assert!(s.why.contains("остановил человек"), "{}", s.why);
}

/// REQ-PK-03 AC3: a stop token the declaration does not word is a LOUD error
/// naming the missing value — never a machine name printed as the phrase.
#[test]
fn an_undeclared_stop_token_is_a_loud_error_naming_it() {
    let (dir, api) = home();
    let id = task(&api, "неизвестный тормоз");
    api.task_start(&id).unwrap();
    let store = Store::open(dir.path()).unwrap();
    let t = store.get_task(&id).unwrap();
    let m = workflow::park(
        Process::factory(),
        &t,
        Some("future brake tripped"),
        "platform",
        Utc::now(),
    )
    .unwrap();
    store.commit(t.lock_version, &m).unwrap();
    store
        .insert_event(
            Some(t.id),
            event_kind::PROCESS_PARKED,
            "platform",
            serde_json::json!({
                "brake": "future_brake",
                "counter": 2,
                "limit": 2,
                "reason": "future brake tripped",
            }),
        )
        .unwrap();
    let e = api.task_status(&id).unwrap_err();
    assert_eq!(e.code(), devai_api::ErrorCode("config"), "{e}");
    assert!(format!("{e}").contains("future_brake"), "{e}");
}

/// A failed launch followed by a HUMAN stop: the launch fact stays visible,
/// but it must not claim the task is running — `continues` is false while the
/// task is held, and `parked` still says this event did not stop it.
#[test]
fn a_failed_launch_does_not_claim_a_held_task_is_running() {
    let (dir, api) = home();
    let id = task(&api, "отказ запуска, потом блокировка");
    api.task_start(&id).unwrap();
    let store = Store::open(dir.path()).unwrap();
    let t = store.get_task(&id).unwrap();
    store
        .insert_event(
            Some(t.id),
            event_kind::EXECUTION_LAUNCH_FAILED,
            "platform",
            serde_json::json!({
                "failure_class": "spawn",
                "consecutive": 1,
                "limit": 3,
                "parked": false,
                "reason": "no runner",
            }),
        )
        .unwrap();
    let m = workflow::block(
        Process::factory(),
        &t,
        Some("ждём человека"),
        "guerrero",
        Utc::now(),
    )
    .unwrap();
    store.commit(t.lock_version, &m).unwrap();

    let v = api.task_status(&id).unwrap();
    assert_eq!(v.stop.unwrap().kind, "manual_block");
    let f = v.launch_failure.expect("the failure stays visible");
    assert_eq!(f.parked, Some(false), "this failure did not park the task");
    assert!(!f.continues, "the task is held, not running");
    assert_eq!(
        f.aftermath, "запуск не состоялся; остановила задачу другая причина (см. выше)",
        "the declared sentence never claims a held task is running"
    );
}

/// REQ-PK-02 AC1/AC3: a launch that did not happen is visible WITH its class,
/// time, verbatim reason and action while the task keeps going — and no stop
/// block is invented for a task that is not held (REQ-PK-01 AC2).
#[test]
fn a_failed_launch_is_visible_without_parking_the_task() {
    let (dir, api) = home();
    let id = task(&api, "запуск не состоялся");
    api.task_start(&id).unwrap();
    // The runner THIS handle launches with: a path that cannot exist — a
    // spawn-class launch failure, deterministic on every host.
    let api = api.with_runner(dir.path().join("no-such-runner"));
    let e = api
        .task_run(RunReq {
            id: id.clone(),
            agent: "mock".into(),
            workspace: None,
            force_reaudit: false,
        })
        .unwrap_err();
    assert_eq!(e.code(), devai_api::ErrorCode("launch_failed"), "{e}");

    let v = api.task_status(&id).unwrap();
    assert!(
        v.stop.is_none(),
        "the task is not stopped, so no stop block is printed"
    );
    let f = v.launch_failure.expect("the failed launch is a fact");
    assert_eq!(f.failure_class, "spawn");
    assert!(
        f.class_word.contains("породить процесс"),
        "{}",
        f.class_word
    );
    assert!(!f.action.trim().is_empty());
    assert_eq!(f.consecutive, Some(1));
    assert_eq!(f.limit, Some(3));
    assert_eq!(f.counter_text, "1 из 3");
    assert_eq!(f.parked, Some(false));
    assert!(f.continues, "the task keeps going; only the launch failed");
    assert_eq!(
        f.aftermath, "продолжает идти, запуск не состоялся",
        "the aftermath sentence is the declaration's"
    );
    let reason = f.reason.expect("the stored reason verbatim");
    assert!(!reason.is_empty());

    // A later failed launch grows the counter in the history (the same
    // handle — one runner for the whole test, no global env to restore).
    let _ = api.task_run(RunReq {
        id: id.clone(),
        agent: "mock".into(),
        workspace: None,
        force_reaudit: false,
    });
    let second = api.task_status(&id).unwrap().launch_failure.unwrap();
    assert_eq!(
        second.consecutive,
        Some(2),
        "the counter grows in the history"
    );
}

/// REQ-PK-02 AC2 + REQ-PK-06: when the launch threshold parks the task, BOTH
/// facts are present and their counters agree — the stop is the launch brake,
/// not an agent failure.
#[test]
fn the_parked_launch_failure_carries_both_facts_with_matching_counters() {
    let (dir, api) = home();
    let id = task(&api, "порог запусков");
    api.task_start(&id).unwrap();
    let api = api.with_runner(dir.path().join("no-such-runner"));
    for _ in 0..3 {
        let _ = api.task_run(RunReq {
            id: id.clone(),
            agent: "mock".into(),
            workspace: None,
            force_reaudit: false,
        });
    }
    let v = api.task_status(&id).unwrap();
    assert_eq!(v.task.state, TaskState::Blocked, "the brake parks the task");
    let f = v.launch_failure.expect("the failed launch is a fact");
    assert_eq!(f.parked, Some(true), "this failure parked the task");
    assert!(!f.continues);
    assert_eq!(f.aftermath, "запаркована этим отказом");
    assert_eq!(f.consecutive, Some(3));
    assert_eq!(f.limit, Some(3));
    let s = v.stop.expect("the stop is a fact too");
    assert_eq!(s.kind, "consecutive_launch_failures");
    assert_eq!(s.counter, f.consecutive, "the counters never disagree");
    assert_eq!(s.limit, f.limit);
    assert_eq!(s.counter_text, "3 из 3");
    assert!(s.why.contains("тормоз запусков"), "{}", s.why);
    assert!(s.failure_class.is_none(), "not an agent failure");
}

/// A plain BLOCKING question hold (raised by a role, no agent-failure gate) is
/// worded as a QUESTION: the asker's stored words verbatim, no counter, no
/// failure class — never passed off as an agent failure or a manual stop.
#[test]
fn a_plain_question_hold_is_worded_as_a_question() {
    let (_dir, api) = home();
    let id = task(&api, "вопрос ждёт ответа");
    api.task_start(&id).unwrap();
    let asked = api
        .question_ask(AskReq {
            id: id.clone(),
            text: "Подтвердить план работ?".into(),
            context: None,
            options: Vec::new(),
            blocking: true,
            execution: None,
        })
        .unwrap();

    let v = api.task_status(&id).unwrap();
    assert_eq!(v.task.state, TaskState::WaitingForInput);
    assert_eq!(asked.question.id, v.open_questions[0].id);
    let s = v.stop.expect("a question hold is a stop");
    assert_eq!(s.kind, "question");
    assert!(s.why.contains("ждёт ответа"), "{}", s.why);
    // Waiting for a human is not «остановлено вручную», and 0 из 0 is invented.
    assert_eq!((s.counter, s.limit), (None, None));
    assert_eq!(s.counter_text, "счётчика нет: задача ждёт ответа человека");
    assert_eq!(s.reason.as_deref(), Some("Подтвердить план работ?"));
    assert!(
        s.failure_class.is_none(),
        "a question is not a failure class"
    );
    assert!(v.launch_failure.is_none());
}

/// REQ-PK-02 AC3 on the OTHER end of the line: a FINISHED task is never
/// described as going. The launch failure stays a fact, `continues` is false and
/// `finished` says which kind of «not going» this is.
#[test]
fn a_finished_task_is_not_reported_as_going() {
    let (dir, api) = home();
    let id = task(&api, "отказ запуска, потом отмена");
    api.task_start(&id).unwrap();
    let store = Store::open(dir.path()).unwrap();
    let t = store.get_task(&id).unwrap();
    store
        .insert_event(
            Some(t.id),
            event_kind::EXECUTION_LAUNCH_FAILED,
            "platform",
            serde_json::json!({
                json_field::FAILURE_CLASS: "spawn",
                json_field::CONSECUTIVE: 1,
                json_field::LIMIT: 3,
                json_field::PARKED: false,
                json_field::REASON: "no runner",
            }),
        )
        .unwrap();
    let m = workflow::cancel(
        Process::factory(),
        &t,
        Some("передумали"),
        "guerrero",
        Utc::now(),
    )
    .unwrap();
    store.commit(t.lock_version, &m).unwrap();

    let v = api.task_status(&id).unwrap();
    assert_eq!(v.task.state, TaskState::Cancelled);
    assert!(v.stop.is_none(), "a finished task has no stop block");
    let f = v.launch_failure.expect("the failure stays a fact");
    assert_eq!(f.parked, Some(false));
    assert!(!f.continues, "a cancelled task is not going");
    assert!(f.finished, "the answer says the task is finished");
    assert_eq!(f.aftermath, "задача завершена, запуск не состоялся");
}

/// REQ-PK-01 AC3 and the corruption rule: a stored `question_id` that does not
/// parse is a LOUD error naming the event — a corrupt payload never renders as
/// «причина не записана».
#[test]
fn a_corrupt_question_payload_is_a_loud_error_naming_the_event() {
    let (dir, api) = home();
    let id = task(&api, "битый вопрос");
    api.task_start(&id).unwrap();
    let store = Store::open(dir.path()).unwrap();
    let t = store.get_task(&id).unwrap();
    let m = workflow::raise_question(
        Process::factory(),
        &t,
        new_id(),
        true,
        "implementation-agent",
        Utc::now(),
    )
    .unwrap();
    store.commit(t.lock_version, &m).unwrap();
    store
        .insert_event(
            Some(t.id),
            event_kind::QUESTION_RAISED,
            "implementation-agent",
            serde_json::json!({ json_field::QUESTION_ID: "not-a-uuid", "blocking": true }),
        )
        .unwrap();

    let e = api.task_status(&id).unwrap_err();
    assert_eq!(e.code(), devai_api::ErrorCode("corrupt"), "{e}");
    assert!(format!("{e}").contains("question_id"), "{e}");
}

/// The same rule for the launch fact: without its class the event cannot be
/// worded at all, so the read refuses loudly instead of inventing a class.
#[test]
fn a_failed_launch_without_a_class_is_a_loud_error() {
    let (dir, api) = home();
    let id = task(&api, "отказ без класса");
    api.task_start(&id).unwrap();
    let store = Store::open(dir.path()).unwrap();
    store
        .insert_event(
            Some(id.parse().unwrap()),
            event_kind::EXECUTION_LAUNCH_FAILED,
            "platform",
            serde_json::json!({ json_field::REASON: "boom" }),
        )
        .unwrap();

    let e = api.task_status(&id).unwrap_err();
    assert_eq!(e.code(), devai_api::ErrorCode("corrupt"), "{e}");
    assert!(format!("{e}").contains("failure_class"), "{e}");
}

/// Absence is SAID, never invented: a brake whose event recorded no counter
/// states that — not `0 из 0`.
#[test]
fn a_brake_without_a_counter_states_the_absence() {
    let (dir, api) = home();
    let store = Store::open(dir.path()).unwrap();
    let id = task(&api, "тормоз без счётчика");
    api.task_start(&id).unwrap();
    let t = store.get_task(&id).unwrap();
    let m = workflow::park(
        Process::factory(),
        &t,
        Some("parked without a counter"),
        "platform",
        Utc::now(),
    )
    .unwrap();
    store.commit(t.lock_version, &m).unwrap();
    store
        .insert_event(
            Some(t.id),
            event_kind::PROCESS_PARKED,
            "platform",
            serde_json::json!({
                json_field::BRAKE: "rounds_without_shift",
                json_field::REASON: "parked without a counter",
            }),
        )
        .unwrap();
    let s = api
        .task_status(&id)
        .unwrap()
        .stop
        .expect("the brake is the stop");
    assert_eq!((s.counter, s.limit), (None, None));
    assert_eq!(s.counter_text, "счётчик в событии не записан");
}

/// The same for the launch fact: an event that does not say whether it parked
/// says «не записано», never `parked: false`, and its missing counter is named.
#[test]
fn a_launch_failure_without_details_states_the_absence() {
    let (dir, api) = home();
    let store = Store::open(dir.path()).unwrap();
    let id = task(&api, "отказ без подробностей");
    api.task_start(&id).unwrap();
    store
        .insert_event(
            Some(id.parse().unwrap()),
            event_kind::EXECUTION_LAUNCH_FAILED,
            "platform",
            serde_json::json!({
                json_field::FAILURE_CLASS: "spawn",
                json_field::REASON: "boom",
            }),
        )
        .unwrap();
    let f = api
        .task_status(&id)
        .unwrap()
        .launch_failure
        .expect("the failed launch is a fact");
    assert_eq!(f.parked, None, "the event did not say — nothing is assumed");
    assert_eq!((f.consecutive, f.limit), (None, None));
    assert_eq!(f.counter_text, "счётчик не записан");
    assert!(f.continues, "the task itself is still going");
    assert_eq!(
        f.aftermath, "событие не записало, запарковал ли этот отказ задачу",
        "the absence of `parked` is declared words, not an assumed «no»"
    );
}

/// A payload field of the WRONG TYPE is corrupt, not absent (rework R2,
/// rl/error-handling): `counter: "3"` must not read as «счётчик не записан»,
/// and a `process.parked` that cannot name its brake must not disappear from
/// the stop block — both are named events, not silent absences.
#[test]
fn wrong_typed_stop_payloads_are_loud_not_absent() {
    let (dir, api) = home();
    let store = Store::open(dir.path()).unwrap();

    let typed = task(&api, "счётчик строкой");
    api.task_start(&typed).unwrap();
    let t = store.get_task(&typed).unwrap();
    let m = workflow::park(
        Process::factory(),
        &t,
        Some("counter as text"),
        "platform",
        Utc::now(),
    )
    .unwrap();
    store.commit(t.lock_version, &m).unwrap();
    store
        .insert_event(
            Some(t.id),
            event_kind::PROCESS_PARKED,
            "platform",
            serde_json::json!({
                json_field::BRAKE: stop_kind::ROUNDS_WITHOUT_SHIFT,
                json_field::COUNTER: "3",
                json_field::LIMIT: 3,
                json_field::REASON: "counter as text",
            }),
        )
        .unwrap();
    let e = api.task_status(&typed).unwrap_err();
    assert_eq!(e.code(), devai_api::ErrorCode("corrupt"), "{e}");
    assert!(format!("{e}").contains("counter"), "{e}");

    let nameless = task(&api, "тормоз без имени");
    api.task_start(&nameless).unwrap();
    let t = store.get_task(&nameless).unwrap();
    let m = workflow::park(
        Process::factory(),
        &t,
        Some("no brake token"),
        "platform",
        Utc::now(),
    )
    .unwrap();
    store.commit(t.lock_version, &m).unwrap();
    store
        .insert_event(
            Some(t.id),
            event_kind::PROCESS_PARKED,
            "platform",
            serde_json::json!({
                json_field::COUNTER: 3,
                json_field::LIMIT: 3,
                json_field::REASON: "no brake token",
            }),
        )
        .unwrap();
    let e = api.task_status(&nameless).unwrap_err();
    assert_eq!(e.code(), devai_api::ErrorCode("corrupt"), "{e}");
    assert!(format!("{e}").contains("brake"), "{e}");
}

/// The same rule for the launch fact: a `parked` that is written as text is a
/// corrupt payload, never «the event did not say».
#[test]
fn a_wrong_typed_parked_flag_is_a_loud_error() {
    let (_dir, api) = home();
    let id = task(&api, "parked строкой");
    api.task_start(&id).unwrap();
    let store = Store::open(_dir.path()).unwrap();
    store
        .insert_event(
            Some(id.parse().unwrap()),
            event_kind::EXECUTION_LAUNCH_FAILED,
            "platform",
            serde_json::json!({
                json_field::FAILURE_CLASS: "spawn",
                json_field::PARKED: "yes",
                json_field::REASON: "boom",
            }),
        )
        .unwrap();

    let e = api.task_status(&id).unwrap_err();
    assert_eq!(e.code(), devai_api::ErrorCode("corrupt"), "{e}");
    assert!(format!("{e}").contains("parked"), "{e}");
}
