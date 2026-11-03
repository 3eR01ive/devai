//! Workflow sovereignty tests: illegal transitions must be rejected, legal
//! ones must produce the right events and staleness propagation.

use chrono::Utc;
use devai_core::types::*;
use devai_core::workflow;

fn now() -> Timestamp {
    Utc::now()
}

fn draft_task() -> Task {
    Task::new("t", "business need", None, now())
}

fn artifact(task: &Task, kind: ArtifactKind, seq: u32, status: ArtifactStatus, deps: Vec<Id>) -> Artifact {
    Artifact {
        id: new_id(),
        task_id: task.id,
        kind,
        name: format!("{}-v{seq}", kind.as_str().to_lowercase()),
        seq,
        category: DataCategory::Artifact,
        status,
        sha256: "x".into(),
        size: 1,
        media_type: "text/plain".into(),
        contract: None,
        created_by: "user".into(),
        execution_id: None,
        depends_on: deps,
        created_at: now(),
    }
}

fn in_state(task: &Task, target: TaskState, artifacts: &mut Vec<Artifact>) -> Task {
    let actor = "tester";
    let mut t = task.clone();
    if target == TaskState::Draft {
        return t;
    }
    t = workflow::start(&t, actor, now()).unwrap().task;
    if target == TaskState::Analysis {
        return t;
    }
    let spec = artifact(&t, ArtifactKind::RequirementsSpec, 1, ArtifactStatus::Current, vec![]);
    artifacts.push(spec.clone());
    t = workflow::requirements_published(&t, artifacts, None, actor, now()).unwrap().task;
    if target == TaskState::Review {
        return t;
    }
    t = workflow::review_recorded(&t, Verdict::Approved, None, "reviewer", now()).unwrap().task;
    if target == TaskState::Approval {
        return t;
    }
    t = workflow::approve_requirements(&t, None, actor, now()).unwrap().task;
    if target == TaskState::Implementation {
        return t;
    }
    t = workflow::implementation_reported(&t, None, "impl", now()).unwrap().task;
    if target == TaskState::Qa {
        return t;
    }
    t
}

// --- happy path -----------------------------------------------------------

#[test]
fn full_happy_lifecycle() {
    let mut artifacts = vec![];
    let mut t = draft_task();

    t = workflow::start(&t, "u", now()).unwrap().task;
    assert_eq!(t.state, TaskState::Analysis);

    let spec = artifact(&t, ArtifactKind::RequirementsSpec, 1, ArtifactStatus::Current, vec![]);
    artifacts.push(spec.clone());
    t = workflow::requirements_published(&t, &artifacts, None, "analyst", now()).unwrap().task;
    assert_eq!(t.state, TaskState::Review);
    assert_eq!(t.requirements_seq, 1);

    t = workflow::review_recorded(&t, Verdict::Approved, None, "reviewer", now()).unwrap().task;
    assert_eq!(t.state, TaskState::Approval);

    t = workflow::approve_requirements(&t, Some("ok"), "u", now()).unwrap().task;
    assert_eq!(t.state, TaskState::Implementation);

    t = workflow::implementation_reported(&t, None, "impl", now()).unwrap().task;
    assert_eq!(t.state, TaskState::Qa);

    // accept requires a quality report
    let err = workflow::accept(&t, &artifacts, None, "u", now()).unwrap_err();
    assert!(err.to_string().contains("QUALITY_REPORT"));

    let qr = artifact(&t, ArtifactKind::QualityReport, 1, ArtifactStatus::Current, vec![spec.id]);
    artifacts.push(qr);
    t = workflow::accept(&t, &artifacts, None, "u", now()).unwrap().task;
    assert_eq!(t.state, TaskState::Accepted);
    assert!(t.state.is_terminal());
}

#[test]
fn review_can_send_back_to_analysis() {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Review, &mut artifacts);
    let t = workflow::review_recorded(&t, Verdict::NeedsRevision, Some("gaps"), "reviewer", now()).unwrap();
    assert_eq!(t.task.state, TaskState::Analysis);
}

