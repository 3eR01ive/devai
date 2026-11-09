//! The QA gate's public availability and its two human acts (task 01a1bfa0,
//! REQ-QG-01/06/07/08/09) exercised on a real store home:
//!
//! * `task.status` publishes per-action availability and the platform refusal —
//!   the window does not derive either;
//! * acceptance needs a CURRENT quality report and refuses with the same typed
//!   error the CLI gets;
//! * accept/return write ONE workflow event each, with the rationale and the
//!   actor, and return additionally records the report's staleness (a saved
//!   fact, not a second act);
//! * a RUNNING quality audit withholds both acts and is not invented as a
//!   human wait.
use chrono::Utc;
use devai_api::Api;
use devai_core::types::*;
use devai_store::{ArtifactDraft, Store};

fn seeded(state: TaskState) -> (tempfile::TempDir, Task) {
    let home = tempfile::TempDir::new().unwrap();
    let store = Store::open(home.path()).unwrap();
    let mut task = Task::new("qa gate", "бизнес-описание задачи", None, Utc::now());
    task.state = state;
    store.create_task(&task).unwrap();
    (home, task)
}

fn publish_report(home: &tempfile::TempDir, task: &Task) {
    Store::open(home.path())
        .unwrap()
        .publish_artifact(ArtifactDraft {
            task_id: task.id,
            kind: ArtifactKind::QualityReport,
            name: "quality-report.json".into(),
            category: DataCategory::Artifact,
            content: r#"{"verdict":"PASS","report_text":"отчёт на подпись"}"#
                .as_bytes()
                .to_vec(),
            media_type: "application/json".into(),
            contract: None,
            created_by: "quality-auditor".into(),
            execution_id: None,
            depends_on: vec![],
        })
        .unwrap();
}

fn running_audit_row(task: &Task) -> RoleExecution {
    RoleExecution {
        id: new_id(),
        task_id: task.id,
        role_slug: "quality-auditor".into(),
        role_version: 1,
        agent_name: None,
        status: ExecutionStatus::Running,
        inputs: vec![],
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
    }
}

fn running_audit(home: &tempfile::TempDir, task: &Task) {
    Store::open(home.path())
        .unwrap()
        .start_execution(&running_audit_row(task))
        .unwrap();
}

/// REQ-QG-08: the read states BOTH acts independently — a missing report cuts
/// acceptance only, and the return stays available with its own fact.
#[test]
fn status_publishes_per_action_availability() {
    let (home, task) = seeded(TaskState::Qa);
    let api = Api::open(home.path(), "tester").unwrap();
    let status = api.task_status(&task.id.to_string()).unwrap();
    let gate = status.quality_gate.expect("QA publishes the gate");
    assert!(!gate.may_accept, "no CURRENT report: acceptance is off");
    assert_eq!(
        gate.accept_denied.as_ref().map(|d| d.code.as_str()),
        Some("no_current_report")
    );
    assert!(
        gate.may_return,
        "the return is not hidden by the missing report"
    );
    assert!(gate.return_denied.is_none());

    publish_report(&home, &task);
    let status = api.task_status(&task.id.to_string()).unwrap();
    let gate = status.quality_gate.unwrap();
    assert!(gate.may_accept && gate.may_return);
    assert!(gate.current_report.is_some());
}

/// REQ-QG-06 AC1: without a CURRENT report the acceptance refuses with the
/// platform's typed error — the same act the CLI calls.
#[test]
fn acceptance_without_a_report_is_refused() {
    let (home, task) = seeded(TaskState::Qa);
    let api = Api::open(home.path(), "tester").unwrap();
    let err = api
        .task_accept(&task.id.to_string(), Some("accept".into()))
        .unwrap_err();
    assert_eq!(err.code().to_string(), "guard");
    let after = api.task_status(&task.id.to_string()).unwrap();
    assert_eq!(after.task.state, TaskState::Qa, "nothing moved");
}

fn events_of(store: &Store, task: &Task, kind: &str) -> Vec<WorkflowEvent> {
    store
        .events(task.id, None)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == kind)
        .collect()
}

