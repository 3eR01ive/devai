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

fn artifact(
    task: &Task,
    kind: ArtifactKind,
    seq: u32,
    status: ArtifactStatus,
    deps: Vec<Id>,
) -> Artifact {
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
    let spec = artifact(
        &t,
        ArtifactKind::RequirementsSpec,
        1,
        ArtifactStatus::Current,
        vec![],
    );
    artifacts.push(spec.clone());
    t = workflow::requirements_published(&t, artifacts, None, actor, now())
        .unwrap()
        .task;
    if target == TaskState::Review {
        return t;
    }
    t = workflow::review_recorded(&t, Verdict::Approved, None, "reviewer", now())
        .unwrap()
        .task;
    if target == TaskState::Approval {
        return t;
    }
    t = workflow::approve_requirements(&t, None, actor, now())
        .unwrap()
        .task;
    if target == TaskState::Implementation {
        return t;
    }
    t = workflow::implementation_reported(
        &t,
        &workflow::ImplementationReportFacts::bare(new_id()),
        "impl",
        now(),
    )
    .unwrap()
    .task;
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

    let spec = artifact(
        &t,
        ArtifactKind::RequirementsSpec,
        1,
        ArtifactStatus::Current,
        vec![],
    );
    artifacts.push(spec.clone());
    t = workflow::requirements_published(&t, &artifacts, None, "analyst", now())
        .unwrap()
        .task;
    assert_eq!(t.state, TaskState::Review);
    assert_eq!(t.requirements_seq, 1);

    t = workflow::review_recorded(&t, Verdict::Approved, None, "reviewer", now())
        .unwrap()
        .task;
    assert_eq!(t.state, TaskState::Approval);

    t = workflow::approve_requirements(&t, Some("ok"), "u", now())
        .unwrap()
        .task;
    assert_eq!(t.state, TaskState::Implementation);

    t = workflow::implementation_reported(
        &t,
        &workflow::ImplementationReportFacts::bare(new_id()),
        "impl",
        now(),
    )
    .unwrap()
    .task;
    assert_eq!(t.state, TaskState::Qa);

    // accept requires a quality report (and no open 'blocks' edge — &[])
    let err = workflow::accept(&t, &artifacts, &[], &[], None, "u", now()).unwrap_err();
    assert!(err.to_string().contains("QUALITY_REPORT"));

    let qr = artifact(
        &t,
        ArtifactKind::QualityReport,
        1,
        ArtifactStatus::Current,
        vec![spec.id],
    );
    artifacts.push(qr);
    t = workflow::accept(&t, &artifacts, &[], &[], None, "u", now())
        .unwrap()
        .task;
    assert_eq!(t.state, TaskState::Accepted);
    assert!(t.state.is_terminal());
}

#[test]
fn review_can_send_back_to_analysis() {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Review, &mut artifacts);
    let t = workflow::review_recorded(&t, Verdict::NeedsRevision, Some("gaps"), "reviewer", now())
        .unwrap();
    assert_eq!(t.task.state, TaskState::Analysis);
}

#[test]
fn qa_can_return_to_rework_and_back() {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Qa, &mut artifacts);
    let t = workflow::return_to_rework(&t, Some("tests missing"), "auditor", now())
        .unwrap()
        .task;
    assert_eq!(t.state, TaskState::Rework);
    // The rework brought REAL changes: the gate compares stored stamped
    // commits, a new one passes REWORK → QA exactly as before (REQ-RG-01 AC4).
    let prev = new_id();
    let facts = gate_facts(&prev, Some("aaaa"), "bbbb");
    let t = workflow::implementation_reported(&t, &facts, "impl", now())
        .unwrap()
        .task;
    assert_eq!(t.state, TaskState::Qa);
}

// --- empty-rework gate (REQ-RG-01..05) --------------------------------------

/// Facts for a REWORK publication: one prior report (id `prev`, optional
/// stamped commit) and the current report naming `current` commit.
fn gate_facts(
    prev: &Id,
    prev_commit: Option<&str>,
    current_commit: &str,
) -> workflow::ImplementationReportFacts {
    workflow::ImplementationReportFacts {
        summary: Some("rework".into()),
        current_report: new_id(),
        current_commit: Some(current_commit.into()),
        priors: vec![workflow::PriorImplementationReport {
            artifact_id: *prev,
            seq: 1,
            commit: prev_commit.map(|s| s.to_string()),
        }],
        ratifications: vec![],
        qa_return_rationale: Some("п1; п2; п3".into()),
    }
}