#[test]
fn qa_can_return_to_rework_and_back() {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Qa, &mut artifacts);
    let t = workflow::return_to_rework(&t, Some("tests missing"), "auditor", now()).unwrap().task;
    assert_eq!(t.state, TaskState::Rework);
    let t = workflow::implementation_reported(&t, None, "impl", now()).unwrap().task;
    assert_eq!(t.state, TaskState::Qa);
}

// --- sovereignty violations ------------------------------------------------

#[test]
fn implementation_cannot_start_without_approval() {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Review, &mut artifacts);
    // an external agent tries to jump straight to implementation
    let err = workflow::approve_requirements(&t, None, "external", now()).unwrap_err();
    assert!(err.to_string().contains("not allowed in state REVIEW"), "{err}");
    let err = workflow::implementation_reported(&t, None, "external", now()).unwrap_err();
    assert!(err.to_string().contains("IMPLEMENTATION"), "{err}");
    // and cannot run an implementation role while awaiting review approval
    assert!(workflow::check_execution_allowed("implementation-agent", &t).is_err());
}

#[test]
fn cannot_publish_spec_from_draft() {
    let t = draft_task();
    let mut artifacts = vec![];
    let spec = artifact(&t, ArtifactKind::RequirementsSpec, 1, ArtifactStatus::Current, vec![]);
    artifacts.push(spec);
    let err = workflow::requirements_published(&t, &artifacts, None, "u", now()).unwrap_err();
    assert!(err.to_string().contains("not allowed in state DRAFT"), "{err}");
}

#[test]
fn role_execution_stage_matrix() {
    let mut artifacts = vec![];
    let impl_task = in_state(&draft_task(), TaskState::Implementation, &mut artifacts);
    assert!(workflow::check_execution_allowed("requirements-analyst", &impl_task).is_err());
    assert!(workflow::check_execution_allowed("implementation-agent", &impl_task).is_ok());
    assert!(workflow::check_execution_allowed("quality-auditor", &impl_task).is_err());

    let mut a2 = vec![];
    let qa_task = in_state(&draft_task(), TaskState::Qa, &mut a2);
    assert!(workflow::check_execution_allowed("quality-auditor", &qa_task).is_ok());
    assert!(workflow::check_execution_allowed("implementation-agent", &qa_task).is_err());
}

#[test]
fn accepted_task_accepts_no_more_operations() {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Qa, &mut artifacts);
    let qr = artifact(&t, ArtifactKind::QualityReport, 1, ArtifactStatus::Current, vec![]);
    artifacts.push(qr);
    let t = workflow::accept(&t, &artifacts, None, "u", now()).unwrap().task;
    assert!(workflow::pause(&t, None, "u", now()).is_err());
    assert!(workflow::cancel(&t, None, "u", now()).is_err());
    assert!(workflow::start(&t, "u", now()).is_err());
    assert!(workflow::allowed_ops(&t).is_empty());
}

// --- holds and intervention -------------------------------------------------

#[test]
fn blocking_question_parks_and_answers_restores() {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Analysis, &mut artifacts);
    let q = new_id();
    let m = workflow::raise_question(&t, q, true, "analyst", now()).unwrap();
    assert_eq!(m.task.state, TaskState::WaitingForInput);
    assert_eq!(m.task.return_state, Some(TaskState::Analysis));

    let m = workflow::answer_question(&m.task, q, "u", now()).unwrap();
    assert_eq!(m.task.state, TaskState::Analysis);
    assert_eq!(m.task.return_state, None);
}

#[test]
fn nonblocking_question_keeps_state() {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Analysis, &mut artifacts);
    let m = workflow::raise_question(&t, new_id(), false, "analyst", now()).unwrap();
    assert_eq!(m.task.state, TaskState::Analysis);
}

#[test]
fn pause_resume_roundtrip() {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Implementation, &mut artifacts);
    let t = workflow::pause(&t, Some("lunch"), "u", now()).unwrap().task;
    assert_eq!(t.state, TaskState::Paused);
    assert_eq!(t.return_state, Some(TaskState::Implementation));
    let t = workflow::resume(&t, "u", now()).unwrap().task;
    assert_eq!(t.state, TaskState::Implementation);
}