/// REQ-QG-07: the acceptance is ONE attributed event carrying the rationale.
#[test]
fn acceptance_leaves_one_attributed_event() {
    let (home, task) = seeded(TaskState::Qa);
    publish_report(&home, &task);
    let api = Api::open(home.path(), "guerrero").unwrap();
    api.task_accept(&task.id.to_string(), Some("принято человеком".into()))
        .unwrap();
    let status = api.task_status(&task.id.to_string()).unwrap();
    assert_eq!(status.task.state, TaskState::Accepted);
    assert!(status.quality_gate.is_none(), "the gate is gone outside QA");

    let store = Store::open(home.path()).unwrap();
    let accepted = events_of(&store, &task, event_kind::TASK_ACCEPTED);
    assert_eq!(accepted.len(), 1, "acceptance is ONE event");
    assert_eq!(accepted[0].actor, "guerrero");
    assert_eq!(
        accepted[0]
            .payload
            .get("rationale")
            .and_then(|v| v.as_str()),
        Some("принято человеком")
    );
}

/// REQ-QG-07: the return is ONE attributed event (the device rides in the
/// actor marker) plus the rejected report's saved STALE fact — not a second
/// act.
#[test]
fn return_leaves_one_event_and_marks_the_report_stale() {
    let (home, task) = seeded(TaskState::Qa);
    publish_report(&home, &task);
    let api = Api::open(home.path(), "guerrero · телефон: Pixel (192.168.1.42)").unwrap();
    api.task_return(&task.id.to_string(), Some("вернуть: пункт 1".into()))
        .unwrap();
    let status = api.task_status(&task.id.to_string()).unwrap();
    assert_eq!(status.task.state, TaskState::Rework);

    let store = Store::open(home.path()).unwrap();
    let returned = events_of(&store, &task, event_kind::QUALITY_RETURNED);
    assert_eq!(returned.len(), 1, "return is ONE event");
    assert_eq!(
        returned[0].actor, "guerrero · телефон: Pixel (192.168.1.42)",
        "the device rides in the actor marker; the person stays the author"
    );
    assert_eq!(
        returned[0]
            .payload
            .get("rationale")
            .and_then(|v| v.as_str()),
        Some("вернуть: пункт 1")
    );
    let report = store
        .artifacts(task.id)
        .unwrap()
        .into_iter()
        .find(|a| a.kind == ArtifactKind::QualityReport)
        .unwrap();
    assert_eq!(
        report.status,
        ArtifactStatus::Stale,
        "the rejected report is a saved stale fact, not a second act"
    );
}

/// Rework R1: a RUNNING audit whose owner is provably dead (reboot, killed
/// runner) must not hold the gate — the window would otherwise be locked out
/// until a human closes the row in the console.
#[test]
fn an_orphaned_audit_row_does_not_block_the_gate() {
    let Some(host) = devai_runtime::liveness::current_host() else {
        return; // no `/proc` here: nothing is provably dead
    };
    let (home, task) = seeded(TaskState::Qa);
    publish_report(&home, &task);
    let mut orphan = running_audit_row(&task);
    orphan.owner_pid = Some(i64::MAX);
    orphan.owner_host = Some(host);
    Store::open(home.path())
        .unwrap()
        .start_execution(&orphan)
        .unwrap();

    let api = Api::open(home.path(), "tester").unwrap();
    let gate = api
        .task_status(&task.id.to_string())
        .unwrap()
        .quality_gate
        .unwrap();
    assert!(!gate.audit_running, "a dead owner is not a live audit");
    assert!(gate.may_accept && gate.may_return);
    let snapshot = api.inbox_snapshot().unwrap();
    let facts = snapshot
        .tasks
        .iter()
        .find(|f| f.task.id == task.id)
        .expect("the task is in the snapshot");
    assert!(!facts.audit_running, "no «идёт аудит» for an orphan row");
}

/// REQ-QG-09 AC1: a live audit withholds both acts — the gate publishes the
/// reason and the task is kept out of the human-wait section.
#[test]
fn a_running_audit_withholds_the_gate_and_is_not_a_wait_row() {
    let (home, task) = seeded(TaskState::Qa);
    publish_report(&home, &task);
    running_audit(&home, &task);

    let api = Api::open(home.path(), "tester").unwrap();
    let gate = api
        .task_status(&task.id.to_string())
        .unwrap()
        .quality_gate
        .unwrap();
    assert!(gate.audit_running);
    assert!(!gate.may_accept && !gate.may_return);
    assert_eq!(
        gate.accept_denied.as_ref().map(|d| d.code.as_str()),
        Some("audit_running")
    );
    assert!(gate.accept_denied.unwrap().text.contains("идёт аудит"));

    let snapshot = api.inbox_snapshot().unwrap();
    let facts = snapshot
        .tasks
        .iter()
        .find(|f| f.task.id == task.id)
        .expect("the task is in the snapshot");
    assert!(
        facts.audit_running,
        "the platform publishes the live-audit fact the composer excludes on"
    );
    // The UI composer decides membership from this fact; the UI-side test
    // pins the exclusion.
}
