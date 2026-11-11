//! WHY a task stands still, read out of its history (task 01a1ca39,
//! REQ-PK-01/02/04/06): the stop facts of the status response and the failed
//! launch that did not park anything — both folded from the append-only log
//! and worded from `config/workflow/park-reasons.yaml`.
//!
//! The history is written exactly as the producers write it (the same
//! `workflow::park*` mutations and the same store doors the brakes use), and
//! the assertions judge the PUBLISHED answer — the read the CLI and the window
//! consume — never an internal field.

use std::path::Path;
use std::sync::Mutex;

use chrono::Utc;
use devai_api::{Api, RunReq, TaskCreateReq};
use devai_core::process::Process;
use devai_core::types::*;
use devai_core::workflow;
use devai_store::Store;
use tempfile::TempDir;

static ENV_LOCK: Mutex<()> = Mutex::new(());

/// `DEVAI_RUNNER` is process-global: serialise the writers and put the found
/// value back on drop (including on panic).
struct RunnerEnv {
    _lock: std::sync::MutexGuard<'static, ()>,
    previous: Option<std::ffi::OsString>,
}

impl RunnerEnv {
    fn set(path: &Path) -> Self {
        let lock = ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let previous = std::env::var_os("DEVAI_RUNNER");
        // SAFETY: `ENV_LOCK` serialises every reader/writer in this binary.
        unsafe { std::env::set_var("DEVAI_RUNNER", path) }
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for RunnerEnv {
    fn drop(&mut self) {
        match self.previous.take() {
            Some(v) => unsafe { std::env::set_var("DEVAI_RUNNER", v) },
            None => unsafe { std::env::remove_var("DEVAI_RUNNER") },
        }
    }
}

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

/// REQ-PK-01/06: a task parked by the ROUNDS brake names that brake, its
/// counter pair and the stored reason verbatim — written through the same
/// `workflow::park` mutation and the same `process.parked` event the brake
/// emits.
#[test]
fn rounds_brake_park_is_presented_from_the_history() {
    let (dir, api) = home();
    let id = task(&api, "тормоз кругов");
    api.task_start(&id).unwrap();
    let store = Store::open(dir.path()).unwrap();
    let t = store.get_task(&id).unwrap();
    let now = Utc::now();
    let reason = "brake: 3 round(s) of `implementation-agent` without a new artifact (limit 3)";
    let m = workflow::park(Process::factory(), &t, Some(reason), "platform", now).unwrap();
    store.commit(t.lock_version, &m).unwrap();
    store
        .insert_event(
            Some(t.id),
            event_kind::PROCESS_PARKED,
            "platform",
            serde_json::json!({
                "brake": "rounds_without_shift",
                "counter": 3,
                "limit": 3,
                "reason": reason,
            }),
        )
        .unwrap();

    let v = api.task_status(&id).unwrap();
    let s = v.stop.expect("the stop is visible");
    assert_eq!(s.kind, "rounds_without_shift");
    assert_eq!(s.counter, Some(3));
    assert_eq!(s.limit, Some(3));
    assert_eq!(s.counter_text, "3 из 3");
    assert!(s.why.contains("тормоз кругов"), "{}", s.why);
    assert!(!s.action.trim().is_empty());
    assert_eq!(s.actor_word, "платформа");
    assert_eq!(
        s.reason.as_deref(),
        Some(reason),
        "the stored reason is never shortened or retold"
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
    assert!(!f.parked, "this failure did not park the task");
    assert!(!f.continues, "the task is held, not running");
}

/// REQ-PK-02 AC1/AC3: a launch that did not happen is visible WITH its class,
/// time, verbatim reason and action while the task keeps going — and no stop
/// block is invented for a task that is not held (REQ-PK-01 AC2).
#[test]
fn a_failed_launch_is_visible_without_parking_the_task() {
    let (dir, api) = home();
    let id = task(&api, "запуск не состоялся");
    api.task_start(&id).unwrap();
    let _env = RunnerEnv::set(&dir.path().join("no-such-runner"));
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
    assert_eq!(f.consecutive, 1);
    assert_eq!(f.limit, 3);
    assert!(!f.parked);
    assert!(f.continues, "the task keeps going; only the launch failed");
    let reason = f.reason.expect("the stored reason verbatim");
    assert!(!reason.is_empty());

    // A later failed launch grows the counter in the history (same runner:
    // one env guard for the whole test — the lock is not re-entrant).
    let _ = api.task_run(RunReq {
        id: id.clone(),
        agent: "mock".into(),
        workspace: None,
        force_reaudit: false,
    });
    let second = api.task_status(&id).unwrap().launch_failure.unwrap();
    assert_eq!(second.consecutive, 2, "the counter grows in the history");
}

/// REQ-PK-02 AC2 + REQ-PK-06: when the launch threshold parks the task, BOTH
/// facts are present and their counters agree — the stop is the launch brake,
/// not an agent failure.
#[test]
fn the_parked_launch_failure_carries_both_facts_with_matching_counters() {
    let (dir, api) = home();
    let id = task(&api, "порог запусков");
    api.task_start(&id).unwrap();
    let _env = RunnerEnv::set(&dir.path().join("no-such-runner"));
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
    assert!(f.parked, "this failure parked the task");
    assert!(!f.continues);
    assert_eq!(f.consecutive, 3);
    assert_eq!(f.limit, 3);
    let s = v.stop.expect("the stop is a fact too");
    assert_eq!(s.kind, "consecutive_launch_failures");
    assert_eq!(
        s.counter,
        Some(f.consecutive),
        "the counters never disagree"
    );
    assert_eq!(s.limit, Some(f.limit));
    assert_eq!(s.counter_text, "3 из 3");
    assert!(s.why.contains("тормоз запусков"), "{}", s.why);
    assert!(s.failure_class.is_none(), "not an agent failure");
}