#[test]
fn block_and_fail_releases() {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Implementation, &mut artifacts);
    let b = workflow::block(&t, Some("need access"), "u", now()).unwrap().task;
    assert_eq!(b.state, TaskState::Blocked);
    let b = workflow::resume(&b, "u", now()).unwrap().task;
    assert_eq!(b.state, TaskState::Implementation);

    let f = workflow::fail(&t, Some("runtime crashed"), "runtime", now()).unwrap().task;
    assert_eq!(f.state, TaskState::Failed);
    let f = workflow::resume(&f, "u", now()).unwrap().task;
    assert_eq!(f.state, TaskState::Implementation);
}

#[test]
fn cancel_from_active_and_not_from_hold_twice() {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Review, &mut artifacts);
    let t = workflow::cancel(&t, Some("dropped"), "u", now()).unwrap().task;
    assert_eq!(t.state, TaskState::Cancelled);
    assert!(workflow::cancel(&t, None, "u", now()).is_err());
}

// --- revise & staleness ------------------------------------------------------

#[test]
fn revise_supersedes_old_spec_and_stales_dependents() {
    let mut artifacts = vec![];
    let mut t = in_state(&draft_task(), TaskState::Implementation, &mut artifacts);

    // Implementation already produced dependent results.
    let plan = artifact(&t, ArtifactKind::ImplementationPlan, 1, ArtifactStatus::Current, vec![artifacts[0].id]);
    let patch = artifact(&t, ArtifactKind::Patch, 1, ArtifactStatus::Current, vec![plan.id]);
    let report = artifact(&t, ArtifactKind::ImplementationReport, 1, ArtifactStatus::Current, vec![patch.id]);
    let _cache = Artifact {
        category: DataCategory::Cache,
        ..artifact(&t, ArtifactKind::ProjectProfile, 1, ArtifactStatus::Current, vec![report.id])
    };
    artifacts.extend([plan.clone(), patch.clone(), report.clone()]);

    t = workflow::implementation_reported(&t, None, "impl", now()).unwrap().task; // QA

    // user revises requirements while in QA
    let spec2 = artifact(&t, ArtifactKind::RequirementsSpec, 2, ArtifactStatus::Current, vec![]);
    artifacts.push(spec2.clone());
    let m = workflow::requirements_published(&t, &artifacts, Some("scope change"), "u", now()).unwrap();
    t = m.task;
    assert_eq!(t.state, TaskState::Review);
    assert_eq!(t.requirements_seq, 2);

    let mut stale: Vec<_> = m
        .artifact_status
        .iter()
        .filter(|(_, s)| *s == ArtifactStatus::Stale)
        .map(|(id, _)| *id)
        .collect();
    stale.sort();
    let mut expected = vec![plan.id, patch.id, report.id];
    expected.sort();
    assert_eq!(stale, expected, "dependent artifacts must be STALE");

    let superseded: Vec<_> = m
        .artifact_status
        .iter()
        .filter(|(_, s)| *s == ArtifactStatus::Superseded)
        .map(|(id, _)| *id)
        .collect();
    assert_eq!(superseded, vec![artifacts[0].id]);
}

#[test]
fn approval_reject_returns_to_analysis() {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Approval, &mut artifacts);
    let t = workflow::reject_requirements(&t, Some("nope"), "u", now()).unwrap().task;
    assert_eq!(t.state, TaskState::Analysis);
}

#[test]
fn allowed_ops_projection() {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Approval, &mut artifacts);
    let ops = workflow::allowed_ops(&t);
    assert!(ops.contains(&"approve"));
    assert!(ops.contains(&"reject"));
    assert!(!ops.contains(&"accept"));
}

// --- REQ-007: the store already superseded at v3 — the stale closure must
// still fire, and no row may be flipped twice. ---