/// A `rework_noop_ratified`-shaped decision for the pair (prev, current).
fn ratification(prev: &Id, current: &Id) -> Decision {
    Decision {
        id: new_id(),
        task_id: new_id(),
        kind: decision_kind::REWORK_NOOP_RATIFIED.into(),
        verdict: Verdict::Approved,
        actor: "operator".into(),
        rationale: Some("no fixes were required".into()),
        refs: vec![prev.to_string(), current.to_string()],
        created_at: now(),
    }
}

fn rework_task() -> Task {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Qa, &mut artifacts);
    workflow::return_to_rework(&t, Some("п1; п2; п3"), "auditor", now())
        .unwrap()
        .task
}

#[test]
fn empty_rework_gate_trips_loud_instead_of_silent_qa() {
    // The 04.11 precedent as a unit: REWORK publication naming the identical
    // commit must NOT ride into QA — the task parks on a blocking question
    // returning to REWORK, with an immutable trip event (REQ-RG-01/02).
    let t = rework_task();
    let prev = new_id();
    let facts = gate_facts(&prev, Some("c0ffee"), "c0ffee");
    let m = workflow::implementation_reported(&t, &facts, "impl", now()).unwrap();
    assert_eq!(m.task.state, TaskState::WaitingForInput);
    assert_eq!(m.task.return_state, Some(TaskState::Rework));
    let tripped = m
        .events
        .iter()
        .find(|e| e.kind == event_kind::QA_REWORK_GATE_TRIPPED)
        .expect("the trip is an event, not a log");
    assert_eq!(tripped.payload[json_field::PREV_COMMIT], "c0ffee");
    assert_eq!(tripped.payload[json_field::CURRENT_COMMIT], "c0ffee");
    assert_eq!(tripped.payload[json_field::PREV_REPORT], prev.to_string());
    assert_eq!(
        tripped.payload[json_field::QA_RETURN_RATIONALE],
        "п1; п2; п3"
    );
    // The parked state carries its answerable question IN the mutation:
    // the store persists both in one transaction.
    let qid = m
        .events
        .iter()
        .find(|e| e.kind == event_kind::QUESTION_RAISED)
        .unwrap()
        .payload[json_field::QUESTION_ID]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(m.questions.len(), 1, "the gate ships one blocking question");
    assert_eq!(
        m.questions[0].id.to_string(),
        qid,
        "the event cites the question the store will hold"
    );
    assert!(m.questions[0].blocking);
    assert!(
        m.questions[0].text.contains("ratify-empty-rework"),
        "the question names the operator escape"
    );
}

#[test]
fn ratified_identical_rework_reaches_qa_citing_the_decision() {
    // REQ-RG-03: a human ratification for exactly this pair admits the
    // passage, and the transition event references the decision id.
    let t = rework_task();
    let prev = new_id();
    let mut facts = gate_facts(&prev, Some("c0ffee"), "c0ffee");
    let d = ratification(&prev, &facts.current_report);
    facts.ratifications.push(d.clone());
    let m = workflow::implementation_reported(&t, &facts, "operator", now()).unwrap();
    assert_eq!(m.task.state, TaskState::Qa);
    let ev = &m.events[0];
    assert_eq!(ev.kind, event_kind::IMPLEMENTATION_REPORTED);
    assert_eq!(ev.payload[json_field::RATIFIED_BY], d.id.to_string());
    assert_eq!(ev.payload[json_field::VERDICT], "IDENTICAL_RATIFIED");
}

#[test]
fn ratification_is_bound_to_its_cycle_pair_never_lifetime() {
    // REQ-RG-03 AC4: the next cycle's identical commit trips again — the old
    // decision references the previous pair and is inert for a new one.
    let t = rework_task();
    let prev = new_id();
    let mut facts = gate_facts(&prev, Some("c0ffee"), "c0ffee");
    facts
        .ratifications
        .push(ratification(&prev, &facts.current_report));
    assert_eq!(
        workflow::implementation_reported(&t, &facts, "operator", now())
            .unwrap()
            .task
            .state,
        TaskState::Qa
    );
    // New cycle: prior is the RATIFIED report itself; new current id; same
    // unchanged commit → no pair-bound ratification exists → trip.
    let mut t2 = facts.clone();
    t2.priors = vec![workflow::PriorImplementationReport {
        artifact_id: prev,
        seq: 2,
        commit: Some("c0ffee".into()),
    }];
    t2.current_report = new_id();
    let m = workflow::implementation_reported(&rework_task(), &t2, "impl", now()).unwrap();
    assert_eq!(
        m.task.state,
        TaskState::WaitingForInput,
        "cycle-N ratification is not lifetime"
    );
}

