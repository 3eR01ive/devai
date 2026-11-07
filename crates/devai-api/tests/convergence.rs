//! Behaviour pins of the convergence slice against a REAL store home
//! (task 01a1b5be, REQ-CV-01..13).
//!
//! The unit tests in `src/convergence.rs` pin the arithmetic on injected
//! bodies; this file pins the three things only a store can prove:
//!
//! * the slice is READ-ONLY — two reads leave tasks, artifacts, executions
//!   and events exactly as they were (REQ-CV-01);
//! * a round is assembled from the stored blob + the execution row it names
//!   (score/theta from the body, requirements/OQ from the spec the review
//!   execution was bound to, time from `finished_at - started_at`);
//! * `Api::inbox_snapshot` carries the fact of the gate's stage and `null`
//!   everywhere else, in the same batch — one unreadable body never spoils
//!   the neighbouring rows.

use chrono::{DateTime, TimeZone, Utc};
use devai_api::convergence::note;
use devai_api::{Api, STAGE_QUALITY, STAGE_SPEC_REVIEW};
use devai_core::types::*;
use devai_store::{ArtifactDraft, Store};
use serde_json::json;

fn ts(secs: i64) -> DateTime<Utc> {
    Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
}

struct Home {
    dir: tempfile::TempDir,
    store: Store,
}

impl Home {
    fn new() -> Home {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        Home { dir, store }
    }

    fn api(&self) -> Api {
        Api::open(self.dir.path(), "tester").unwrap()
    }

    fn task(&self, state: TaskState, title: &str) -> Task {
        let mut t = Task::new(title, "бизнес-описание", None, ts(0));
        t.state = state;
        self.store.create_task(&t).unwrap();
        t
    }

    fn publish(&self, task: &Task, kind: ArtifactKind, body: &str, exec: Option<Id>) -> Artifact {
        self.store
            .publish_artifact(ArtifactDraft {
                task_id: task.id,
                kind,
                name: kind.as_str().to_lowercase(),
                category: DataCategory::Artifact,
                content: body.as_bytes().to_vec(),
                media_type: "application/json".into(),
                contract: None,
                created_by: "tester".into(),
                execution_id: exec,
                depends_on: vec![],
            })
            .unwrap()
    }

    fn execution(
        &self,
        task: &Task,
        inputs: &[Id],
        started: i64,
        finished: Option<i64>,
    ) -> RoleExecution {
        let e = RoleExecution {
            id: new_id(),
            task_id: task.id,
            role_slug: "reviewer".into(),
            role_version: 1,
            agent_name: Some("mock".into()),
            status: ExecutionStatus::Running,
            inputs: inputs.iter().map(|i| i.to_string()).collect(),
            summary: None,
            error: None,
            started_at: ts(started),
            finished_at: None,
            workspace_revision: None,
            attempt: 1,
            parent_execution: None,
            failure_class: None,
            owner_pid: None,
            owner_host: None,
            owner_process_start: None,
        };
        self.store.start_execution(&e).unwrap();
        if let Some(f) = finished {
            self.store
                .finish_execution(&e, ExecutionStatus::Completed, None, None, ts(f))
                .unwrap();
        }
        e
    }
}

fn quality_body(
    score: f64,
    theta: Option<f64>,
    profile: (&str, u64),
    checks: &[(&str, f64)],
) -> String {
    let checks: Vec<serde_json::Value> = checks
        .iter()
        .map(|(id, s)| json!({ "id": id, "score": s }))
        .collect();
    let mut ev = json!({ "verdict": "FAIL", "gates": [], "checks": checks, "score": score });
    if let Some(t) = theta {
        ev["theta"] = json!(t);
    }
    json!({
        "verdict": "RETURN",
        "report_text": "текст",
        "profile": { "project": profile.0, "version": profile.1 },
        "evaluation": ev,
    })
    .to_string()
}

fn spec_body(requirements: usize, open_questions: usize) -> String {
    json!({
        "requirements": vec![json!({ "id": "r" }); requirements],
        "open_questions": vec!["вопрос"; open_questions],
    })
    .to_string()
}