#[test]
fn revise_from_v3_shaped_snapshot_stales_dependents_without_double_flip() {
    let mut artifacts = vec![];
    let mut t = in_state(&draft_task(), TaskState::Implementation, &mut artifacts);
    let spec1 = artifacts[0].clone();

    let plan = artifact(&t, ArtifactKind::ImplementationPlan, 1, ArtifactStatus::Current, vec![spec1.id]);
    let report = artifact(&t, ArtifactKind::ImplementationReport, 1, ArtifactStatus::Current, vec![plan.id]);
    artifacts.extend([plan.clone(), report.clone()]);
    t = workflow::implementation_reported(&t, None, "impl", now()).unwrap().task; // QA

    // Since REQ-001 the publication flipped v1 to SUPERSEDED in the store
    // BEFORE this call sees the snapshot — a CURRENT-only filter would
    // silently empty the predecessor set and kill the stale closure.
    let mut superseded_v1 = spec1.clone();
    superseded_v1.status = ArtifactStatus::Superseded;
    let spec2 = artifact(&t, ArtifactKind::RequirementsSpec, 2, ArtifactStatus::Current, vec![]);
    let artifacts: Vec<Artifact> = artifacts.iter().map(|a| if a.id == spec1.id { superseded_v1.clone() } else { a.clone() }).chain(std::iter::once(spec2.clone())).collect();

    let m = workflow::requirements_published(&t, &artifacts, Some("scope change"), "u", now()).unwrap();
    assert_eq!(m.task.state, TaskState::Review);
    assert_eq!(m.task.requirements_seq, 2);

    // No double flip: the already-SUPERSEDED predecessor is NOT re-flipped.
    assert!(!m.artifact_status.iter().any(|(id, s)| *id == spec1.id && *s == ArtifactStatus::Superseded), "no competing supersession writes — the store already marked it");

    // The stale closure still propagates over the superseded predecessor.
    let stale: Vec<Id> = m.artifact_status.iter().filter(|(_, s)| *s == ArtifactStatus::Stale).map(|(id, _)| *id).collect();
    assert!(stale.contains(&plan.id), "direct dependent must go STALE");
    assert!(stale.contains(&report.id), "transitive dependent must go STALE");
    assert!(m.events.iter().any(|e| e.kind == event_kind::ARTIFACTS_STALED), "the staling is recorded");
}

// --- QA return7 п.(г): a corrupt JSON array column is a read error. ---

#[test]
fn json_array_column_fails_loud_instead_of_reading_empty() {
    assert_eq!(json_array::<Id>(None).unwrap(), Vec::<Id>::new());
    assert_eq!(json_array::<Id>(Some("")).unwrap(), Vec::<Id>::new());
    assert!(json_array::<Id>(Some("[\"1b9d6ed4-1c7c-7c3e-9d1e-cd0f0a0b0c0d\"]")).unwrap().len() == 1);
    let err = json_array::<Id>(Some("{broken")).unwrap_err();
    assert!(format!("{err}").contains("corrupt JSON array"), "{err}");
}

// --- REQ-PROG-03: pipeline projection is engine-owned display data. ---

#[test]
fn pipeline_projection_maps_states_to_their_spine_position() {
    let order = workflow::pipeline_order();
    // A1: the spine follows the declared TaskState order up to acceptance.
    assert_eq!(
        order.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        vec!["DRAFT", "ANALYSIS", "REVIEW", "APPROVAL", "IMPLEMENTATION", "QA", "ACCEPTED"]
    );
    let mut t = draft_task();
    // on-spine states are their own position
    t.state = TaskState::Implementation;
    assert_eq!(workflow::pipeline_position(&t), Some(TaskState::Implementation));
    assert!(!workflow::pipeline_off_spine(&t));
    // REWORK is the QA return loop: positioned at QA, off the spine itself
    t.state = TaskState::Rework;
    assert_eq!(workflow::pipeline_position(&t), Some(TaskState::Qa));
    assert!(workflow::pipeline_off_spine(&t));
    // hold states are positioned at the remembered stage
    t.state = TaskState::Paused;
    t.return_state = Some(TaskState::Review);
    assert_eq!(workflow::pipeline_position(&t), Some(TaskState::Review));
    // ACCEPTED is the last spine node; CANCELLED ends off-spine with no position
    t.state = TaskState::Accepted;
    t.return_state = None;
    assert_eq!(workflow::pipeline_position(&t), Some(TaskState::Accepted));
    assert!(!workflow::pipeline_off_spine(&t));
    t.state = TaskState::Cancelled;
    assert_eq!(workflow::pipeline_position(&t), None);
    // every spine node's ops set comes from the same engine (no CLI table):
    for s in order {
        t.state = *s;
        let _ = workflow::allowed_ops(&t);
    }
}
