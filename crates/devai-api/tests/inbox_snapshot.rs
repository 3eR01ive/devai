//! Behaviour pins of `inbox.snapshot` — the ONE batch read behind the
//! human-wait section (task 01a1b0ea).
//!
//! The earlier iteration covered only the UI composer; the SERVER path
//! (`Api::inbox_snapshot` → `task_facts` → `gate_presentation` →
//! `group_by_task`) had no executing test at all. Here every branch is
//! exercised against a real store home:
//!
//! * the two-child-table index puts each question/artifact on ITS OWN task
//!   (no cross-task bleed, one snapshot per table);
//! * the APPROVAL presentation carries the CURRENT spec body, and the
//!   selection rule (CURRENT, else the LAST — ratified OQ-GATE-3) is pinned
//!   by the pure unit `pick_spec_artifact`;
//! * a QA task without a CURRENT quality report carries the platform's
//!   honest `report_note` instead of an empty presentation;
//! * states that present nothing (DRAFT/IMPLEMENTATION/…) get neither a
//!   spec nor a report — the batch read invents no gate.

use chrono::Utc;
use devai_api::{Api, SpecBodyView};
use devai_core::types::*;
use devai_store::{ArtifactDraft, Store};

/// One seeded home: the store handle writes the fixture rows, the API is
/// the surface under test (the same door every consumer uses).
struct Seeded {
    home: tempfile::TempDir,
    task: Task,
}

impl Seeded {
    fn new(state: TaskState, title: &str, spec_body: Option<&str>) -> Seeded {
        let home = tempfile::TempDir::new().unwrap();
        let store = Store::open(home.path()).unwrap();
        let mut task = Task::new(title, "бизнес-описание задачи", None, Utc::now());
        task.state = state;
        store.create_task(&task).unwrap();
        if let Some(body) = spec_body {
            store
                .publish_artifact(draft(task.id, ArtifactKind::RequirementsSpec, body))
                .unwrap();
        }
        Seeded { home, task }
    }

    fn store(&self) -> Store {
        Store::open(self.home.path()).unwrap()
    }

    fn api(&self) -> Api {
        Api::open(self.home.path(), "tester").unwrap()
    }
}

fn draft(task_id: Id, kind: ArtifactKind, body: &str) -> ArtifactDraft {
    ArtifactDraft {
        task_id,
        kind,
        name: format!("{}.json", kind.as_str().to_lowercase()),
        category: DataCategory::Artifact,
        content: body.as_bytes().to_vec(),
        media_type: "application/json".into(),
        contract: None,
        created_by: "user".into(),
        execution_id: None,
        depends_on: vec![],
    }
}

fn question(task_id: Id, text: &str, blocking: bool) -> Question {
    Question {
        id: new_id(),
        task_id,
        execution_id: None,
        text: text.into(),
        context: None,
        options: vec![],
        blocking,
        status: QuestionStatus::Open,
        answer: None,
        answered_by: None,
        answered_at: None,
        created_at: Utc::now(),
    }
}

/// The children of two tasks arrive on their OWN task: one index pass over
/// questions and artifacts, one lookup per task — and no row of task A can
/// appear in the facts of task B.
#[test]
fn children_are_indexed_by_their_own_task() {
    let home = tempfile::TempDir::new().unwrap();
    let store = Store::open(home.path()).unwrap();
    let mut task_a = Task::new("задача A", "бизнес-описание задачи", None, Utc::now());
    task_a.state = TaskState::Approval;
    let mut task_b = Task::new("задача B", "бизнес-описание задачи", None, Utc::now());
    task_b.state = TaskState::WaitingForInput;
    store.create_task(&task_a).unwrap();
    store.create_task(&task_b).unwrap();
    store
        .publish_artifact(draft(
            task_a.id,
            ArtifactKind::RequirementsSpec,
            "{\"human_paragraph\":\"абзац A\"}",
        ))
        .unwrap();
    store
        .create_question(&question(task_a.id, "вопрос A", true))
        .unwrap();
    store
        .create_question(&question(task_b.id, "заметка B", false))
        .unwrap();
    store
        .publish_artifact(draft(task_b.id, ArtifactKind::ReviewReport, "{}"))
        .unwrap();

    let snapshot = Api::open(home.path(), "tester")
        .unwrap()
        .inbox_snapshot()
        .unwrap();
    let facts_a = snapshot
        .tasks
        .iter()
        .find(|f| f.task.id == task_a.id)
        .expect("task A is in the snapshot");
    let facts_b = snapshot
        .tasks
        .iter()
        .find(|f| f.task.id == task_b.id)
        .expect("task B is in the snapshot");

    assert_eq!(facts_a.questions.len(), 1);
    assert_eq!(facts_a.questions[0].text, "вопрос A");
    assert_eq!(facts_b.questions.len(), 1);
    assert_eq!(facts_b.questions[0].text, "заметка B");

    // The APPROVAL presentation is read from A's own artifact slice.
    let spec = facts_a.spec.as_ref().expect("APPROVAL presents its spec");
    assert!(spec.current, "the only spec artifact is CURRENT");
    match &spec.body {
        SpecBodyView::Text { text } => assert_eq!(text, "{\"human_paragraph\":\"абзац A\"}"),
        other => panic!("a text artifact must arrive as text, got {other:?}"),
    }
    // B is WAITING_FOR_INPUT: no gate presentation is invented for it.
    assert!(facts_b.spec.is_none() && facts_b.report.is_none());
    assert!(facts_b.report_note.is_none());
}

