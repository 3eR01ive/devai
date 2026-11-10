//! Agent failures as stored facts (task 01a1b60c, REQ-AF-01/05/06).
//!
//! These are the data-layer pins the higher surfaces rest on: one event per
//! (agent, class) anti-spam window, a growing window counter for suppressed
//! repeats, a SEPARATE consecutive series that only a success resets, the
//! accumulated total including suppressed repeats, and the window boundary
//! surviving a restart (the counters are rows, not process state).

use chrono::{Duration, Utc};
use devai_core::types::*;
use devai_store::Store;

fn open() -> (tempfile::TempDir, Store) {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Store::open(dir.path()).unwrap();
    (dir, store)
}

fn task(store: &Store) -> Task {
    let t = Task::new("отказы агента", "проверить счётчики", None, Utc::now());
    store.create_task(&t).unwrap();
    t
}

#[allow(clippy::too_many_arguments)]
fn record(
    store: &Store,
    t: &Task,
    at: Timestamp,
    class: &str,
    window_minutes: i64,
    threshold: i64,
) -> AgentFailureRecorded {
    store
        .record_agent_failure(&AgentFailureRecord {
            id: new_id(),
            task_id: t.id,
            execution_id: Some(new_id()),
            agent_name: "hermes".into(),
            driver: "hermes".into(),
            llm_provider: "deepseek".into(),
            model: "deepseek-v4".into(),
            class: class.into(),
            exit_code: Some(2),
            duration_ms: Some(120),
            tail: "404 Model not exist".into(),
            output_empty: false,
            driver_log_tail: Some("журнал: 404".into()),
            driver_log_note: None,
            message: "`hermes` exited with exit status: 2: 404 Model not exist".into(),
            basis: "text:model not exist".into(),
            threshold,
            window_minutes,
            at,
        })
        .unwrap()
}

/// REQ-AF-06 AC1/AC2/AC3: within the window the first failure writes ONE
/// event, the rest only grow the window counter; the series and the total
/// are separate numbers and both count every actual failure (AC6).
#[test]
fn one_event_per_window_and_suppressed_repeats_still_count() {
    let (_d, store) = open();
    let t = task(&store);
    let t0 = Utc::now();

    let a = record(&store, &t, t0, "provider_model", 10, 3);
    let b = record(
        &store,
        &t,
        t0 + Duration::minutes(1),
        "provider_model",
        10,
        3,
    );
    let c = record(
        &store,
        &t,
        t0 + Duration::minutes(2),
        "provider_model",
        10,
        3,
    );

    assert!(
        a.event_written,
        "the first failure of the class opens the window"
    );
    assert!(
        !b.event_written && !c.event_written,
        "repeats are suppressed"
    );
    assert_eq!(a.window_count, 1);
    assert_eq!(b.window_count, 2);
    assert_eq!(c.window_count, 3);
    // The SERIES counts all three actual failures; the window counter and
    // the total are separate numbers (REQ-AF-06 AC3/AC6).
    assert_eq!(c.series_count, 3);
    assert_eq!(c.total_failures, 3);

    let events = store.events(t.id, None).unwrap();
    let failed: Vec<_> = events
        .iter()
        .filter(|e| e.kind == event_kind::AGENT_FAILED)
        .collect();
    assert_eq!(failed.len(), 1, "one agent.failed event for the window");
    let p = &failed[0].payload;
    assert_eq!(p["class"], "provider_model");
    assert_eq!(p["failure_class"], failure_class::PROVIDER);
    assert_eq!(p["agent"], "hermes");
    assert_eq!(p["driver"], "hermes");
    assert_eq!(p["llm_provider"], "deepseek");
    assert_eq!(p["model"], "deepseek-v4");
    assert_eq!(p["exit_code"], 2);
    assert_eq!(p["output_empty"], false);
    assert_eq!(p["window_count"], 1);
    assert_eq!(p["series_count"], 1);
    assert_eq!(p["basis"], "text:model not exist");
    assert!(p["at"].is_string() && p["first_at"].is_string());
}

