//! End-to-end proof of the minimal process: the engine drives a real task
//! through every autonomous stage with role executions, stops at both human
//! gates, and cannot pass them — offline, deterministically, on `mock`.

use chrono::Utc;
use devai_core::types::*;
use devai_core::workflow;
use devai_runtime::{Engine, Gate};
use devai_store::{Store, seed};

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Store::open(dir.path()).unwrap();
    seed::seed_defaults(&store, Utc::now()).unwrap();
    (dir, store)
}

fn approved(store: &Store, id: &str) {
    let t = store.resolve_task(id).unwrap();
    let m = workflow::approve_requirements(&t, Some("test"), "tester", Utc::now()).unwrap();
    store.commit(t.lock_version, &m).unwrap();
}

fn accepted(store: &Store, id: &str) {
    let t = store.resolve_task(id).unwrap();
    let a = store.artifacts(t.id).unwrap();
    let m = workflow::accept(&t, &a, Some("test"), "tester", Utc::now()).unwrap();
    store.commit(t.lock_version, &m).unwrap();
}

#[test]
fn full_cycle_through_both_human_gates() {
    let (_d, store) = store();
    let t = Task::new("Minimal process", "Run devai's own roles end to end", None, Utc::now());
    store.create_task(&t).unwrap();
    let engine = Engine::new(&store);

    // 1) DRAFT → ... → APPROVAL, stopping exactly at the first human gate.
    let r1 = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r1.gate, Gate::WaitingApproval);
    assert_eq!(r1.steps.len(), 2, "analyst + reviewer run autonomously");
    assert_eq!(r1.steps[0].role, "requirements-analyst");
    assert_eq!(r1.steps[1].role, "requirements-reviewer");
    assert_eq!(r1.now, TaskState::Approval);

    // Sovereignty: running again changes nothing — the engine cannot approve.
    let before = store.get_task(&t.id.to_string()).unwrap();
    let r1b = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert!(r1b.steps.is_empty());
    assert_eq!(store.get_task(&t.id.to_string()).unwrap().lock_version, before.lock_version);

    // 2) human approval, then IMPLEMENTATION → QA, stop at acceptance gate.
    approved(&store, &t.id.to_string());
    let r2 = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r2.gate, Gate::WaitingAccept);
    assert_eq!(r2.steps.len(), 2, "implementation + quality auditor run");
    assert_eq!(r2.steps[0].role, "implementation-agent");
    assert_eq!(r2.steps[1].role, "quality-auditor");

    // 3) human acceptance closes the task.
    accepted(&store, &t.id.to_string());
    let r3 = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r3.gate, Gate::Terminal(TaskState::Accepted));
    assert!(r3.steps.is_empty());

    // Every stage result exists as a formal, current artifact.
    let arts = store.artifacts(t.id).unwrap();
    for kind in [
        ArtifactKind::RequirementsSpec,
        ArtifactKind::ReviewReport,
        ArtifactKind::ImplementationReport,
        ArtifactKind::QualityReport,
    ] {
        assert!(arts.iter().any(|a| a.kind == kind && a.status == ArtifactStatus::Current), "missing current {kind:?}");
    }
    // Role artifacts are attributed to executions, not to "user".
    for a in &arts {
        assert_ne!(a.created_by, "user");
        assert!(a.execution_id.is_some());
    }
    // Downstream results depend on the spec (provenance chain intact).
    let spec = arts.iter().find(|a| a.kind == ArtifactKind::RequirementsSpec).unwrap();
    for a in arts.iter().filter(|a| a.kind != ArtifactKind::RequirementsSpec) {
        assert!(a.depends_on.contains(&spec.id), "{:?} must depend on the spec", a.kind);
    }

    // History is a full, append-only record of the whole cycle.
    let ev = store.all_events_for_task(t.id).unwrap();
    let kinds: Vec<&str> = ev.iter().map(|e| e.kind.as_str()).collect();
    for expected in [
        event_kind::TASK_CREATED,
        event_kind::WORKFLOW_STARTED,
        event_kind::REQUIREMENTS_PUBLISHED,
        event_kind::REVIEW_RECORDED,
        event_kind::REQUIREMENTS_APPROVED,
        event_kind::IMPLEMENTATION_REPORTED,
        event_kind::EXECUTION_STARTED,
        event_kind::EXECUTION_FINISHED,
        event_kind::TASK_ACCEPTED,
    ] {
        assert!(kinds.contains(&expected), "event log lacks {expected}");
    }
    // Executions were recorded for all four role runs.
    assert_eq!(store.list_executions(Some(t.id)).unwrap().len(), 4);
}