#[test]
fn incomparable_commits_warn_distinctly_and_claim_no_verdict() {
    // REQ-RG-04: one side lacks a stamped commit → the passage proceeds, but
    // the record says the check NEVER RAN — no "gate passed" verdict ships.
    let t = rework_task();
    let prev = new_id();
    let mut facts = gate_facts(&prev, Some("c0ffee"), "ignored");
    facts.current_commit = None;
    let m = workflow::implementation_reported(&t, &facts, "impl", now()).unwrap();
    assert_eq!(m.task.state, TaskState::Qa);
    assert!(
        m.events[0].payload.get(json_field::VERDICT).is_none(),
        "no verdict without a real comparison"
    );
    let warn = m
        .events
        .iter()
        .find(|e| e.kind == event_kind::QA_REWORK_GATE_INCOMPARABLE)
        .expect("the gap is a distinct event");
    assert!(
        warn.payload[json_field::REASON]
            .as_str()
            .unwrap()
            .contains("no stamped commit")
    );
    // And with neither side comparable: still distinct, still no verdict.
    let mut none = gate_facts(&prev, None, "ignored");
    none.current_commit = None;
    let m = workflow::implementation_reported(&rework_task(), &none, "impl", now()).unwrap();
    assert!(m.events[0].payload.get(json_field::VERDICT).is_none());
}

#[test]
fn first_publication_from_implementation_is_never_gated() {
    // REQ-RG-01: the IMPLEMENTATION path is untouched, even with priors that
    // would collide — the gate binds REWORK only.
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Implementation, &mut artifacts);
    let prev = new_id();
    let facts = gate_facts(&prev, Some("c0ffee"), "c0ffee");
    let m = workflow::implementation_reported(&t, &facts, "impl", now()).unwrap();
    assert_eq!(m.task.state, TaskState::Qa);
    assert!(
        m.events
            .iter()
            .all(|e| e.kind == event_kind::IMPLEMENTATION_REPORTED),
        "no gate events on the first passage"
    );
}

#[test]
fn allowed_ops_rework_lists_the_gate_primitive() {
    // REQ-RG-05: the chosen loud mechanism is visible in the operations table.
    let t = rework_task();
    let ops = workflow::allowed_ops(&t);
    assert!(
        ops.contains(&"raise-question"),
        "Rework must surface the blocking question: {ops:?}"
    );
}

// --- sovereignty violations ------------------------------------------------

#[test]
fn implementation_cannot_start_without_approval() {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Review, &mut artifacts);
    // an external agent tries to jump straight to implementation
    let err = workflow::approve_requirements(&t, None, "external", now()).unwrap_err();
    assert!(
        err.to_string().contains("not allowed in state REVIEW"),
        "{err}"
    );
    let err = workflow::implementation_reported(
        &t,
        &workflow::ImplementationReportFacts::bare(new_id()),
        "external",
        now(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("IMPLEMENTATION"), "{err}");
    // and cannot run an implementation role while awaiting review approval
    assert!(workflow::check_execution_allowed("implementation-agent", &t).is_err());
}

#[test]
fn cannot_publish_spec_from_draft() {
    let t = draft_task();
    let mut artifacts = vec![];
    let spec = artifact(
        &t,
        ArtifactKind::RequirementsSpec,
        1,
        ArtifactStatus::Current,
        vec![],
    );
    artifacts.push(spec);
    let err = workflow::requirements_published(&t, &artifacts, None, "u", now()).unwrap_err();
    assert!(
        err.to_string().contains("not allowed in state DRAFT"),
        "{err}"
    );
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
    let qr = artifact(
        &t,
        ArtifactKind::QualityReport,
        1,
        ArtifactStatus::Current,
        vec![],
    );
    artifacts.push(qr);
    let t = workflow::accept(&t, &artifacts, &[], &[], None, "u", now())
        .unwrap()
        .task;
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
    let t = workflow::resume(&t, &[], &[], "u", now()).unwrap().task;
    assert_eq!(t.state, TaskState::Implementation);
}