/// REQ-AF-06 AC4: past the window the next failure writes a NEW event; the
/// previous window row is kept untouched.
#[test]
fn after_the_window_a_new_event_is_written() {
    let (_d, store) = open();
    let t = task(&store);
    let t0 = Utc::now();

    record(&store, &t, t0, "provider_model", 10, 3);
    let late = record(
        &store,
        &t,
        t0 + Duration::minutes(11),
        "provider_model",
        10,
        3,
    );
    assert!(late.event_written, "a new window opens after the boundary");
    assert_eq!(late.window_count, 1);
    assert_eq!(
        late.series_count, 2,
        "the series is not reset by the window"
    );
    assert_eq!(late.total_failures, 2);
    assert_eq!(
        store
            .events(t.id, None)
            .unwrap()
            .iter()
            .filter(|e| e.kind == event_kind::AGENT_FAILED)
            .count(),
        2
    );
}

/// REQ-AF-06 AC5: different classes (and agents) never silence each other.
#[test]
fn classes_and_agents_do_not_suppress_each_other() {
    let (_d, store) = open();
    let t = task(&store);
    let t0 = Utc::now();
    let a = record(&store, &t, t0, "provider_model", 10, 3);
    let b = record(&store, &t, t0, "network", 10, 3);
    assert!(a.event_written && b.event_written);
    assert_eq!(a.series_count, 1);
    assert_eq!(
        b.series_count, 2,
        "the series is per (task, agent), across classes"
    );

    let other = Task::new("другая", "задача", None, Utc::now());
    store.create_task(&other).unwrap();
    let c = record(&store, &other, t0, "provider_model", 10, 3);
    assert!(
        !c.event_written,
        "the anti-spam window is per (agent, class), not per task"
    );
    assert_eq!(c.series_count, 1, "the series is per task");
    assert_eq!(
        c.window_count, 1,
        "the evidence row is per task — it does not inherit the other task's counter"
    );
    assert_eq!(
        store.agent_failure_total("hermes").unwrap(),
        3,
        "the accumulated count sums every actual failure: two classes on the \
         first task plus the second task's suppressed repeat"
    );
    assert!(
        store.latest_agent_failure(other.id).unwrap().is_some(),
        "the second task keeps its OWN evidence row (progress line), \
         although the window suppressed its event"
    );
}

/// REQ-AF-05 AC2/AC6: only a SUCCESS resets the series; the failure history
/// and the accumulated total stay.
#[test]
fn success_resets_the_series_but_not_history() {
    let (_d, store) = open();
    let t = task(&store);
    let t0 = Utc::now();
    record(&store, &t, t0, "provider_model", 10, 3);
    let before = record(
        &store,
        &t,
        t0 + Duration::minutes(1),
        "provider_model",
        10,
        3,
    );
    assert_eq!(before.series_count, 2);

    store.reset_agent_failure_series("hermes").unwrap();
    assert!(
        store
            .agent_failure_series(t.id, "hermes")
            .unwrap()
            .is_none()
    );
    assert_eq!(
        store.agent_failure_total("hermes").unwrap(),
        2,
        "the history and the total survive the reset"
    );

    let after = record(
        &store,
        &t,
        t0 + Duration::minutes(2),
        "provider_model",
        10,
        3,
    );
    assert_eq!(after.series_count, 1, "a fresh series starts at one");
    assert_eq!(after.total_failures, 3);
}

/// The latest-failure reads carry the evidence the surfaces present, and the
/// journal absence is an explicit note rather than an empty field.
#[test]
fn the_latest_failure_carries_the_observable_evidence() {
    let (_d, store) = open();
    let t = task(&store);
    let t0 = Utc::now();
    store
        .record_agent_failure(&AgentFailureRecord {
            id: new_id(),
            task_id: t.id,
            execution_id: None,
            agent_name: "hermes".into(),
            driver: "hermes".into(),
            llm_provider: "deepseek".into(),
            model: "m".into(),
            class: "unclassified".into(),
            exit_code: Some(2),
            duration_ms: Some(5),
            tail: String::new(),
            output_empty: true,
            driver_log_tail: None,
            driver_log_note: Some("журнал драйвера не найден".into()),
            message: "`hermes` exited with exit status: 2: ".into(),
            basis: "no signals".into(),
            threshold: 3,
            window_minutes: 10,
            at: t0,
        })
        .unwrap();
    let row = store.latest_agent_failure(t.id).unwrap().unwrap();
    assert_eq!(row.class, "unclassified");
    assert!(row.output_empty);
    assert!(row.tail.is_empty(), "the empty-output tail stays empty");
    assert_eq!(
        row.driver_log_note.as_deref(),
        Some("журнал драйвера не найден")
    );
    assert_eq!(
        store
            .latest_agent_failure_for_agent("hermes")
            .unwrap()
            .unwrap()
            .id,
        row.id
    );
}
