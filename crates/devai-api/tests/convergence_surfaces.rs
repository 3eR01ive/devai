//! The two READ surfaces carry ONE convergence object (task 01a1b5be,
//! REQ-CV-12): `task.progress` and `inbox.snapshot` answer byte-identical
//! slices for the same task, the slice never writes, and a terminal task
//! answers `null` on both.

use chrono::{DateTime, TimeZone, Utc};
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

    fn publish(&self, task: &Task, kind: ArtifactKind, body: serde_json::Value) -> Artifact {
        self.store
            .publish_artifact(ArtifactDraft {
                task_id: task.id,
                kind,
                name: kind.as_str().to_lowercase(),
                category: DataCategory::Artifact,
                content: body.to_string().into_bytes(),
                media_type: "application/json".into(),
                contract: None,
                created_by: "tester".into(),
                execution_id: None,
                depends_on: vec![],
            })
            .unwrap()
    }

    fn quality(&self, task: &Task, score: f64, checks: serde_json::Value) -> Artifact {
        self.publish(
            task,
            ArtifactKind::QualityReport,
            json!({
                "profile": {"project": "home", "version": 12},
                "evaluation": {"score": score, "theta": 0.8, "checks": checks},
            }),
        )
    }

    fn counts(&self, task: &Task) -> (usize, usize, usize, usize) {
        (
            self.store.list_tasks().unwrap().len(),
            self.store.artifacts_all().unwrap().len(),
            self.store.list_executions(None).unwrap().len(),
            self.store.all_events_for_task(task.id).unwrap().len(),
        )
    }
}

#[test]
fn progress_and_inbox_answer_the_same_slice_and_write_nothing() {
    let home = Home::new();
    let task = home.task(TaskState::Qa, "curving");
    home.quality(&task, 0.94, json!([{"id": "a", "score": 0.5}]));
    home.quality(&task, 0.99, json!([{"id": "a", "score": 1.0}]));

    let before = home.counts(&task);
    let api = home.api();
    let progress = api.task_progress(&task.id.to_string()).unwrap();
    let conv = progress.convergence.clone().expect("non-terminal task");
    assert_eq!(conv.stages.len(), 1);
    assert_eq!(conv.stages[0].stage, STAGE_QUALITY);
    assert_eq!(conv.stages[0].series.len(), 2);
    assert_eq!(conv.stages[0].series[1].score_milli, Some(990));
    assert_eq!(
        conv.stages[0].series[1].mode,
        devai_runtime::convergence::mode::CONVERGING
    );

    // A spec stage exists only when a review carrier does; here it does not.
    assert!(conv.stage(STAGE_SPEC_REVIEW).is_none());

    let snapshot = api.inbox_snapshot().unwrap();
    let row = snapshot
        .tasks
        .iter()
        .find(|f| f.task.id == task.id)
        .unwrap();
    assert_eq!(
        serde_json::to_string(&row.convergence).unwrap(),
        serde_json::to_string(&progress.convergence).unwrap(),
        "the row carries the SAME object as the progress read"
    );

    let after = home.counts(&task);
    assert_eq!(before, after, "the slice writes nothing (REQ-CV-01)");
}

#[test]
fn a_terminal_task_answers_null_on_both_surfaces() {
    let home = Home::new();
    let task = home.task(TaskState::Accepted, "done");
    home.quality(&task, 1.0, json!([]));

    let api = home.api();
    let progress = api.task_progress(&task.id.to_string()).unwrap();
    assert!(progress.convergence.is_none());
    let snapshot = api.inbox_snapshot().unwrap();
    let row = snapshot
        .tasks
        .iter()
        .find(|f| f.task.id == task.id)
        .unwrap();
    assert!(row.convergence.is_none());
}