#[test]
fn block_and_fail_releases() {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Implementation, &mut artifacts);
    let b = workflow::block(&t, Some("need access"), "u", now())
        .unwrap()
        .task;
    assert_eq!(b.state, TaskState::Blocked);
    let b = workflow::resume(&b, &[], &[], "u", now()).unwrap().task;
    assert_eq!(b.state, TaskState::Implementation);

    let f = workflow::fail(&t, Some("runtime crashed"), "runtime", now())
        .unwrap()
        .task;
    assert_eq!(f.state, TaskState::Failed);
    let f = workflow::resume(&f, &[], &[], "u", now()).unwrap().task;
    assert_eq!(f.state, TaskState::Implementation);
}

#[test]
fn cancel_from_active_and_not_from_hold_twice() {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Review, &mut artifacts);
    let t = workflow::cancel(&t, Some("dropped"), "u", now())
        .unwrap()
        .task;
    assert_eq!(t.state, TaskState::Cancelled);
    assert!(workflow::cancel(&t, None, "u", now()).is_err());
}

// --- revise & staleness ------------------------------------------------------

#[test]
fn revise_supersedes_old_spec_and_stales_dependents() {
    let mut artifacts = vec![];
    let mut t = in_state(&draft_task(), TaskState::Implementation, &mut artifacts);

    // Implementation already produced dependent results.
    let plan = artifact(
        &t,
        ArtifactKind::ImplementationPlan,
        1,
        ArtifactStatus::Current,
        vec![artifacts[0].id],
    );
    let patch = artifact(
        &t,
        ArtifactKind::Patch,
        1,
        ArtifactStatus::Current,
        vec![plan.id],
    );
    let report = artifact(
        &t,
        ArtifactKind::ImplementationReport,
        1,
        ArtifactStatus::Current,
        vec![patch.id],
    );
    let _cache = Artifact {
        category: DataCategory::Cache,
        ..artifact(
            &t,
            ArtifactKind::ProjectProfile,
            1,
            ArtifactStatus::Current,
            vec![report.id],
        )
    };
    artifacts.extend([plan.clone(), patch.clone(), report.clone()]);

    t = workflow::implementation_reported(
        &t,
        &workflow::ImplementationReportFacts::bare(new_id()),
        "impl",
        now(),
    )
    .unwrap()
    .task; // QA

    // user revises requirements while in QA
    let spec2 = artifact(
        &t,
        ArtifactKind::RequirementsSpec,
        2,
        ArtifactStatus::Current,
        vec![],
    );
    artifacts.push(spec2.clone());
    let m =
        workflow::requirements_published(&t, &artifacts, Some("scope change"), "u", now()).unwrap();
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
    let t = workflow::reject_requirements(&t, Some("nope"), "u", now())
        .unwrap()
        .task;
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

    let plan = artifact(
        &t,
        ArtifactKind::ImplementationPlan,
        1,
        ArtifactStatus::Current,
        vec![spec1.id],
    );
    let report = artifact(
        &t,
        ArtifactKind::ImplementationReport,
        1,
        ArtifactStatus::Current,
        vec![plan.id],
    );
    artifacts.extend([plan.clone(), report.clone()]);
    t = workflow::implementation_reported(
        &t,
        &workflow::ImplementationReportFacts::bare(new_id()),
        "impl",
        now(),
    )
    .unwrap()
    .task; // QA

    // Since REQ-001 the publication flipped v1 to SUPERSEDED in the store
    // BEFORE this call sees the snapshot — a CURRENT-only filter would
    // silently empty the predecessor set and kill the stale closure.
    let mut superseded_v1 = spec1.clone();
    superseded_v1.status = ArtifactStatus::Superseded;
    let spec2 = artifact(
        &t,
        ArtifactKind::RequirementsSpec,
        2,
        ArtifactStatus::Current,
        vec![],
    );
    let artifacts: Vec<Artifact> = artifacts
        .iter()
        .map(|a| {
            if a.id == spec1.id {
                superseded_v1.clone()
            } else {
                a.clone()
            }
        })
        .chain(std::iter::once(spec2.clone()))
        .collect();

    let m =
        workflow::requirements_published(&t, &artifacts, Some("scope change"), "u", now()).unwrap();
    assert_eq!(m.task.state, TaskState::Review);
    assert_eq!(m.task.requirements_seq, 2);

    // No double flip: the already-SUPERSEDED predecessor is NOT re-flipped.
    assert!(
        !m.artifact_status
            .iter()
            .any(|(id, s)| *id == spec1.id && *s == ArtifactStatus::Superseded),
        "no competing supersession writes — the store already marked it"
    );

    // The stale closure still propagates over the superseded predecessor.
    let stale: Vec<Id> = m
        .artifact_status
        .iter()
        .filter(|(_, s)| *s == ArtifactStatus::Stale)
        .map(|(id, _)| *id)
        .collect();
    assert!(stale.contains(&plan.id), "direct dependent must go STALE");
    assert!(
        stale.contains(&report.id),
        "transitive dependent must go STALE"
    );
    assert!(
        m.events
            .iter()
            .any(|e| e.kind == event_kind::ARTIFACTS_STALED),
        "the staling is recorded"
    );
}