#[test]
fn rejection_loops_back_through_a_new_spec_version() {
    let (_d, store) = store();
    let t = Task::new("Loop", "Reviewer rejects once, second spec passes", None, Utc::now());
    store.create_task(&t).unwrap();
    let engine = Engine::new(&store);
    engine.advance(&t.id.to_string(), "mock", "tester").unwrap();

    // human REJECT in APPROVAL → back to ANALYSIS
    let cur = store.get_task(&t.id.to_string()).unwrap();
    let m = workflow::reject_requirements(&cur, Some("too thin"), "tester", Utc::now()).unwrap();
    store.commit(cur.lock_version, &m).unwrap();

    // The engine now re-runs analyst (publishing spec v2, superseding v1)
    // and reviewer, then stops at the gate again.
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.gate, Gate::WaitingApproval);
    let after = store.get_task(&t.id.to_string()).unwrap();
    assert_eq!(after.requirements_seq, 2);

    let arts = store.artifacts(t.id).unwrap();
    let specs: Vec<_> = arts.iter().filter(|a| a.kind == ArtifactKind::RequirementsSpec).collect();
    assert_eq!(specs.len(), 2, "old spec is kept, never deleted");
    assert_eq!(specs.iter().filter(|a| a.status == ArtifactStatus::Superseded).count(), 1);
    assert_eq!(specs.iter().filter(|a| a.status == ArtifactStatus::Current).count(), 1);
}

#[test]
fn failed_adapter_leaves_workflow_untouched() {
    let (_d, store) = store();
    let t = Task::new("Failure", "An agent crash must not move the task", None, Utc::now());
    store.create_task(&t).unwrap();
    // Register a broken agent: unknown provider.
    store
        .put_agent(&AgentInstance {
            id: new_id(),
            name: "broken".into(),
            provider: "telepathy".into(),
            model: "".into(),
            runtime: "test".into(),
            created_at: Utc::now(),
        })
        .unwrap();
    let engine = Engine::new(&store);
    let err = engine.advance(&t.id.to_string(), "broken", "tester").unwrap_err();
    assert!(err.to_string().contains("telepathy"), "{err}");

    let after = store.get_task(&t.id.to_string()).unwrap();
    // Only the automatic DRAFT→ANALYSIS happened; the failed execution did
    // not change state any further, and no artifact was published.
    assert_eq!(after.state, TaskState::Analysis);
    assert!(store.artifacts(t.id).unwrap().is_empty());
    let execs = store.list_executions(Some(t.id)).unwrap();
    assert_eq!(execs.len(), 1);
    assert_eq!(execs[0].status, ExecutionStatus::Failed);
    assert!(execs[0].error.is_some());
}

#[test]
fn holds_and_terminals_stop_the_engine() {
    let (_d, store) = store();
    let t = Task::new("Paused", "Engine must not run roles on a held task", None, Utc::now());
    store.create_task(&t).unwrap();
    let engine = Engine::new(&store);
    // start, then pause
    let m = workflow::start(&store.get_task(&t.id.to_string()).unwrap(), "tester", Utc::now()).unwrap();
    store.commit(t.lock_version, &m).unwrap();
    let cur = store.get_task(&t.id.to_string()).unwrap();
    let m = workflow::pause(&cur, Some("brake"), "tester", Utc::now()).unwrap();
    store.commit(cur.lock_version, &m).unwrap();

    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.gate, Gate::Hold(TaskState::Paused));
    assert!(r.steps.is_empty());
}