fn review_body(verdict: &str) -> String {
    json!({ "verdict": verdict, "summary": "s", "findings": [], "human_paragraph": "абзац" })
        .to_string()
}

/// One task with both stages populated: two review rounds (volume
/// 14/OQ5 → 14/OQ2) and two quality rounds (0.94 → 0.98, one closed item).
fn seeded() -> (Home, Task) {
    let home = Home::new();
    let task = home.task(TaskState::Qa, "кривая");

    let spec1 = home.publish(
        &task,
        ArtifactKind::RequirementsSpec,
        &spec_body(14, 5),
        None,
    );
    let r1 = home.execution(&task, &[spec1.id], 0, Some(10));
    home.publish(
        &task,
        ArtifactKind::ReviewReport,
        &review_body("NEEDS_REVISION"),
        Some(r1.id),
    );

    let spec2 = home.publish(
        &task,
        ArtifactKind::RequirementsSpec,
        &spec_body(14, 2),
        None,
    );
    let r2 = home.execution(&task, &[spec2.id], 10, Some(20));
    home.publish(
        &task,
        ArtifactKind::ReviewReport,
        &review_body("NEEDS_REVISION"),
        Some(r2.id),
    );

    let q1 = home.execution(&task, &[], 20, Some(140));
    home.publish(
        &task,
        ArtifactKind::QualityReport,
        &quality_body(0.94, Some(0.95), ("devai", 12), &[("a", 0.5), ("b", 1.0)]),
        Some(q1.id),
    );
    let q2 = home.execution(&task, &[], 140, Some(200));
    home.publish(
        &task,
        ArtifactKind::QualityReport,
        &quality_body(0.98, Some(0.95), ("devai", 12), &[("a", 1.0), ("b", 1.0)]),
        Some(q2.id),
    );
    (home, task)
}

#[test]
fn the_slice_reads_both_stages_out_of_the_stored_facts() {
    let (home, task) = seeded();
    let v = home.api().task_convergence(&task.id.to_string()).unwrap();

    assert_eq!(v.gap_milli, 10);
    assert_eq!(v.reason, None);
    let stages: Vec<&str> = v.stages.iter().map(|s| s.stage.as_str()).collect();
    assert_eq!(stages, vec![STAGE_SPEC_REVIEW, STAGE_QUALITY], "spec first");

    let spec = &v.stages[0];
    assert_eq!(spec.series.len(), 2);
    assert_eq!(spec.series[0].requirements_count, Some(14));
    assert_eq!(spec.series[0].oq_count, Some(5));
    assert_eq!(spec.series[1].requirements_count, Some(14));
    assert_eq!(spec.series[1].oq_count, Some(2));
    assert_eq!(spec.series[1].delta_oq, Some(-3));
    assert_eq!(spec.series[1].modes, vec!["CONVERGING"]);
    assert_eq!(spec.series[1].time_seconds, Some(10));
    assert_eq!(spec.stage_total_seconds, 20);
    assert_eq!(
        spec.series[0].low_count, None,
        "check-level counters are a quality fact, not a spec one"
    );

    let q = &v.stages[1];
    assert_eq!(q.series[0].score_milli, Some(940));
    assert_eq!(q.series[1].score_milli, Some(980));
    assert_eq!(q.series[1].delta_score_milli, Some(40));
    assert_eq!(q.series[1].closed_count, Some(1));
    assert_eq!(q.series[1].theta_milli, Some(950));
    assert_eq!(q.series[1].profile.as_deref(), Some("devai@12"));
    assert_eq!(q.series[1].modes, vec!["CONVERGING"]);
    assert_eq!(q.series[1].time_seconds, Some(60));
    assert_eq!(q.stage_total_seconds, 180);
    assert!(!q.series[1].missing.iter().any(|m| *m == "theta_absent"));
}

