//! Agent-failure surfaces of the published API (task 01a1b60c,
//! REQ-AF-05/07/08): the progress line below the threshold, the derived
//! agent status row, and the wait facts of a task parked by the failure
//! threshold. No surface stores its own status — all three fold the same
//! stored failure rows.

use chrono::Utc;
use devai_api::Api;
use devai_core::types::*;
use devai_core::workflow;
use devai_store::Store;

struct Home {
    dir: tempfile::TempDir,
    store: Store,
}

impl Home {
    fn new() -> Self {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        Home { dir, store }
    }
    fn api(&self) -> Api {
        Api::open(self.dir.path(), "tester").unwrap()
    }
    fn task(&self, state: TaskState) -> Task {
        let mut t = Task::new("отказ агента", "видимый отказ", None, Utc::now());
        t.state = state;
        self.store.create_task(&t).unwrap();
        self.store
            .put_agent(&AgentInstance {
                id: new_id(),
                name: "hermes".into(),
                provider: "hermes".into(),
                model: "deepseek-v4".into(),
                runtime: "deepseek".into(),
                reasoning: "high".into(),
                created_at: Utc::now(),
                active: false,
            })
            .unwrap();
        t
    }
    fn fail(&self, t: &Task, at: Timestamp) -> AgentFailureRecorded {
        self.store
            .record_agent_failure(&AgentFailureRecord {
                id: new_id(),
                task_id: t.id,
                execution_id: Some(new_id()),
                agent_name: "hermes".into(),
                driver: "hermes".into(),
                llm_provider: "deepseek".into(),
                model: "deepseek-v4".into(),
                class: "provider_model".into(),
                exit_code: Some(2),
                duration_ms: Some(900),
                tail: "404 Model not exist".into(),
                output_empty: false,
                driver_log_tail: Some("журнал: 404 Model not exist".into()),
                driver_log_note: None,
                message: "`hermes` exited with exit status: 2: 404 Model not exist".into(),
                basis: "text:model not exist".into(),
                threshold: 3,
                window_minutes: 10,
                at,
            })
            .unwrap()
    }
}

/// REQ-AF-05 AC3 + REQ-AF-07: two failures stay below the threshold — the
/// task is not parked, progress carries the line, and the agent status row
/// names the class and the count in words.
#[test]
fn below_threshold_is_a_progress_line_and_a_status_row() {
    let h = Home::new();
    let t = h.task(TaskState::Analysis);
    let now = Utc::now();
    h.fail(&t, now);
    h.fail(&t, now + chrono::Duration::minutes(1));
    let api = h.api();

    let progress = api.task_progress(&t.id.to_string()).unwrap();
    let f = progress.agent_failure.expect("the failure is visible");
    assert_eq!(f.class, "provider_model");
    assert_eq!(f.class_word, "модель исчезла у провайдера");
    assert_eq!(f.series_count, 2);
    assert_eq!(f.threshold, 3);
    assert!(!f.parked, "below the threshold nothing is parked");
    assert_eq!(progress.task.state, TaskState::Analysis);
    assert!(!f.tail.is_empty());
    assert!(f.driver_log_tail.is_some());

    let status = api.agent_status_list().unwrap();
    let hermes = status.iter().find(|s| s.name == "hermes").unwrap();
    assert_eq!(hermes.availability, "unavailable");
    assert_eq!(hermes.last_class.as_deref(), Some("provider_model"));
    assert_eq!(
        hermes.last_class_word, "модель исчезла у провайдера",
        "the row speaks words, not the raw token"
    );
    assert_eq!(hermes.failures, 2, "suppressed repeats are counted");
    assert_eq!(hermes.series, 2);
    assert!(hermes.line.contains("недоступен"), "{}", hermes.line);
    assert!(hermes.line.contains("модель исчезла"), "{}", hermes.line);
    assert!(hermes.line.contains("2 отказов"), "{}", hermes.line);
    assert!(hermes.line.contains(':'), "the line always carries a time");
}

/// REQ-AF-08: at the threshold the task parks; the wait facts present the
/// agent, the class, the count and the blocking question. After the answer
/// the task leaves the parked state but the series stays (REQ-AF-05 AC6).
#[test]
fn parked_task_is_presented_with_the_question() {
    let h = Home::new();
    let t = h.task(TaskState::Analysis);
    let now = Utc::now();
    h.fail(&t, now);
    h.fail(&t, now + chrono::Duration::minutes(1));
    let third = h.fail(&t, now + chrono::Duration::minutes(2));
    assert_eq!(third.series_count, 3);

    let current = h.store.get_task(&t.id.to_string()).unwrap();
    let qid = new_id();
    let m = workflow::park_agent_failure(
        devai_core::process::Process::factory(),
        &current,
        qid,
        workflow::AgentFailureParking {
            agent: "hermes",
            class: "provider_model",
            series_count: third.series_count,
            threshold: 3,
            first_at: third.first_at,
            last_at: third.last_at,
            tail: "404 Model not exist",
            action: devai_core::agent_failure::action("provider_model"),
            execution_id: None,
        },
        "tester",
        Utc::now(),
    )
    .unwrap();
    h.store.commit(current.lock_version, &m).unwrap();
    h.store
        .mark_agent_failure_parked(t.id, "hermes", Utc::now())
        .unwrap();

    let api = h.api();
    let snap = api.inbox_snapshot().unwrap();
    let facts = snap.tasks.iter().find(|f| f.task.id == t.id).unwrap();
    assert_eq!(facts.task.state, TaskState::WaitingForInput);
    let failure = facts
        .agent_failure
        .as_ref()
        .expect("the wait row carries the failure");
    assert!(failure.parked);
    assert_eq!(failure.agent, "hermes");
    assert_eq!(failure.class_word, "модель исчезла у провайдера");
    assert_eq!(failure.series_count, 3);
    assert_eq!(failure.threshold, 3);
    assert!(
        facts
            .questions
            .iter()
            .any(|q| q.id == qid && q.blocking && q.text.contains("Что случилось")),
        "the blocking question is stored with the row"
    );

    // Answering restores the stage; the series is NOT reset by the answer.
    let parked = h.store.get_task(&t.id.to_string()).unwrap();
    let answered = workflow::answer_question(
        devai_core::process::Process::factory(),
        &parked,
        qid,
        "tester",
        Utc::now(),
    )
    .unwrap();
    h.store.commit(parked.lock_version, &answered).unwrap();
    let back = api.task_progress(&t.id.to_string()).unwrap();
    assert_eq!(back.task.state, TaskState::Analysis);
    assert_eq!(back.agent_failure.as_ref().unwrap().series_count, 3);
    assert!(
        !back.agent_failure.as_ref().unwrap().parked,
        "after the answer the task is not parked"
    );
}

/// REQ-AF-07 AC2/AC5: an agent with no outcomes is «неизвестно», and a
/// failure that carries no recorded class says so plainly.
#[test]
fn unknown_agents_and_unclassified_failures_are_honest() {
    let h = Home::new();
    h.store
        .put_agent(&AgentInstance {
            id: new_id(),
            name: "fresh".into(),
            provider: "claude".into(),
            model: String::new(),
            runtime: String::new(),
            reasoning: String::new(),
            created_at: Utc::now(),
            active: false,
        })
        .unwrap();
    let api = h.api();
    let status = api.agent_status_list().unwrap();
    let fresh = status.iter().find(|s| s.name == "fresh").unwrap();
    assert_eq!(fresh.availability, "unknown");
    assert!(fresh.line.contains("неизвестно"));
    assert_eq!(fresh.last_class, None);
    assert_eq!(fresh.last_class_word, "причина не классифицирована");
}