// --- QA return7 п.(г): a corrupt JSON array column is a read error. ---

#[test]
fn json_array_column_fails_loud_instead_of_reading_empty() {
    assert_eq!(json_array::<Id>(None).unwrap(), Vec::<Id>::new());
    assert_eq!(json_array::<Id>(Some("")).unwrap(), Vec::<Id>::new());
    assert!(
        json_array::<Id>(Some("[\"1b9d6ed4-1c7c-7c3e-9d1e-cd0f0a0b0c0d\"]"))
            .unwrap()
            .len()
            == 1
    );
    let err = json_array::<Id>(Some("{broken")).unwrap_err();
    assert!(format!("{err}").contains("corrupt JSON array"), "{err}");
}

// --- task dependency graph: typed edges gate continuation (REQ-03). ---

/// One edge as the store's dependency projection hands it to the pure
/// workflow: both endpoints' states travel in on the edge row.
fn dep(task: &Task, on_id: Id, on_state: TaskState, kind: DependencyKind) -> TaskDependency {
    TaskDependency {
        id: new_id(),
        task_id: task.id,
        task_state: task.state,
        task_title: task.title.clone(),
        depends_on_id: on_id,
        depends_on_state: on_state,
        depends_on_title: "depended-on".into(),
        kind,
        actor: "tester".into(),
        created_at: now(),
    }
}

/// A stored decision as the gate input hands it to the pure workflow.
fn decision(task: &Task, kind: &str, verdict: Verdict, refs: Vec<String>) -> Decision {
    Decision {
        id: new_id(),
        task_id: task.id,
        kind: kind.into(),
        verdict,
        actor: "operator".into(),
        rationale: Some("human act".into()),
        refs,
        created_at: now(),
    }
}

#[test]
fn resume_refuses_open_blocks_edges_and_carries_the_blocker_ids_as_data() {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Implementation, &mut artifacts);
    let t = workflow::pause(&t, None, "u", now()).unwrap().task;

    let blocker_a = new_id();
    let blocker_b = new_id();
    let genealogy_only = new_id();
    let mut deps = vec![
        dep(&t, blocker_a, TaskState::Paused, DependencyKind::Blocks),
        dep(&t, blocker_b, TaskState::Analysis, DependencyKind::Blocks),
        // RATIFIED DECISION OQ3: 'parent' is pure genealogy and NEVER
        // gates — only an explicit 'blocks' edge does.
        dep(
            &t,
            genealogy_only,
            TaskState::Implementation,
            DependencyKind::Parent,
        ),
    ];
    let err = workflow::resume(&t, &deps, &[], "u", now()).unwrap_err();
    match &err {
        devai_core::CoreError::OpenBlockers { blockers } => {
            assert_eq!(
                blockers.len(),
                2,
                "both open blockers enumerated, the parent edge not among them"
            );
            assert!(
                blockers.contains(&blocker_a.to_string())
                    && blockers.contains(&blocker_b.to_string())
            );
            assert!(!blockers.contains(&genealogy_only.to_string()));
        }
        other => panic!("expected OpenBlockers, got {other:?}"),
    }
    assert!(
        err.to_string().contains(&blocker_a.to_string()),
        "the refusal message lists the blocking ids as data"
    );

    // RATIFIED DECISION OQ1: a blocker only CLOSES its edge by being
    // ACCEPTED. CANCELLED and FAILED are dead dependencies — an UNRESOLVED
    // problem, re-tuning is a human act — so they keep the gate shut.
    for d in &mut deps {
        d.depends_on_state = if d.kind == DependencyKind::Blocks {
            TaskState::Cancelled
        } else {
            d.depends_on_state
        };
    }
    let err = workflow::resume(&t, &deps, &[], "u", now()).unwrap_err();
    assert!(
        matches!(&err, devai_core::CoreError::OpenBlockers { blockers } if blockers.len() == 2),
        "CANCELLED does not close (OQ1): {err}"
    );
    for d in &mut deps {
        d.depends_on_state = if d.kind == DependencyKind::Blocks {
            TaskState::Failed
        } else {
            d.depends_on_state
        };
    }
    assert!(
        workflow::resume(&t, &deps, &[], "u", now()).is_err(),
        "FAILED does not close (OQ1)"
    );

    // REQ-03 AC2: closing every blocking dependency (blocker ACCEPTED) lets
    // resume through with no other change.
    for d in &mut deps {
        d.depends_on_state = if d.kind == DependencyKind::Blocks {
            TaskState::Accepted
        } else {
            d.depends_on_state
        };
    }
    let r = workflow::resume(&t, &deps, &[], "u", now()).unwrap().task;
    assert_eq!(
        r.state,
        TaskState::Implementation,
        "resume passes once the blockers are ACCEPTED"
    );
    assert_eq!(r.return_state, None);
}