#[test]
fn the_slice_writes_nothing() {
    let (home, task) = seeded();
    let before = (
        home.store.list_tasks().unwrap().len(),
        home.store.artifacts(task.id).unwrap().len(),
        home.store.list_executions(Some(task.id)).unwrap().len(),
        home.store.all_events_for_task(task.id).unwrap().len(),
    );
    let first = home.api().task_convergence(&task.id.to_string()).unwrap();
    let second = home.api().task_convergence(&task.id.to_string()).unwrap();
    let after = (
        home.store.list_tasks().unwrap().len(),
        home.store.artifacts(task.id).unwrap().len(),
        home.store.list_executions(Some(task.id)).unwrap().len(),
        home.store.all_events_for_task(task.id).unwrap().len(),
    );
    assert_eq!(before, after, "no rows, no events: the slice only reads");
    assert_eq!(
        serde_json::to_string(&first.stages).unwrap(),
        serde_json::to_string(&second.stages).unwrap(),
        "two reads of an unchanged store answer identically"
    );
}

#[test]
fn a_deleted_body_is_a_gap_and_never_breaks_the_neighbour() {
    let (home, task) = seeded();
    // Delete the SECOND quality report's blob: the round becomes a gap, the
    // first round keeps its own measurement (REQ-CV-03/13).
    let arts = home.store.artifacts(task.id).unwrap();
    let reports: Vec<&Artifact> = arts
        .iter()
        .filter(|a| a.kind == ArtifactKind::QualityReport)
        .collect();
    assert_eq!(reports.len(), 2);
    std::fs::remove_file(home.store.blob_path(&reports[1].sha256)).unwrap();

    let v = home.api().task_convergence(&task.id.to_string()).unwrap();
    let q = v.stages.iter().find(|s| s.stage == STAGE_QUALITY).unwrap();
    assert!(q.series[0].measured);
    assert_eq!(q.series[0].score_milli, Some(940));
    assert!(!q.series[1].measured);
    assert_eq!(q.series[1].missing, vec!["report_body_absent"]);
    assert_eq!(q.series[1].score, None, "a missing body is not a zero");
    assert_eq!(q.series[1].mode_note, Some(note::INSUFFICIENT_DATA));
    assert_eq!(q.counters.unmeasured, 1);
}

#[test]
fn the_inbox_fact_is_the_gate_stage_and_null_everywhere_else() {
    let (home, task) = seeded(); // QA
    let approval = home.task(TaskState::Approval, "утверждение");
    let spec = home.publish(
        &approval,
        ArtifactKind::RequirementsSpec,
        &spec_body(3, 1),
        None,
    );
    let r = home.execution(&approval, &[spec.id], 0, Some(5));
    home.publish(
        &approval,
        ArtifactKind::ReviewReport,
        &review_body("APPROVED"),
        Some(r.id),
    );
    let waiting = home.task(TaskState::WaitingForInput, "вопрос");
    let failed = home.task(TaskState::Failed, "сбой");

    let snap = home.api().inbox_snapshot().unwrap();
    let fact = |id: Id| {
        snap.tasks
            .iter()
            .find(|t| t.task.id == id)
            .unwrap()
            .convergence
            .clone()
    };

    let qa = fact(task.id).expect("QA row carries the quality fact");
    assert_eq!(qa.stage, STAGE_QUALITY);
    assert_eq!(qa.rounds, 2);
    assert_eq!(qa.modes, vec!["CONVERGING"]);
    assert_eq!(qa.series[1].score_milli, Some(980));

    let ap = fact(approval.id).expect("APPROVAL row carries the spec fact");
    assert_eq!(ap.stage, STAGE_SPEC_REVIEW);
    assert_eq!(ap.rounds, 1);

    assert!(fact(waiting.id).is_none(), "no chart outside the two gates");
    assert!(fact(failed.id).is_none());
}

#[test]
fn a_gate_without_rounds_answers_an_empty_fact_of_its_stage() {
    let home = Home::new();
    let task = home.task(TaskState::Approval, "пустое утверждение");
    let snap = home.api().inbox_snapshot().unwrap();
    let row = snap.tasks.iter().find(|t| t.task.id == task.id).unwrap();
    let fact = row.convergence.as_ref().expect("gate fact is present");
    assert_eq!(fact.stage, STAGE_SPEC_REVIEW);
    assert_eq!(fact.rounds, 0);
    assert!(fact.series.is_empty());
    assert_eq!(fact.mode_note, None);
}