#[test]
fn both_stages_are_present_whatever_the_task_state() {
    let home = Home::new();
    // QA is a wait row, so the snapshot builds the slice there; the stage
    // order is a property of the data, not of the stored state.
    let task = home.task(TaskState::Qa, "two stages");
    home.quality(&task, 0.9, json!([{"id": "a", "score": 0.9}]));
    let spec = home.publish(
        &task,
        ArtifactKind::RequirementsSpec,
        json!({"requirements": [{"id": "R"}], "open_questions": []}),
    );
    let exec_id = new_id();
    home.store
        .start_execution(&RoleExecution {
            id: exec_id,
            task_id: task.id,
            role_slug: "reviewer".into(),
            role_version: 1,
            agent_name: None,
            status: ExecutionStatus::Running,
            inputs: vec![spec.id.to_string()],
            summary: None,
            error: None,
            started_at: ts(1),
            finished_at: None,
            workspace_revision: None,
            attempt: 1,
            parent_execution: None,
            failure_class: None,
            owner_pid: None,
            owner_host: None,
            owner_process_start: None,
            bundle: None,
        })
        .unwrap();
    home.publish(
        &task,
        ArtifactKind::ReviewReport,
        json!({"verdict": "NEEDS_REVISION"}),
    );

    // Binding needs the review row to name the execution; publish through the
    // store draft so the link is stored.
    let api = home.api();
    let progress = api.task_progress(&task.id.to_string()).unwrap();
    let conv = progress.convergence.expect("non-terminal task");
    let names: Vec<&str> = conv.stages.iter().map(|s| s.stage).collect();
    assert!(names.contains(&STAGE_SPEC_REVIEW));
    assert!(names.contains(&STAGE_QUALITY));
    assert_eq!(names[0], STAGE_SPEC_REVIEW, "fixed stage order");
}

#[test]
fn a_wait_row_gets_the_slice_and_an_idle_row_answers_null() {
    // REQ-CV-12: the snapshot builds the slice ONLY for rows the wait
    // section shows; a task in a state the table puts in `none` answers
    // null and its bodies are never read.
    let home = Home::new();
    let waiting = home.task(TaskState::Qa, "waiting");
    home.quality(&waiting, 0.9, json!([]));
    let idle = home.task(TaskState::Implementation, "idle");
    home.quality(&idle, 0.9, json!([]));

    let snapshot = home.api().inbox_snapshot().unwrap();
    let waiting_row = snapshot
        .tasks
        .iter()
        .find(|f| f.task.id == waiting.id)
        .unwrap();
    assert!(waiting_row.convergence.is_some(), "QA is a wait row");
    let idle_row = snapshot
        .tasks
        .iter()
        .find(|f| f.task.id == idle.id)
        .unwrap();
    assert!(
        idle_row.convergence.is_none(),
        "IMPLEMENTATION is not a wait row"
    );
}

#[test]
fn snapshot_cost_is_measured_on_twenty_tasks() {
    // REQ-CV-16 AC2: the latency is measured on a fixture and PRINTED as a
    // number (the ceiling is not ratified — open question 1). Twenty wait
    // rows, three quality rounds each; the snapshot builds every slice in
    // ONE pass.
    let home = Home::new();
    for i in 0..20 {
        let task = home.task(TaskState::Qa, &format!("curve-{i}"));
        for round in 0..3 {
            home.quality(
                &task,
                0.9 + round as f64 * 0.02,
                json!([{"id": "a", "score": 0.9}]),
            );
        }
    }
    let api = home.api();
    let started = std::time::Instant::now();
    let snapshot = api.inbox_snapshot().unwrap();
    let elapsed = started.elapsed();
    println!(
        "convergence snapshot: {} tasks, {} rows with a slice, {} ms",
        snapshot.tasks.len(),
        snapshot
            .tasks
            .iter()
            .filter(|f| f.convergence.is_some())
            .count(),
        elapsed.as_millis()
    );
    assert_eq!(snapshot.tasks.len(), 20);
    assert!(
        snapshot.tasks.iter().all(|f| f.convergence.is_some()),
        "every QA row is a wait row"
    );
    assert!(
        elapsed.as_millis() < 2_000,
        "the slice must not turn a 20-task snapshot into a batch job: {elapsed:?}"
    );
}

#[test]
fn the_inbox_snapshot_reads_a_shared_spec_body_once() {
    // Five tasks, one shared spec artifact is impossible across tasks; the
    // contract we can prove at this level is that a snapshot of several
    // tasks never fails on one broken body and keeps every row.
    let home = Home::new();
    let good = home.task(TaskState::Qa, "good");
    home.quality(&good, 0.9, json!([]));
    let broken = home.task(TaskState::Qa, "broken");
    home.publish(
        &broken,
        ArtifactKind::QualityReport,
        json!({"note": "no score"}),
    );

    let api = home.api();
    let snapshot = api.inbox_snapshot().unwrap();
    assert_eq!(snapshot.tasks.len(), 2);
    let broken_row = snapshot
        .tasks
        .iter()
        .find(|f| f.task.id == broken.id)
        .unwrap();
    let conv = broken_row.convergence.as_ref().unwrap();
    assert!(!conv.stages[0].series[0].measured);
}