/// A QA task whose CURRENT quality report is absent answers with the
/// platform's typed error text in `report_note` — the row is never a silent
/// hole (REQ-GATE-03 AC3).
#[test]
fn qa_without_a_current_report_carries_the_platform_note() {
    let s = Seeded::new(TaskState::Qa, "гейт качества", None);
    let snapshot = s.api().inbox_snapshot().unwrap();
    let facts = &snapshot.tasks[0];
    assert!(facts.report.is_none(), "no report to sign");
    let note = facts
        .report_note
        .as_deref()
        .expect("an honest platform note instead of an empty presentation");
    // Текст заметки принадлежит презентации (QRPT, @5): «отчёта ещё нет» —
    // после мержа единый сборщик отдаёт свою формулировку; тест держит
    // обещание «заметка называет недостающее», а не литерал старого текста.
    assert!(
        note.contains("no quality report"),
        "the note names what is missing: {note}"
    );
}

/// A QA task WITH a CURRENT report presents it whole (the machine body plus
/// the stored rendering) — the batch path and `task.report` share one
/// builder.
#[test]
fn qa_presents_the_current_report() {
    let s = Seeded::new(TaskState::Qa, "гейт качества", None);
    let store = s.store();
    store
        .publish_artifact(draft(
            s.task.id,
            ArtifactKind::QualityReport,
            "{\"verdict\":\"PASS\",\"report_text\":\"отчёт на подпись\"}",
        ))
        .unwrap();
    let snapshot = s.api().inbox_snapshot().unwrap();
    let facts = &snapshot.tasks[0];
    let report = facts
        .report
        .as_ref()
        .expect("the CURRENT report is presented");
    assert_eq!(report.report_text.as_deref(), Some("отчёт на подпись"));
    assert!(facts.report_note.is_none());
}

/// A task in a non-gate state presents nothing even when it owns artifacts
/// of the gate kinds: membership is the UI's business, presentation is the
/// stored token's.
#[test]
fn non_gate_states_present_no_gate() {
    let s = Seeded::new(
        TaskState::Implementation,
        "в работе",
        Some("{\"human_paragraph\":\"старая спека\"}"),
    );
    let snapshot = s.api().inbox_snapshot().unwrap();
    let facts = &snapshot.tasks[0];
    assert!(facts.spec.is_none() && facts.report.is_none() && facts.report_note.is_none());
    // The rows themselves still arrive: the section decides membership.
    assert_eq!(facts.task.state, TaskState::Implementation);
}

/// The batch read carries the demand table it was loaded with — the UI
/// renders wording from DATA, not from code.
#[test]
fn snapshot_carries_the_demand_table() {
    let s = Seeded::new(TaskState::Approval, "спека", None);
    let snapshot = s.api().inbox_snapshot().unwrap();
    let approval = snapshot
        .policy
        .state(TaskState::Approval)
        .expect("the home's table names every state");
    assert_eq!(
        approval.group,
        devai_core::inbox::WaitGroup::Platform,
        "APPROVAL is a platform wait in the factory table"
    );
    assert!(
        approval.commands.iter().any(|c| c.contains("task approve")),
        "the demand carries the literal CLI action"
    );
}

/// The selection rule behind `spec` is a pure function: CURRENT wins;
/// with no CURRENT the LAST spec rides with `current: false`.
#[test]
fn spec_selection_is_current_then_last() {
    // `publish_artifact` always lands CURRENT, so the fabricated rows are
    // built directly: the rule under test is statuses + seq, nothing else.
    let base = Artifact {
        id: new_id(),
        task_id: new_id(),
        kind: ArtifactKind::RequirementsSpec,
        name: "spec-v1".into(),
        seq: 1,
        category: DataCategory::Artifact,
        status: ArtifactStatus::Superseded,
        sha256: "0".repeat(64),
        size: 2,
        media_type: "application/json".into(),
        contract: None,
        created_by: "user".into(),
        execution_id: None,
        depends_on: vec![],
        created_at: Utc::now(),
    };
    let later = Artifact {
        seq: 2,
        status: ArtifactStatus::Stale,
        id: new_id(),
        ..base.clone()
    };
    let mut arts = vec![base.clone(), later.clone()];
    let (picked, current) = devai_api::pick_spec_artifact(&arts).unwrap();
    assert_eq!(picked.id, later.id, "the LAST spec is presented");
    assert!(!current, "labelled as not-current (ratified OQ-GATE-3)");

    arts[0].status = ArtifactStatus::Current;
    let (picked, current) = devai_api::pick_spec_artifact(&arts).unwrap();
    assert_eq!(picked.id, base.id, "CURRENT wins over a later sequence");
    assert!(current);
}