#[test]
fn acceptance_is_refused_while_a_blocking_edge_is_open() {
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Qa, &mut artifacts);
    let qr = artifact(
        &t,
        ArtifactKind::QualityReport,
        1,
        ArtifactStatus::Current,
        vec![],
    );
    artifacts.push(qr);
    let blocker = new_id();
    let deps = vec![dep(
        &t,
        blocker,
        TaskState::Analysis,
        DependencyKind::Blocks,
    )];
    // REQ-03 AC3: the human gate refuses and enumerates the blocker.
    let err = workflow::accept(&t, &artifacts, &deps, &[], None, "u", now()).unwrap_err();
    assert!(
        matches!(&err, devai_core::CoreError::OpenBlockers { blockers } if blockers == &vec![blocker.to_string()]),
        "{err}"
    );
    // OQ1: cancelling the blocker is NOT a closure for the acceptance gate.
    let deps_cancelled = vec![dep(
        &t,
        blocker,
        TaskState::Cancelled,
        DependencyKind::Blocks,
    )];
    assert!(
        workflow::accept(&t, &artifacts, &deps_cancelled, &[], None, "u", now()).is_err(),
        "CANCELLED does not close (OQ1)"
    );
    // The same acceptance passes with the edge closed (blocker ACCEPTED).
    let deps_closed = vec![dep(
        &t,
        blocker,
        TaskState::Accepted,
        DependencyKind::Blocks,
    )];
    let accepted = workflow::accept(&t, &artifacts, &deps_closed, &[], None, "u", now())
        .unwrap()
        .task;
    assert_eq!(accepted.state, TaskState::Accepted);
}

