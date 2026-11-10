//! Parking on the agent-failure threshold (task 01a1b60c, REQ-AF-05).
//!
//! The parking command is pure workflow: it must move the task to
//! WAITING_FOR_INPUT on a BLOCKING question in the accepted human form
//! («что случилось / почему важно / что сделать»), remember the stage for
//! restoration, and carry the agent, class, counts, time span, tail and the
//! class action — without inventing a gate verdict or a repair promise.

use chrono::Utc;
use devai_core::agent_failure::{class, word};
use devai_core::types::*;
use devai_core::workflow;

fn task() -> Task {
    let mut t = Task::new("отказ агента", "проверить парковку", None, Utc::now());
    t.state = TaskState::Analysis;
    t
}

#[test]
fn the_threshold_parks_with_the_accepted_question_form() {
    let t = task();
    let now = Utc::now();
    let question = new_id();
    let m = workflow::park_agent_failure(
        &t,
        question,
        workflow::AgentFailureParking {
            agent: "hermes",
            class: class::PROVIDER_MODEL,
            series_count: 3,
            threshold: 3,
            first_at: now - chrono::Duration::minutes(12),
            last_at: now,
            tail: "404 Model not exist",
            action: devai_core::agent_failure::action(class::PROVIDER_MODEL),
            execution_id: None,
        },
        "tester",
        now,
    )
    .unwrap();

    assert_eq!(m.task.state, TaskState::WaitingForInput);
    assert_eq!(
        m.task.return_state,
        Some(TaskState::Analysis),
        "the stage is remembered for restoration on answer"
    );
    let q = m.questions.iter().find(|q| q.id == question).unwrap();
    assert!(q.blocking, "the parking question blocks");
    for marker in ["Что случилось", "Почему важно", "Что сделать"]
    {
        assert!(q.text.contains(marker), "missing `{marker}` in: {}", q.text);
    }
    assert!(q.text.contains("hermes"));
    assert!(q.text.contains(word(class::PROVIDER_MODEL)));
    assert!(q.text.contains("3 раз подряд"));
    assert!(q.text.contains("порог 3"));
    assert!(q.text.contains("404 Model not exist"));
    assert!(q.text.contains("перевыпустить агента"));
    let ctx = q.context.as_deref().unwrap();
    assert!(ctx.contains("gate=agent-failure"));
    assert!(ctx.contains("class=provider_model"));
    // Not a verdict about the work, no automatic switch promised.
    assert!(!q.text.to_lowercase().contains("автоматическ"));
    assert_eq!(
        m.events
            .iter()
            .filter(|e| e.kind == event_kind::QUESTION_RAISED)
            .count(),
        1
    );
}

/// A task already parked is not a legal target for a second park: the
/// command refuses instead of silently re-parking (the engine checks the
/// hold state before calling it).
#[test]
fn an_already_held_task_is_refused() {
    let mut t = task();
    t.state = TaskState::WaitingForInput;
    t.return_state = Some(TaskState::Analysis);
    let err = workflow::park_agent_failure(
        &t,
        new_id(),
        workflow::AgentFailureParking {
            agent: "hermes",
            class: class::NETWORK,
            series_count: 3,
            threshold: 3,
            first_at: Utc::now(),
            last_at: Utc::now(),
            tail: "",
            action: "проверить сеть",
            execution_id: None,
        },
        "tester",
        Utc::now(),
    );
    assert!(err.is_err());
}