#[test]
fn human_override_decision_releases_the_dependency_gate() {
    // RATIFIED DECISION OQ2: the refusal is unconditional EXCEPT for an
    // explicit `dependency_override` decision (same decisions mechanism)
    // covering every currently-open blocker.
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Implementation, &mut artifacts);
    let t = workflow::pause(&t, None, "u", now()).unwrap().task;
    let blocker_a = new_id();
    let blocker_b = new_id();
    let deps = vec![
        dep(&t, blocker_a, TaskState::Paused, DependencyKind::Blocks),
        dep(&t, blocker_b, TaskState::Analysis, DependencyKind::Blocks),
    ];
    let blockers = workflow::open_blockers(&t, &deps);

    // A decision covering only ONE of two open blockers does not release.
    let partial = vec![decision(
        &t,
        decision_kind::DEPENDENCY_OVERRIDE,
        Verdict::Approved,
        vec![blocker_a.to_string()],
    )];
    assert!(
        workflow::resume(&t, &deps, &partial, "u", now()).is_err(),
        "exact coverage required"
    );
    // A non-APPROVED verdict is not a ratification.
    let rejected = vec![decision(
        &t,
        decision_kind::DEPENDENCY_OVERRIDE,
        Verdict::Rejected,
        blockers.clone(),
    )];
    assert!(
        workflow::resume(&t, &deps, &rejected, "u", now()).is_err(),
        "only APPROVED ratifies"
    );
    // An override recorded for ANOTHER task says nothing here.
    let mut foreign = decision(
        &t,
        decision_kind::DEPENDENCY_OVERRIDE,
        Verdict::Approved,
        blockers.clone(),
    );
    foreign.task_id = new_id();
    assert!(
        workflow::resume(&t, &deps, &[foreign], "u", now()).is_err(),
        "decisions are per task"
    );
    // A NEW blocker opened after the ratification re-opens the gate.
    let full = vec![decision(
        &t,
        decision_kind::DEPENDENCY_OVERRIDE,
        Verdict::Approved,
        vec![blocker_a.to_string(), blocker_b.to_string()],
    )];
    let mut deps_grown = deps.clone();
    deps_grown.push(dep(&t, new_id(), TaskState::Draft, DependencyKind::Blocks));
    assert!(
        workflow::resume(&t, &deps_grown, &full, "u", now()).is_err(),
        "new blockers are not covered"
    );
    // The exact full-coverage APPROVED decision releases resume…
    let r = workflow::resume(&t, &deps, &full, "u", now()).unwrap().task;
    assert_eq!(
        r.state,
        TaskState::Implementation,
        "the human escape releases resume"
    );
    // …and the acceptance gate too. Auto-unpause stays FORBIDDEN (OQ2):
    // the override alone changes no state — the task moved PAUSED → …
    // only because the human called `resume`; `effective_blockers` never
    // mutates, and closing a blocker resumes nothing by itself.
    let mut qa_artifacts = vec![];
    let q = in_state(&draft_task(), TaskState::Qa, &mut qa_artifacts);
    let qr = artifact(
        &q,
        ArtifactKind::QualityReport,
        1,
        ArtifactStatus::Current,
        vec![],
    );
    qa_artifacts.push(qr);
    let qb = new_id();
    let qdeps = vec![dep(&q, qb, TaskState::Paused, DependencyKind::Blocks)];
    let qfull = vec![decision(
        &q,
        decision_kind::DEPENDENCY_OVERRIDE,
        Verdict::Approved,
        vec![qb.to_string()],
    )];
    let a = workflow::accept(&q, &qa_artifacts, &qdeps, &qfull, None, "u", now())
        .unwrap()
        .task;
    assert_eq!(
        a.state,
        TaskState::Accepted,
        "the same escape releases the human acceptance gate"
    );
}

#[test]
fn override_covers_reports_false_without_open_blockers() {
    // The escape is only meaningful over an OPEN gate: with no blockers the
    // pure helper reports no coverage (nothing ratified) while the gate
    // trivially passes on the empty blocker set.
    let t = draft_task();
    let blocker = new_id();
    assert!(!workflow::override_covers(
        &t,
        &[],
        &[decision(
            &t,
            decision_kind::DEPENDENCY_OVERRIDE,
            Verdict::Approved,
            vec![]
        )]
    ));
    let d = dep(&t, blocker, TaskState::Accepted, DependencyKind::Blocks);
    assert!(
        workflow::effective_blockers(&t, &[d], &[]).is_empty(),
        "ACCEPTED closed the edge (OQ1)"
    );
}

#[test]
fn edges_of_other_tasks_never_gate_this_one() {
    // open_blockers filters on the edge's OWN dependent task id (pure
    // function robust against a graph slice handed to it wholesale).
    let mut artifacts = vec![];
    let t = in_state(&draft_task(), TaskState::Implementation, &mut artifacts);
    let t = workflow::pause(&t, None, "u", now()).unwrap().task;
    let mut foreign = dep(&t, new_id(), TaskState::Analysis, DependencyKind::Blocks);
    foreign.task_id = new_id();
    let deps = [foreign];
    assert!(workflow::open_blockers(&t, &deps).is_empty());
    assert!(workflow::resume(&t, &deps, &[], "u", now()).is_ok());
}

// --- REQ-PROG-03: pipeline projection is engine-owned display data. ---

#[test]
fn pipeline_projection_maps_states_to_their_spine_position() {
    let order = workflow::pipeline_order();
    // A1: the spine follows the declared TaskState order up to acceptance.
    assert_eq!(
        order.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
        vec![
            "DRAFT",
            "ANALYSIS",
            "REVIEW",
            "APPROVAL",
            "IMPLEMENTATION",
            "QA",
            "ACCEPTED"
        ]
    );
    let mut t = draft_task();
    // on-spine states are their own position
    t.state = TaskState::Implementation;
    assert_eq!(
        workflow::pipeline_position(&t),
        Some(TaskState::Implementation)
    );
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
