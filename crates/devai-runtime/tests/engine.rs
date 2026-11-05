//! End-to-end proof of the minimal process: the engine drives a real task
//! through every autonomous stage with role executions, stops at both human
//! gates, and cannot pass them — offline, deterministically, on `mock`.

use std::path::PathBuf;

use chrono::Utc;
use devai_core::types::*;
use devai_core::workflow;
use devai_runtime::{Engine, Gate};
use devai_store::{ArtifactDraft, Store, seed};

fn store() -> (tempfile::TempDir, Store) {
    let dir = tempfile::TempDir::new().unwrap();
    let store = Store::open(dir.path()).unwrap();
    seed::seed_defaults(&store, Utc::now()).unwrap();
    (dir, store)
}

/// Probes must run against a real git workspace, not the crate dir — and
/// NEVER against the developer's own repository: a writing stage creates
/// `rework/<task-id>` branch refs and `.git/worktrees/` registrations in
/// whatever repo it is given (REQ-WT-08 channels (i)/(ii)), so pointing
/// tests at the source checkout would accumulate branch debris in a human's
/// history. One committed temp repository serves the whole test binary.
fn workspace() -> PathBuf {
    use std::sync::OnceLock;
    static WS: OnceLock<PathBuf> = OnceLock::new();
    WS.get_or_init(|| {
        let dir = tempfile::TempDir::new().unwrap();
        let p = dir.path().to_path_buf();
        git_at(&p, &["init", "-q"]);
        std::fs::write(p.join("tracked.txt"), b"alpha\n").unwrap();
        git_at(&p, &["add", "tracked.txt"]);
        git_at(
            &p,
            &[
                "-c",
                "user.email=devai@test.local",
                "-c",
                "user.name=devai-test",
                "-c",
                "commit.gpgsign=false",
                "commit",
                "-q",
                "-m",
                "bound state",
            ],
        );
        std::mem::forget(dir); // lives as long as the test binary needs it
        p
    })
    .clone()
}

/// A tiny offline profile. We must NOT run the factory profile inside tests:
/// its `cargo test --workspace` gate would spawn this very test again — an
/// evaluation harness that recurses is a harness bug, not evidence.
fn tiny_profile() -> devai_store::config::Profile {
    tiny_profile_gate("true")
}

/// The same profile with the first gate's probe command replaced. Tests that
/// must prove WHERE the platform ran the probes (REQ-WT-03) pass a
/// location-sensitive command here — the gate only passes inside the checkout
/// it targets.
fn tiny_profile_gate(cmd: &str) -> devai_store::config::Profile {
    static YAML: &str = r#"
version: 1
project: devai
applies_to_roles: [quality-auditor]
error_types:
  process/untested-branch: { desc: d, severity: minor }
  data/immutability-breach: { desc: d, severity: catastrophic }
gates:
  - id: always-green
    desc: location-sensitive gate
    probes: [ { cmd: "@GATE@", pass_exit: [0] } ]
  - id: honesty-check
    desc: a failing probe must surface, not vanish
    probes: [ { cmd: "exit 127", pass_exit: [0] } ]
evaluations:
  - id: correctness
    title: T
    checks:
      - id: mech/a
        statement: mechanical
        detects: [process/untested-branch]
        verify: mechanical
        probes: [ { cmd: "true", pass_exit: [0] } ]
        reviewer: tool
      - id: rub/b
        statement: rubric
        detects: [process/untested-branch]
        verify: rubric
        anchors: { "1.0": ok, "0.4": mid, "0.0": bad }
        reviewer: either
      - id: hum/c
        statement: human only
        detects: [data/immutability-breach]
        verify: rubric
        anchors: { "1.0": ok, "0.4": mid, "0.0": bad }
        reviewer: human
scoring:
  theta: 0.8
  floors: { catastrophic: 0.7 }
  blocked_policy: { max_blocked_weight: 0.2 }
  weights:
    mech/a: 0.5
    rub/b: 0.3
    hum/c: 0.2
"#;
    serde_yaml::from_str(&YAML.replace("@GATE@", cmd)).unwrap()
}

fn mk_engine(store: &Store) -> Engine<'_> {
    Engine::new(store)
        .with_workspace(workspace())
        .with_profile(tiny_profile())
}

fn approved(store: &Store, id: &str) {
    let t = store.resolve_task(id).unwrap();
    let m = workflow::approve_requirements(&t, Some("test"), "tester", Utc::now()).unwrap();
    store.commit(t.lock_version, &m).unwrap();
}

fn accepted(store: &Store, id: &str) {
    let t = store.resolve_task(id).unwrap();
    let a = store.artifacts(t.id).unwrap();
    // &[]: this task stores no dependency edges (REQ-08 AC2 — the absence
    // of rows is an empty set, so acceptance is unchanged for old tasks).
    let m = workflow::accept(&t, &a, &[], &[], Some("test"), "tester", Utc::now()).unwrap();
    store.commit(t.lock_version, &m).unwrap();
}

#[test]
fn full_cycle_through_both_human_gates() {
    let (_d, store) = store();
    let t = Task::new(
        "Minimal process",
        "Run devai's own roles end to end",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    let engine = mk_engine(&store);

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
    assert_eq!(
        store.get_task(&t.id.to_string()).unwrap().lock_version,
        before.lock_version
    );

    // 2) human approval, then IMPLEMENTATION → QA, stop at acceptance gate.
    approved(&store, &t.id.to_string());
    let r2 = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r2.gate, Gate::WaitingAccept);
    assert_eq!(r2.steps.len(), 2, "implementation + quality auditor run");
    assert_eq!(r2.steps[0].role, "implementation-agent");
    assert_eq!(r2.steps[1].role, "quality-auditor");

    // The QA artifact is the PLATFORM-ASSEMBLED quality-report@4: probes ran
    // in code, the mock auditor contributed no fabricated scores, and the
    // verdict function stayed "not ready" because a human check is unsigned.
    let (qa_artifact, qa) = {
        let arts = store.artifacts(t.id).unwrap();
        let a = arts
            .iter()
            .find(|a| a.kind == ArtifactKind::QualityReport)
            .unwrap()
            .clone();
        let body = store.read_blob(&a.sha256).unwrap();
        (a, body)
    };
    // The report is pinned to the strengthened contract version.
    assert_eq!(
        qa_artifact
            .contract
            .as_ref()
            .map(|c| (c.name.as_str(), c.version)),
        Some(("quality-report", 4)),
        "QA artifact must be published under quality-report@4"
    );
    let rep: serde_json::Value = serde_json::from_slice(&qa).unwrap();
    assert_eq!(rep["verdict"], "ESCALATE");
    assert_eq!(rep["profile"]["project"], "devai");

    // REQ-001/REQ-004: EVERY engine stage records a workspace revision with
    // all four components, and the assembled @4 report embeds EXACTLY the
    // revision stored on the auditor's execution row — no drift.
    {
        use devai_runtime::revision;
        let execs = store.list_executions(Some(t.id)).unwrap();
        assert_eq!(execs.len(), 4, "analyst, reviewer, implementer, auditor");
        for e in &execs {
            let r = e.workspace_revision.as_ref().unwrap_or_else(|| {
                panic!("{} execution must record a workspace_revision", e.role_slug)
            });
            assert!(r.git_head.is_some(), "{}: git HEAD component", e.role_slug);
            assert!(
                r.dirty_digest.is_some(),
                "{}: dirty digest component",
                e.role_slug
            );
            assert!(
                r.profile.as_deref().is_some_and(|p| p.contains('@')),
                "{}: profile name@version",
                e.role_slug
            );
            assert!(
                !r.roles.is_empty(),
                "{}: roles slug@version list",
                e.role_slug
            );
            assert!(
                r.is_comparable(),
                "{}: git workspace → comparable",
                e.role_slug
            );
        }
        let qa_exec = execs
            .iter()
            .find(|e| e.role_slug == "quality-auditor")
            .unwrap();
        let embedded: devai_core::types::WorkspaceRevision =
            serde_json::from_value(rep["workspace_revision"].clone())
                .expect("@4 embeds workspace_revision");
        assert_eq!(
            embedded,
            qa_exec.workspace_revision.clone().unwrap(),
            "artifact revision == execution row revision"
        );
        assert_eq!(
            embedded.git_head.as_deref(),
            revision::git_head(&workspace()).as_deref(),
            "HEAD == git rev-parse HEAD of the probe workspace"
        );
    }
    assert_eq!(rep["evaluation"]["verdict"], "INCONCLUSIVE");
    assert_eq!(rep["evaluation"]["gates"].as_array().unwrap().len(), 2);
    let states: Vec<&str> = rep["evaluation"]["gates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["state"].as_str().unwrap())
        .collect();
    assert_eq!(
        states,
        ["PASS", "BLOCKED"],
        "blocked probe never folds into PASS"
    );
    assert!(
        rep["evaluation"]["pending_human"]
            .as_array()
            .unwrap()
            .contains(&serde_json::json!("hum/c"))
    );
    let checks = rep["evaluation"]["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 3);
    assert!(
        checks
            .iter()
            .any(|c| c["id"] == "mech/a" && c["state"] == "MECHANICAL_PASS")
    );
    assert!(
        checks
            .iter()
            .any(|c| c["id"] == "rub/b" && c["state"] == "BLOCKED")
    );
    assert!(
        checks
            .iter()
            .any(|c| c["id"] == "hum/c" && c["state"] == "PENDING_HUMAN")
    );
    // QA rule "evidence per criterion": every criteria entry of the stored
    // report cites concrete evidence — the probe command+exit for mechanical
    // results, the auditor's gap record / pending signature marker otherwise.
    for c in checks.iter() {
        let ev = c["evidence"]
            .as_array()
            .unwrap_or_else(|| panic!("check {} must serialize evidence", c["id"]));
        assert!(
            ev.iter()
                .any(|e| e.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false)),
            "check {} ({}) must cite at least one concrete evidence item, got {ev:?}",
            c["id"],
            c["state"].as_str().unwrap_or("?"),
        );
    }
    assert!(
        rep["report_text"]
            .as_str()
            .unwrap()
            .starts_with("ВЕРДИКТ: ВЕРДИКТ НЕ ГОТОВ")
    );
    assert!(
        r2.steps[1]
            .summary
            .as_deref()
            .unwrap_or_default()
            .contains("ESCALATE")
    );

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
        assert!(
            arts.iter()
                .any(|a| a.kind == kind && a.status == ArtifactStatus::Current),
            "missing current {kind:?}"
        );
    }
    // Role artifacts are attributed to executions, not to "user".
    for a in &arts {
        assert_ne!(a.created_by, "user");
        assert!(a.execution_id.is_some());
    }
    // Downstream results depend on the spec (provenance chain intact).
    let spec = arts
        .iter()
        .find(|a| a.kind == ArtifactKind::RequirementsSpec)
        .unwrap();
    for a in arts
        .iter()
        .filter(|a| a.kind != ArtifactKind::RequirementsSpec)
    {
        assert!(
            a.depends_on.contains(&spec.id),
            "{:?} must depend on the spec",
            a.kind
        );
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
fn approved_decisions_reach_every_post_approve_role() {
    // Ratified OQ-RAT-1 (decision of 03.11): stored human decisions reach
    // EVERY role after the approval gate, not only the auditor — an
    // implementer who never sees the ratified OQ resolution re-implements
    // the rejected alternative (precedent 01a1a2cb-cbba itself). The mock
    // echoes its input-section labels (the OQ-F ratified observation
    // surface: echo, not prompt persistence), so this asserts the ACTUAL
    // delivery, end-to-end through the real engine.
    let (_d, store) = store();
    let t = Task::new(
        "Delivery",
        "Ratified decisions must reach the implementer",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    let engine = mk_engine(&store);
    engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    approved(&store, &t.id.to_string());
    // The human's ratified decision, stored right after approval — exactly
    // what the CLI `task approve --rationale` persists (REQ-QAD-001 path).
    store
        .record_decision(&Decision {
            id: new_id(),
            task_id: t.id,
            kind: decision_kind::REQUIREMENTS_APPROVAL.into(),
            verdict: Verdict::Approved,
            actor: "tester".into(),
            rationale: Some("OQ1=X ratified".into()),
            refs: vec![],
            created_at: Utc::now(),
        })
        .unwrap();
    engine.advance(&t.id.to_string(), "mock", "tester").unwrap();

    let arts = store.artifacts(t.id).unwrap();
    let impl_art = arts
        .iter()
        .find(|a| a.kind == ArtifactKind::ImplementationReport)
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&store.read_blob(&impl_art.sha256).unwrap()).unwrap();
    let notes = body["notes"].as_str().unwrap_or_default();
    assert!(
        notes.contains("APPROVED_DECISIONS"),
        "the implementer's prompt must carry the decisions section, echoed notes: {notes}"
    );
    let qa_art = arts
        .iter()
        .find(|a| a.kind == ArtifactKind::QualityReport)
        .unwrap();
    let qa_body = store.read_blob(&qa_art.sha256).unwrap();
    let qa: serde_json::Value = serde_json::from_slice(&qa_body).unwrap();
    assert_eq!(
        qa["verdict"], "ESCALATE",
        "auditor still runs after the shared-section change"
    );
}

#[test]
fn rejection_loops_back_through_a_new_spec_version() {
    let (_d, store) = store();
    let t = Task::new(
        "Loop",
        "Reviewer rejects once, second spec passes",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    let engine = mk_engine(&store);
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
    let specs: Vec<_> = arts
        .iter()
        .filter(|a| a.kind == ArtifactKind::RequirementsSpec)
        .collect();
    assert_eq!(specs.len(), 2, "old spec is kept, never deleted");
    assert_eq!(
        specs
            .iter()
            .filter(|a| a.status == ArtifactStatus::Superseded)
            .count(),
        1
    );
    assert_eq!(
        specs
            .iter()
            .filter(|a| a.status == ArtifactStatus::Current)
            .count(),
        1
    );
}

#[test]
fn failed_adapter_leaves_workflow_untouched() {
    let (_d, store) = store();
    let t = Task::new(
        "Failure",
        "An agent crash must not move the task",
        None,
        Utc::now(),
    );
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
    let engine = mk_engine(&store);
    let err = engine
        .advance(&t.id.to_string(), "broken", "tester")
        .unwrap_err();
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
    // REQ-FG-03: a provider failure is CLASSIFIED as provider — it never
    // entered the form loop (one row, one attempt, no rejection events).
    assert_eq!(
        execs[0].failure_class.as_deref(),
        Some(failure_class::PROVIDER)
    );
    assert_eq!(execs[0].attempt, 1);
    assert!(
        store
            .events(t.id, None)
            .unwrap()
            .iter()
            .all(|e| e.kind != event_kind::EXECUTION_FORM_REJECTED)
    );
}

// ---------------------------------------------------------------------------
// The response-form contract gate (task 01a1a2d8): a form refusal must not
// eat the cycle. The fixture providers (`mock-form-*`, `mock-spec-noecho`)
// live in adapters.rs; these tests drive the loop THROUGH the engine, which
// owns what the loop leaves behind: ROWS, EVENTS, the ESCALATION, the
// STAMPING (OQ-FG-06).
// ---------------------------------------------------------------------------

/// Register one builtin fixture provider as an agent instance.
fn form_agent(store: &Store, name: &str) {
    store
        .put_agent(&AgentInstance {
            id: new_id(),
            name: name.into(),
            provider: name.into(),
            model: "".into(),
            runtime: "test".into(),
            created_at: Utc::now(),
        })
        .unwrap();
}

/// Drive DRAFT → ANALYSIS as pure bookkeeping, so what a test observes below
/// is exactly ONE stage's behavior.
fn to_analysis(store: &Store, t: &Task) {
    let cur = store.get_task(&t.id.to_string()).unwrap();
    let m = workflow::start(&cur, "tester", Utc::now()).unwrap();
    store.commit(cur.lock_version, &m).unwrap();
}

fn analyst_rows(store: &Store, task_id: Id) -> Vec<RoleExecution> {
    let mut rows: Vec<RoleExecution> = store
        .list_executions(Some(task_id))
        .unwrap()
        .into_iter()
        .filter(|e| e.role_slug == "requirements-analyst")
        .collect();
    rows.sort_by_key(|e| e.attempt);
    rows
}

#[test]
fn form_loop_self_repairs_without_the_operator() {
    let (_d, store) = store();
    let t = Task::new(
        "Flaky form",
        "A single shape slip must heal inside one run",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    to_analysis(&store, &t);
    form_agent(&store, "mock-form-flaky");
    let engine = mk_engine(&store);
    // The run walks to the first human gate as if nothing had happened: the
    // refusals were repaired INSIDE the cycle, not by a re-launch (REQ-FG-02 AC1).
    let r = engine
        .advance(&t.id.to_string(), "mock-form-flaky", "tester")
        .unwrap();
    assert_eq!(r.gate, Gate::WaitingApproval, "{:?}", r.gate);
    assert_eq!(
        r.steps.len(),
        2,
        "analyst and reviewer each healed their form mid-run"
    );

    let rows = analyst_rows(&store, t.id);
    assert_eq!(
        rows.len(),
        2,
        "rejected attempt + accepted repair, as ROWS (REQ-FG-06/07)"
    );
    assert_eq!(rows[0].attempt, 1);
    assert_eq!(rows[0].status, ExecutionStatus::Failed);
    assert_eq!(
        rows[0].failure_class.as_deref(),
        Some(failure_class::RESPONSE_FORM)
    );
    assert!(
        rows[0].parent_execution.is_none(),
        "the root attempt is its own parent"
    );
    assert_eq!(rows[1].attempt, 2);
    assert_eq!(
        rows[1].parent_execution,
        Some(rows[0].id),
        "lineage back to the first attempt"
    );
    assert_eq!(rows[1].status, ExecutionStatus::Completed);

    // Loud, never silent (REQ-FG-07): the rejection is an EVENT with the
    // full violation list as data.
    let evs = store.events(t.id, None).unwrap();
    let rejected: Vec<_> = evs
        .iter()
        .filter(|e| e.kind == event_kind::EXECUTION_FORM_REJECTED)
        .collect();
    assert!(
        rejected
            .iter()
            .any(|e| e.payload[json_field::ROLE] == serde_json::json!("requirements-analyst")),
        "{rejected:?}"
    );
    assert!(
        rejected.iter().any(|e| e.payload[json_field::VIOLATIONS]
            .as_array()
            .is_some_and(|a| !a.is_empty())),
        "violations travel as data"
    );

    // The repaired answer became the formal artifact.
    assert!(
        store.artifacts(t.id).unwrap().iter().any(
            |a| a.kind == ArtifactKind::RequirementsSpec && a.status == ArtifactStatus::Current
        )
    );
}

#[test]
fn form_loop_exhaustion_parks_task_on_blocking_question() {
    let (_d, store) = store();
    let t = Task::new(
        "Broken form",
        "Exhaustion must become a question, not a bare FAILED",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    to_analysis(&store, &t);
    form_agent(&store, "mock-form-broken");
    let engine = mk_engine(&store).with_max_repair_attempts(2);
    let r = engine
        .advance(&t.id.to_string(), "mock-form-broken", "tester")
        .unwrap();
    // OQ-FG-08: the task waits for an ANSWER, not for a re-run command.
    assert_eq!(r.now, TaskState::WaitingForInput);
    assert_eq!(r.gate, Gate::Hold(TaskState::WaitingForInput));
    assert!(r.steps.is_empty(), "an exhausted stage produces no step");

    let rows = analyst_rows(&store, t.id);
    assert_eq!(
        rows.len(),
        3,
        "1 initial call + 2 re-prompts = the whole budget spent as rows"
    );
    assert_eq!(
        rows.iter().map(|e| e.attempt).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    assert!(rows.iter().all(|e| e.status == ExecutionStatus::Failed
        && e.failure_class.as_deref() == Some(failure_class::RESPONSE_FORM)));
    assert!(
        rows[1].parent_execution == Some(rows[0].id)
            && rows[2].parent_execution == Some(rows[0].id)
    );

    // The answerable blocking question carries the FULL violation list.
    let qs = store.list_questions(t.id).unwrap();
    assert_eq!(qs.len(), 1);
    assert!(qs[0].blocking, "the park is answerable (OQ-FG-08)");
    assert_eq!(qs[0].status, QuestionStatus::Open);
    assert!(
        qs[0].text.contains("$.unknowns") && qs[0].text.contains("$.assumptions"),
        "every violation, not the first: {}",
        qs[0].text
    );

    let evs = store.events(t.id, None).unwrap();
    assert_eq!(
        evs.iter()
            .filter(|e| e.kind == event_kind::EXECUTION_FORM_REJECTED)
            .count(),
        3
    );
    assert_eq!(
        evs.iter()
            .filter(|e| e.kind == event_kind::EXECUTION_FORM_ESCALATED)
            .count(),
        1
    );

    // Rejected bodies never become artifacts (REQ-FG-07 AC2).
    assert!(store.artifacts(t.id).unwrap().is_empty());
}

#[test]
fn form_loop_disabled_by_config_keeps_todays_failure() {
    let (_d, store) = store();
    let t = Task::new(
        "Loop off",
        "max_repair_attempts=0 means the pre-loop behavior",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    to_analysis(&store, &t);
    form_agent(&store, "mock-form-broken");
    let engine = mk_engine(&store).with_max_repair_attempts(0);
    let err = engine
        .advance(&t.id.to_string(), "mock-form-broken", "tester")
        .unwrap_err();
    assert!(
        err.to_string().contains("does not satisfy the contract"),
        "{err}"
    );
    let rows = analyst_rows(&store, t.id);
    assert_eq!(
        rows.len(),
        1,
        "budget 0: exactly one provider call, like before the feature"
    );
    assert_eq!(
        rows[0].failure_class.as_deref(),
        Some(failure_class::RESPONSE_FORM)
    );
    assert!(
        store.list_questions(t.id).unwrap().is_empty(),
        "a disabled loop escalates nothing"
    );
    assert_eq!(
        store.get_task(&t.id.to_string()).unwrap().state,
        TaskState::Analysis,
        "workflow untouched (REQ-FG-03 AC1)"
    );
}

#[test]
fn platform_stamps_spec_echo_fields_from_stored_data() {
    let (_d, store) = store();
    let t = Task::new(
        "No echo",
        "The analyst skips what only the platform knows",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    to_analysis(&store, &t);
    form_agent(&store, "mock-spec-noecho");
    let engine = mk_engine(&store);
    // A reply WITHOUT task_id/business_description is legal under @2 (the
    // ratified OQ-FG-03/07 pairing) — no repair loop is spent on it.
    let r = engine
        .advance(&t.id.to_string(), "mock-spec-noecho", "tester")
        .unwrap();
    assert_eq!(r.gate, Gate::WaitingApproval);
    assert_eq!(
        analyst_rows(&store, t.id).len(),
        1,
        "valid-under-@2 shape needs no re-prompt"
    );

    let spec = store
        .artifacts(t.id)
        .unwrap()
        .into_iter()
        .find(|a| a.kind == ArtifactKind::RequirementsSpec)
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&store.read_blob(&spec.sha256).unwrap()).unwrap();
    // REQ-FG-06: the platform completed the echo from STORED data.
    assert_eq!(body["task_id"], serde_json::json!(t.id.to_string()));
    assert_eq!(
        body["business_description"],
        serde_json::json!(t.business_description)
    );
}

/// The same contract gate `Engine::run_stage` runs on the assembled QA
/// report (engine.rs) must reject quality reports whose criteria entries
/// lack concrete evidence (rule from @3) and reports that carry no workspace
/// binding (the new @4 rule) — driven through the schema SEEDED into the
/// store, not a fixture copy of it.
#[test]
fn evidenceless_criteria_entries_are_rejected_by_the_contract_gate() {
    let (_d, store) = store();
    let schema = store
        .get_contract("quality-report", 4)
        .unwrap()
        .expect("quality-report@4 must be seeded");
    // Append-only contracts: seeding @4 must never remove or rewrite @2/@3.
    assert!(
        store.get_contract("quality-report", 2).unwrap().is_some(),
        "quality-report@2 must stay loadable"
    );
    assert!(
        store.get_contract("quality-report", 3).unwrap().is_some(),
        "quality-report@3 must stay loadable"
    );
    let good = serde_json::json!({
        "verdict": "ACCEPT",
        "recommendation_basis": "шлюзы зелёные, полы целы",
        "report_text": "ВЕРДИКТ: ПРОШЁЛ",
        "profile": { "project": "devai", "version": 2 },
        "workspace_revision": {
            "git_head": "a".repeat(40),
            "dirty_digest": "b".repeat(64),
            "profile": "devai@2",
            "roles": ["implementation-agent@1", "quality-auditor@1"],
        },
        "evaluation": {
            "verdict": "PASS", "score": 1.0, "theta": 0.8,
            "gates": [{ "id": "build-green", "state": "PASS" }],
            "checks": [
                { "id": "mech/a", "verify": "mechanical", "state": "MECHANICAL_PASS",
                  "weight": 0.5, "detects": ["process/untested-branch"],
                  "evidence": ["`cargo test -p devai-core --test workflow` -> exit 0: 42 passed"] },
                { "id": "rub/b", "verify": "rubric", "state": "SCORED", "score": 1.0,
                  "weight": 0.5, "detects": ["process/untested-branch"],
                  "evidence": ["artifact b16c3e — auditor notes on crates/devai-core/src/workflow.rs:88"] },
            ]
        }
    });
    assert!(devai_runtime::validate_against_contract(&good.to_string(), Some(&schema)).is_ok());

    // A criteria entry omitting `evidence` is contract-invalid...
    let mut missing = good.clone();
    missing["evaluation"]["checks"][1]
        .as_object_mut()
        .unwrap()
        .remove("evidence");
    let err =
        devai_runtime::validate_against_contract(&missing.to_string(), Some(&schema)).unwrap_err();
    assert!(
        err.to_string()
            .contains("$.evaluation.checks[1].evidence: missing required key"),
        "{err}"
    );

    // ...and so is an empty evidence array.
    let mut empty = good.clone();
    empty["evaluation"]["checks"][0]["evidence"] = serde_json::json!([]);
    let err =
        devai_runtime::validate_against_contract(&empty.to_string(), Some(&schema)).unwrap_err();
    assert!(
        err.to_string()
            .contains("$.evaluation.checks[0].evidence: array has 0 item(s), minimum 1"),
        "{err}"
    );

    // @4 binding: a report without the workspace_revision block is rejected...
    let mut unbound = good.clone();
    unbound
        .as_object_mut()
        .unwrap()
        .remove("workspace_revision");
    let err =
        devai_runtime::validate_against_contract(&unbound.to_string(), Some(&schema)).unwrap_err();
    assert!(
        err.to_string()
            .contains("$.workspace_revision: missing required key"),
        "{err}"
    );

    // ...and so is a block missing any of its four components (nulls allowed,
    // absence not).
    let mut half = good.clone();
    half["workspace_revision"]
        .as_object_mut()
        .unwrap()
        .remove("roles");
    let err =
        devai_runtime::validate_against_contract(&half.to_string(), Some(&schema)).unwrap_err();
    assert!(
        err.to_string()
            .contains("$.workspace_revision.roles: missing required key"),
        "{err}"
    );

    // Null git components stay schema-valid (non-git workspace) — freshness
    // logic, not the contract, decides such a revision is not comparable.
    let mut nullable = good.clone();
    nullable["workspace_revision"]["git_head"] = serde_json::Value::Null;
    nullable["workspace_revision"]["dirty_digest"] = serde_json::Value::Null;
    nullable["workspace_revision"]["profile"] = serde_json::Value::Null;
    assert!(devai_runtime::validate_against_contract(&nullable.to_string(), Some(&schema)).is_ok());
}

#[test]
fn holds_and_terminals_stop_the_engine() {
    let (_d, store) = store();
    let t = Task::new(
        "Paused",
        "Engine must not run roles on a held task",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    let engine = mk_engine(&store);
    // start, then pause
    let m = workflow::start(
        &store.get_task(&t.id.to_string()).unwrap(),
        "tester",
        Utc::now(),
    )
    .unwrap();
    store.commit(t.lock_version, &m).unwrap();
    let cur = store.get_task(&t.id.to_string()).unwrap();
    let m = workflow::pause(&cur, Some("brake"), "tester", Utc::now()).unwrap();
    store.commit(cur.lock_version, &m).unwrap();

    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.gate, Gate::Hold(TaskState::Paused));
    assert!(r.steps.is_empty());
}

// ---------------------------------------------------------------------------
// Workspace revision binding + QA freshness gate (REQ-001/005/006/008).
// Isolated git workspaces so digests are stable; the double-audit dogfood
// failure (S=0.869 → S=0.908 with NO code change) must be impossible via
// the normal engine path.
// ---------------------------------------------------------------------------

fn git_at(dir: &std::path::Path, args: &[&str]) {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git is available");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git is available");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A clean, committed, isolated git workspace: HEAD and the dirty digest are
/// both well-defined and stable across recomputations.
fn git_workspace() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::TempDir::new().unwrap();
    let p = dir.path().to_path_buf();
    git_at(&p, &["init", "-q"]);
    std::fs::write(p.join("tracked.txt"), b"alpha\n").unwrap();
    git_at(&p, &["add", "tracked.txt"]);
    git_at(
        &p,
        &[
            "-c",
            "user.email=devai@test.local",
            "-c",
            "user.name=devai-test",
            "-c",
            "commit.gpgsign=false",
            "commit",
            "-q",
            "-m",
            "bound state",
        ],
    );
    (dir, p)
}

fn engine_in<'s>(store: &'s Store, ws: &std::path::Path, force: bool) -> Engine<'s> {
    Engine::new(store)
        .with_workspace(ws.to_path_buf())
        .with_profile(tiny_profile())
        .with_force_reaudit(force)
}

/// Drive DRAFT → … → QA once: a CURRENT @4 report now exists, bound to the
/// workspace revision captured at execution start.
fn drive_to_report(store: &Store, t: &Task, ws: &std::path::Path) {
    let engine = engine_in(store, ws, false);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.gate, Gate::WaitingApproval);
    approved(store, &t.id.to_string());
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(
        r.gate,
        Gate::WaitingAccept,
        "first QA run: gate is inert (no prior report)"
    );
    assert_eq!(r.steps.len(), 2, "implementation + auditor");
}

#[test]
fn qa_reeval_on_unchanged_workspace_is_asked_not_silently_rerun() {
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    let t = Task::new(
        "Freshness",
        "Re-audit needs an explicit decision",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    drive_to_report(&store, &t, &ws);

    let execs_before = store.list_executions(Some(t.id)).unwrap().len();
    let arts_before = store.artifacts(t.id).unwrap().len();

    // REQ-006: even these MOCK executions bind the real git state — mock and
    // real rows share one shape.
    for e in store.list_executions(Some(t.id)).unwrap() {
        let r = e
            .workspace_revision
            .expect("every engine execution records a revision");
        assert!(r.is_comparable(), "{} bound to real git state", e.role_slug);
    }

    // The exact dogfood failure mode: re-run with NOTHING changed. The gate
    // must stop — no execution, no artifact, no token spend, state unmoved.
    let engine = engine_in(&store, &ws, false);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(
        r.gate,
        Gate::ReportFresh,
        "fresh workspace: auditor must not silently re-run"
    );
    assert!(r.steps.is_empty(), "no stage ran");
    assert_eq!(
        store.list_executions(Some(t.id)).unwrap().len(),
        execs_before,
        "no execution started"
    );
    assert_eq!(
        store.artifacts(t.id).unwrap().len(),
        arts_before,
        "no artifact published"
    );
    assert_eq!(
        store.get_task(&t.id.to_string()).unwrap().state,
        TaskState::Qa,
        "workflow did not move"
    );
    let kinds: Vec<String> = store
        .all_events_for_task(t.id)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert!(
        kinds.contains(&event_kind::QA_FRESH_AWAITING_DECISION.to_string()),
        "the ask is recorded: {kinds:?}"
    );
}

#[test]
fn explicit_force_reaudit_overrides_freshness_and_is_attributable() {
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    let t = Task::new("Forced", "explicit decision re-audits", None, Utc::now());
    store.create_task(&t).unwrap();
    drive_to_report(&store, &t, &ws);
    let execs_before = store.list_executions(Some(t.id)).unwrap().len();

    let engine = engine_in(&store, &ws, true);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.gate, Gate::WaitingAccept);
    assert_eq!(r.steps.len(), 1, "auditor ran on the explicit override");
    assert_eq!(r.steps[0].role, "quality-auditor");
    assert_eq!(
        store.list_executions(Some(t.id)).unwrap().len(),
        execs_before + 1
    );
    // The override is persisted, never silent: a decision row + an event.
    let decs = store.list_decisions(t.id).unwrap();
    assert!(
        decs.iter().any(|d| d.kind == "qa_reaudit_override"),
        "override decision recorded"
    );
    let kinds: Vec<String> = store
        .all_events_for_task(t.id)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert!(
        kinds.contains(&event_kind::QA_REAUDIT_FORCED.to_string()),
        "forced re-audit event: {kinds:?}"
    );
}

#[test]
fn any_workspace_change_marks_the_report_stale_and_reaudit_proceeds() {
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    let t = Task::new(
        "Stale",
        "changed workspace re-audits without asking",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    drive_to_report(&store, &t, &ws);

    // REQ-WT-02/03: with the task branch live, the audit measures the
    // PLATFORM WORKTREE — that is the state a report binds, so staleness is
    // proven by changing the worktree. (Editing the main checkout instead
    // would now honestly NOT invalidate the report: it measures something
    // else — see req008_dogfood.)
    let wt = _d.path().join("worktrees").join(t.id.to_string());
    assert!(
        wt.is_dir(),
        "the writing stage created the worktree under the data home"
    );

    // A tracked edit changes the dirty digest → the report no longer binds
    // the current state → the audit runs normally.
    std::fs::write(wt.join("tracked.txt"), b"beta\n").unwrap();
    let engine = engine_in(&store, &ws, false);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(
        r.gate,
        Gate::WaitingAccept,
        "stale report: auditor re-runs without an override"
    );
    assert_eq!(r.steps.len(), 1);

    // A NEW untracked (non-ignored) file changes the digest too: the report
    // just published binds the pre-scratch state, so this run is stale again.
    std::fs::write(wt.join("scratch.txt"), b"untracked\n").unwrap();
    let engine = engine_in(&store, &ws, false);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(
        r.gate,
        Gate::WaitingAccept,
        "untracked addition invalidates freshness"
    );
    assert_eq!(r.steps.len(), 1);
}

#[test]
fn non_git_workspace_fails_the_writing_stage_and_never_claims_freshness() {
    // The old behavior — "an implementation role runs unisolated in a
    // non-git tree" — is exactly what REQ-WT-08 forbids. A writing stage
    // without a commit to branch from must FAIL, not fall back.
    let (_d, store) = store();
    let nongit = tempfile::TempDir::new().unwrap();
    let ws = nongit.path().to_path_buf();
    let t = Task::new(
        "NoGit",
        "unbound tree cannot host a writing stage",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();

    let engine = engine_in(&store, &ws, false);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.gate, Gate::WaitingApproval);
    approved(&store, &t.id.to_string());

    // REQ-WT-01 AC2: explicit failure, no silent fallback to the current
    // directory state. The execution is recorded FAILED and the workflow
    // does not move. (A non-git tree refuses at the repository check itself;
    // the sibling rule — a git repo with no bound commit — is
    // `no_bound_commit_is_an_explicit_failure_not_a_fallback` in worktree.rs.)
    let err = engine
        .advance(&t.id.to_string(), "mock", "tester")
        .unwrap_err();
    assert!(
        err.to_string().contains("not a git repository"),
        "the error names the rule it enforces: {err}"
    );
    let impl_exec = store
        .list_executions(Some(t.id))
        .unwrap()
        .into_iter()
        .find(|e| e.role_slug == "implementation-agent")
        .expect("the writing execution is recorded");
    assert_eq!(
        impl_exec.status,
        ExecutionStatus::Failed,
        "a refused writing stage is honest history"
    );
    assert_eq!(
        store.get_task(&t.id.to_string()).unwrap().state,
        TaskState::Implementation,
        "the refusal moves no state"
    );
    assert!(
        store
            .artifacts(t.id)
            .unwrap()
            .iter()
            .all(|a| a.kind != ArtifactKind::ImplementationReport),
        "nothing is published for a failed writing run"
    );

    // The freshness half of the rule, for a report published by other means:
    // the revision IS recorded (all four components present), but git fields
    // are NULL — recorded, not comparable, such a report is never fresh.
    store
        .publish_artifact(devai_store::ArtifactDraft {
            task_id: t.id,
            kind: ArtifactKind::ImplementationReport,
            name: "implementation-report (manual)".into(),
            category: DataCategory::Artifact,
            content: serde_json::json!({ "branch": "none", "patch": "manually published", "test_results": {} }).to_string().into_bytes(),
            media_type: "application/json".into(),
            contract: None,
            created_by: "user".into(),
            execution_id: None,
            depends_on: vec![],
        })
        .unwrap();
    let cur = store.get_task(&t.id.to_string()).unwrap();
    let mut facts = workflow::ImplementationReportFacts::bare(new_id());
    facts.summary = Some("manual".into());
    let m = workflow::implementation_reported(&cur, &facts, "tester", Utc::now()).unwrap();
    store.commit(cur.lock_version, &m).unwrap();

    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.gate, Gate::WaitingAccept);
    let execs = store.list_executions(Some(t.id)).unwrap();
    let qa = execs
        .iter()
        .find(|e| e.role_slug == "quality-auditor")
        .unwrap();
    let rev = qa
        .workspace_revision
        .as_ref()
        .expect("non-git executions still record a revision");
    assert!(
        rev.git_head.is_none() && rev.dirty_digest.is_none(),
        "no git → NULL head+digest"
    );
    assert!(!rev.is_comparable());

    // Re-run: the auditor runs again (gate inert), it never claims freshness.
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.gate, Gate::WaitingAccept);
    assert_eq!(
        r.steps.len(),
        1,
        "non-git workspace: re-audit proceeds, never blocked as fresh"
    );
}

#[test]
fn report_and_execution_rows_never_drift_and_bumps_mark_stale() {
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    let t = Task::new(
        "Drift",
        "artifact embeds the execution row's revision",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    drive_to_report(&store, &t, &ws);

    // REQ-004: the embedded object equals the stored execution row verbatim.
    let arts = store.artifacts(t.id).unwrap();
    let a = arts
        .iter()
        .filter(|x| x.kind == ArtifactKind::QualityReport)
        .max_by_key(|x| x.seq)
        .unwrap()
        .clone();
    let rep: serde_json::Value =
        serde_json::from_slice(&store.read_blob(&a.sha256).unwrap()).unwrap();
    let qa = store
        .list_executions(Some(t.id))
        .unwrap()
        .into_iter()
        .find(|e| e.role_slug == "quality-auditor")
        .unwrap();
    let embedded: devai_core::types::WorkspaceRevision =
        serde_json::from_value(rep["workspace_revision"].clone()).unwrap();
    assert_eq!(embedded, qa.workspace_revision.clone().unwrap());

    // A role definition bump changes the roles component → stale (A-002:
    // ALL four components must match for fresh). Simulate the init diff-bump:
    // insert a new role version (history row stays).
    let roles = store.list_roles().unwrap();
    let impl_role = roles
        .iter()
        .find(|r| r.slug == "implementation-agent")
        .cloned()
        .unwrap();
    let mut bumped = impl_role.clone();
    bumped.id = new_id();
    bumped.version = impl_role.version + 1;
    store.put_role(&bumped).unwrap();

    let engine = engine_in(&store, &ws, false);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(
        r.gate,
        Gate::WaitingAccept,
        "role version bump alone invalidates freshness"
    );
    assert_eq!(r.steps.len(), 1);
}

#[test]
fn profile_version_bump_marks_report_stale() {
    // Freshness compares ALL FOUR components: an evaluation-profile bump
    // (the QA rules themselves changed) invalidates even a bit-identical
    // git tree — the old verdict was rendered under different rules.
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    let t = Task::new(
        "Profile bump",
        "new rules force a fresh audit",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    drive_to_report(&store, &t, &ws);

    let mut p = tiny_profile();
    p.version = 2;
    let engine = Engine::new(&store).with_workspace(&ws).with_profile(p);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(
        r.gate,
        Gate::WaitingAccept,
        "profile identity changed: not fresh"
    );
    assert_eq!(r.steps.len(), 1);
    // The new report binds the NEW profile identity.
    let arts = store.artifacts(t.id).unwrap();
    let a = arts
        .iter()
        .filter(|x| x.kind == ArtifactKind::QualityReport)
        .max_by_key(|x| x.seq)
        .unwrap();
    let rep: serde_json::Value =
        serde_json::from_slice(&store.read_blob(&a.sha256).unwrap()).unwrap();
    assert_eq!(rep["workspace_revision"]["profile"], "devai@2");
}

#[test]
fn legacy_unbound_report_leaves_the_gate_inert() {
    // @1..@3 reports carry no binding: freshness cannot be CLAIMED for them
    // and must not be DENIED either — the audit simply proceeds, so an
    // existing install upgrades without getting stuck behind a fake ask.
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    let t = Task::new(
        "Legacy",
        "pre-@4 CURRENT report: gate inert",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    drive_to_report(&store, &t, &ws);

    // Re-publish the CURRENT report as a @3-shaped doc WITHOUT the revision
    // block (the highest seq among CURRENT wins the freshness check).
    let arts = store.artifacts(t.id).unwrap();
    let a = arts
        .iter()
        .filter(|x| x.kind == ArtifactKind::QualityReport)
        .max_by_key(|x| x.seq)
        .unwrap()
        .clone();
    let mut rep: serde_json::Value =
        serde_json::from_slice(&store.read_blob(&a.sha256).unwrap()).unwrap();
    rep.as_object_mut().unwrap().remove("workspace_revision");
    store
        .publish_artifact(devai_store::ArtifactDraft {
            task_id: t.id,
            kind: ArtifactKind::QualityReport,
            name: "quality-report (legacy @3 shape)".into(),
            category: DataCategory::Artifact,
            content: rep.to_string().into_bytes(),
            media_type: "application/json".into(),
            contract: Some(ContractRef {
                name: "quality-report".into(),
                version: 3,
            }),
            created_by: "user".into(),
            execution_id: None,
            depends_on: vec![],
        })
        .unwrap();

    let engine = engine_in(&store, &ws, false);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(
        r.gate,
        Gate::WaitingAccept,
        "unbound report: audit proceeds, never a freshness ask"
    );
    assert_eq!(r.steps.len(), 1);
}

// ---------------------------------------------------------------------------
// Worktree isolation for writing roles (REQ-WT-01/02/04/05/08). The unit
// tests in `worktree.rs` prove the plumbing in isolation; these prove the
// ENGINE routes a real stage through it — the defect of 2026-11-01 was the
// agent working uncommitted in the user's checkout, and only the engine can
// make that impossible.
// ---------------------------------------------------------------------------

#[test]
fn writing_stage_is_isolated_in_platform_worktree() {
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    // An untracked file in the main checkout must NOT travel into the
    // branch: the worktree is created from the bound COMMIT (REQ-WT-01 AC1;
    // interim platform answer to OQ-3).
    std::fs::write(ws.join("main-only.txt"), b"not for the branch\n").unwrap();
    let head = git_out(&ws, &["rev-parse", "HEAD"]);
    // REQ-WT-08 AC2 witnesses — the exact state of the main checkout before.
    let status_before = git_out(
        &ws,
        &["status", "--porcelain=v1", "-z", "--untracked-files=all"],
    );
    let index_before = git_out(&ws, &["ls-files", "-s"]);

    let t = Task::new(
        "Isolation",
        "writing runs on a platform branch",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    let engine = engine_in(&store, &ws, false);
    engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    approved(&store, &t.id.to_string());
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.gate, Gate::WaitingAccept);

    let branch = devai_runtime::worktree::branch_name(t.id);
    let wt = _d.path().join("worktrees").join(t.id.to_string());
    // REQ-WT-01: the platform created the worktree on the task's branch,
    // pointing at exactly the bound commit, OUTSIDE the checkout (REQ-WT-05).
    assert!(
        wt.is_dir(),
        "the platform created the isolation worktree under the data home"
    );
    assert_eq!(
        git_out(&wt, &["branch", "--show-current"]),
        branch,
        "REQ-WT-02: the writing checkout is the task branch"
    );
    assert_eq!(
        git_out(&wt, &["rev-parse", "HEAD"]),
        head,
        "REQ-WT-01 AC1: the branch starts at the bound commit"
    );
    assert!(
        !wt.join("main-only.txt").exists(),
        "OQ-3: the main checkout's dirt is not transported"
    );

    // REQ-WT-08 AC2: after a whole writing stage the main checkout is
    // byte-identical — only the two enumerated channels were touched.
    assert_eq!(
        git_out(&ws, &["rev-parse", "HEAD"]),
        head,
        "main HEAD unmoved"
    );
    assert_eq!(
        git_out(
            &ws,
            &["status", "--porcelain=v1", "-z", "--untracked-files=all"]
        ),
        status_before,
        "main working tree unmoved"
    );
    assert_eq!(
        git_out(&ws, &["ls-files", "-s"]),
        index_before,
        "main index unmoved"
    );

    // REQ-WT-04: the published report names branch+worktree+commit STAMPED
    // by the platform — the agent's own claims are overwritten with ground
    // truth and the stamped body must pass implementation-report@2 again.
    let arts = store.artifacts(t.id).unwrap();
    let impl_a = arts
        .iter()
        .find(|a| a.kind == ArtifactKind::ImplementationReport)
        .unwrap();
    assert_eq!(
        impl_a
            .contract
            .as_ref()
            .map(|c| (c.name.as_str(), c.version)),
        Some(("implementation-report", 2)),
        "the role's declared @2 contract is what got published"
    );
    let body: serde_json::Value =
        serde_json::from_slice(&store.read_blob(&impl_a.sha256).unwrap()).unwrap();
    assert_eq!(body["branch"], branch);
    assert_eq!(body["worktree"], wt.display().to_string());
    // The mock changed nothing: the commit that captures the work is the
    // tip that already equals it — no empty commit is faked (REQ-WT-04 AC3).
    assert_eq!(
        body["commit"], head,
        "the named commit resolves on the task branch"
    );
    let schema = store
        .get_contract("implementation-report", 2)
        .unwrap()
        .expect("implementation-report@2 must be seeded");
    assert!(
        devai_runtime::validate_against_contract(&body.to_string(), Some(&schema)).is_ok(),
        "stamped body passes @2"
    );

    // REQ-WT-05: the run's evidence sink lives under the home, declared to
    // the adapter — created for the writing execution.
    let impl_exec = store
        .list_executions(Some(t.id))
        .unwrap()
        .into_iter()
        .find(|e| e.role_slug == "implementation-agent")
        .unwrap();
    assert_eq!(impl_exec.status, ExecutionStatus::Completed);
    let ev = _d
        .path()
        .join("evidence")
        .join(t.id.to_string())
        .join(impl_exec.id.to_string());
    assert!(
        ev.is_dir(),
        "platform-declared evidence dir created for the writing execution"
    );

    // The repository facts are immutable history, not just artifact claims.
    let kinds: Vec<String> = store
        .all_events_for_task(t.id)
        .unwrap()
        .into_iter()
        .map(|e| e.kind)
        .collect();
    assert!(
        kinds.contains(&event_kind::WORKTREE_PREPARED.to_string()),
        "worktree.prepared recorded: {kinds:?}"
    );
    assert!(
        kinds.contains(&event_kind::WORKTREE_COMMITTED.to_string()),
        "worktree.committed recorded: {kinds:?}"
    );
}

#[test]
fn qa_probes_measure_the_worktree_not_the_main_checkout() {
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    let t = Task::new(
        "Probe location",
        "REQ-WT-03: gates run where the work lives",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    let engine = engine_in(&store, &ws, false);
    engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    approved(&store, &t.id.to_string());

    // A gate that passes ONLY inside the task's branch checkout: run in the
    // main checkout the same command exits 1. A PASS is proof of location.
    let branch = devai_runtime::worktree::branch_name(t.id);
    let engine = Engine::new(&store)
        .with_workspace(ws.clone())
        .with_profile(tiny_profile_gate(&format!(
            "git branch --show-current | grep -qx '{branch}'"
        )));
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(
        r.gate,
        Gate::WaitingAccept,
        "implementation ran, then the audit measured its worktree"
    );

    let arts = store.artifacts(t.id).unwrap();
    let a = arts
        .iter()
        .find(|x| x.kind == ArtifactKind::QualityReport)
        .unwrap();
    let rep: serde_json::Value =
        serde_json::from_slice(&store.read_blob(&a.sha256).unwrap()).unwrap();
    let gates = rep["evaluation"]["gates"].as_array().unwrap();
    assert_eq!(gates[0]["id"], "always-green");
    assert_eq!(
        gates[0]["state"], "PASS",
        "REQ-WT-03: the gate probe ran on the task branch — impossible outside the worktree"
    );

    // REQ-WT-05: the transcript is a run artifact under <home>/evidence,
    // referenced from the prompt by path; nothing lands in either checkout
    // (the 2026-11-01 dogfood wrote an evidence file into the source tree).
    let qa_exec = store
        .list_executions(Some(t.id))
        .unwrap()
        .into_iter()
        .find(|e| e.role_slug == "quality-auditor")
        .unwrap();
    let transcript = _d
        .path()
        .join("evidence")
        .join(t.id.to_string())
        .join(qa_exec.id.to_string())
        .join("probes.txt");
    let txt = std::fs::read_to_string(&transcript)
        .expect("probe transcript persisted under the evidence root");
    assert!(
        txt.contains("git branch --show-current"),
        "the transcript cites the command: {txt}"
    );
    assert!(
        !ws.join("probes.txt").exists(),
        "no run artifact in the main checkout"
    );
    assert!(
        !_d.path()
            .join("worktrees")
            .join(t.id.to_string())
            .join("probes.txt")
            .exists(),
        "no run artifact in the worktree"
    );
}

// ---------------------------------------------------------------------------
// Empty-rework gate (REQ-RG-01..06). Regression for the 04.11 precedent
// (task 01a1a14e-2c07): QA returned three concrete points, the REWORK
// execution published a report stamped with the SAME commit as the previous
// one — an empty diff, five minutes — and the platform carried it silently
// into QA. From now on that passage is loud: it parks on an operator
// question and only a stored human decision can call it a conscious no-op.
// ---------------------------------------------------------------------------

#[test]
fn empty_rework_gate_trips_loud_and_ratified_no_op_reopens_qa() {
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    let head = git_out(&ws, &["rev-parse", "HEAD"]);
    let t = Task::new(
        "Empty rework",
        "QA return + unchanged rework must be loud",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    drive_to_report(&store, &t, &ws); // report R1: mock changed nothing → stamped commit == head

    // QA returns with three concrete points — the precedent's shape.
    let cur = store.get_task(&t.id.to_string()).unwrap();
    let m = workflow::return_to_rework(&cur, Some("п1; п2; п3"), "tester", Utc::now()).unwrap();
    store.commit(cur.lock_version, &m).unwrap();

    // The rework execution does nothing either: R2 is stamped with the SAME
    // tip (worktree.rs returns the branch tip, never an empty commit — A1).
    let engine = engine_in(&store, &ws, false);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.steps.len(), 1, "the rework stage ran and published");
    assert_eq!(
        r.steps[0].to,
        TaskState::WaitingForInput,
        "the identical commit parks the task — QA is withheld"
    );
    assert_eq!(
        r.gate,
        Gate::Hold(TaskState::WaitingForInput),
        "the engine stops LOUD at the hold"
    );
    let held = store.get_task(&t.id.to_string()).unwrap();
    assert_eq!(held.state, TaskState::WaitingForInput);
    assert_eq!(
        held.return_state,
        Some(TaskState::Rework),
        "the re-entry point is REWORK"
    );
    // progress/status projection convention (REQ-RG-02 AC2): off-spine,
    // re-entering at REWORK.
    assert!(workflow::pipeline_off_spine(&held));
    assert_eq!(workflow::pipeline_position(&held), Some(TaskState::Rework));

    // The trip is an immutable event naming both reports, both shas and the
    // QA points the skipped rework was supposed to fix (REQ-RG-06 AC2).
    let events = store.all_events_for_task(t.id).unwrap();
    let trip = events
        .iter()
        .rev()
        .find(|e| e.kind == event_kind::QA_REWORK_GATE_TRIPPED)
        .expect("the gate records a trip event");
    assert_eq!(trip.task_id, Some(t.id), "the event is bound to the task");
    assert_eq!(trip.payload[json_field::PREV_COMMIT], head);
    assert_eq!(trip.payload[json_field::CURRENT_COMMIT], head);
    assert_eq!(trip.payload[json_field::QA_RETURN_RATIONALE], "п1; п2; п3");
    let prev_id = trip.payload[json_field::PREV_REPORT]
        .as_str()
        .unwrap()
        .to_string();
    let cur_id = trip.payload[json_field::CURRENT_REPORT]
        .as_str()
        .unwrap()
        .to_string();

    // The parked state carries a real, answerable question (not only a log).
    let open: Vec<_> = store
        .list_questions(t.id)
        .unwrap()
        .into_iter()
        .filter(|q| q.status == QuestionStatus::Open)
        .collect();
    assert_eq!(open.len(), 1, "the gate ships the operator one question");
    assert!(
        open[0].blocking && open[0].text.contains("ratify-empty-rework"),
        "the question names the escape"
    );

    // While unratified, QA is unreachable: the engine holds, nothing runs.
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert!(r.steps.is_empty(), "no stage runs on the gate hold");

    // The human escape mirrors `devai task ratify-empty-rework`: a stored
    // operator decision for exactly this pair, the question release, then the
    // SAME core transition admits the passage citing the decision (REQ-RG-03).
    let d = Decision {
        id: new_id(),
        task_id: t.id,
        kind: decision_kind::REWORK_NOOP_RATIFIED.into(),
        verdict: Verdict::Approved,
        actor: "operator".into(),
        rationale: Some("правок не потребовалось — пункты уже были удовлетворены".into()),
        refs: vec![prev_id.clone(), cur_id.clone()],
        created_at: Utc::now(),
    };
    store.record_decision(&d).unwrap();
    store
        .answer_question(open[0].id, "operator ratified", "operator", Utc::now())
        .unwrap();
    let m = workflow::answer_question(&held, open[0].id, "operator", Utc::now()).unwrap();
    store.commit(held.lock_version, &m).unwrap();
    let cur = store.get_task(&t.id.to_string()).unwrap();
    assert_eq!(cur.state, TaskState::Rework);

    let arts = store.artifacts(t.id).unwrap();
    let current = arts
        .iter()
        .find(|a| a.id.to_string() == cur_id)
        .unwrap()
        .clone();
    let facts = devai_runtime::gate::collect_implementation_report_facts(
        &store,
        t.id,
        &current,
        &arts,
        Some("empty rework ratified".into()),
    )
    .unwrap();
    assert_eq!(
        facts.ratifications.len(),
        1,
        "the collector surfaces only the human's ratifications"
    );
    let m = workflow::implementation_reported(&cur, &facts, "operator", Utc::now()).unwrap();
    assert_eq!(
        m.task.state,
        TaskState::Qa,
        "the ratified no-op passage reaches QA"
    );
    assert_eq!(
        m.events[0].payload[json_field::RATIFIED_BY],
        d.id.to_string(),
        "the transition cites the decision (AC3)"
    );
    store.commit(cur.lock_version, &m).unwrap();
    assert_eq!(
        store.get_task(&t.id.to_string()).unwrap().state,
        TaskState::Qa
    );
}

#[test]
fn rework_with_new_commit_still_reaches_qa() {
    // REQ-RG-06 green path: a rework that ACTUALLY commits work passes the
    // gate untouched — the comparison ran and said DISTINCT; no trip, no
    // hold, no delay.
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    let head = git_out(&ws, &["rev-parse", "HEAD"]);
    let t = Task::new(
        "Real rework",
        "a rework with new work must pass",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    drive_to_report(&store, &t, &ws);
    let cur = store.get_task(&t.id.to_string()).unwrap();
    let m = workflow::return_to_rework(&cur, Some("fix the points"), "tester", Utc::now()).unwrap();
    store.commit(cur.lock_version, &m).unwrap();

    // Real work lands in the task's worktree; the platform commit captures it
    // on the task branch → the new report names a NEW tip.
    let wt = _d.path().join("worktrees").join(t.id.to_string());
    std::fs::write(wt.join("reworked.txt"), b"the fixes QA asked for\n").unwrap();

    let engine = engine_in(&store, &ws, false);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(
        r.gate,
        Gate::WaitingAccept,
        "new work runs through the gate to the auditor"
    );
    assert_eq!(
        store.get_task(&t.id.to_string()).unwrap().state,
        TaskState::Qa
    );
    let events = store.all_events_for_task(t.id).unwrap();
    assert!(
        events
            .iter()
            .all(|e| e.kind != event_kind::QA_REWORK_GATE_TRIPPED),
        "the gate never fired for real work"
    );
    let reported = events
        .iter()
        .rev()
        .find(|e| e.kind == event_kind::IMPLEMENTATION_REPORTED)
        .unwrap();
    assert_eq!(
        reported.payload[json_field::VERDICT],
        "DISTINCT",
        "the gate ran its comparison"
    );
    assert_ne!(
        reported.payload[json_field::CURRENT_COMMIT],
        head,
        "the new tip is not the bound head"
    );
    assert_eq!(
        reported.payload[json_field::PREV_COMMIT],
        head,
        "the compared predecessor was the first report's tip"
    );
}

// ---------------------------------------------------------------------------
// REWORK CONTEXT delivery — the engine executes the declarative
// `config/workflow/rework-context.yaml` table: rationales read VERBATIM from
// the append-only record, findings quoted AS STORED (no read-side
// recomputation), the window cutting at the role's last COMPLETED execution,
// sovereignty (roles never learn it) and inertness (no rework ⇒ no section,
// byte-identical prompts) intact. Ported from the wip/pre-t2-merge delivery
// tests and re-based onto main's typed quality-report model.
// ---------------------------------------------------------------------------

use devai_runtime::evaluation::{
    CheckOutcome, EvaluationBlock, GateOutcome, ProbeState, ProfileIdentity, QualityReportDoc,
};
use devai_runtime::{REWORK_CONTEXT_LABEL, RuntimeError, build_user_prompt, rework_context};
use devai_store::config::{ReworkPolicy, factory_rework_policies, load_rework_policies};

/// The table exactly as the platform ships it — the same bytes the engine
/// falls back to, so every executor assertion below runs on factory policy.
fn policy() -> Vec<ReworkPolicy> {
    factory_rework_policies()
}

/// A QA return as the transition persists it: `quality.returned` carrying
/// the human's rationale text on the payload.
fn returned(store: &Store, task_id: Id, text: &str) {
    store
        .insert_event(
            Some(task_id),
            event_kind::QUALITY_RETURNED,
            "tester",
            serde_json::json!({ (json_field::RATIONALE): text }),
        )
        .unwrap();
}

/// The role's last COMPLETED finish as the store writes it (payload carries
/// `role` + `status` — the window cutoff reads exactly those stored fields).
fn completed(store: &Store, task_id: Id, role: &str) {
    store.insert_event(Some(task_id), event_kind::EXECUTION_FINISHED, &format!("role:{role}"), serde_json::json!({ (json_field::ROLE): role, (json_field::STATUS): ExecutionStatus::Completed.as_str() })).unwrap();
}

fn craft_report(store: &Store, task_id: Id, content: serde_json::Value, version: u32) {
    store
        .publish_artifact(ArtifactDraft {
            task_id,
            kind: ArtifactKind::QualityReport,
            name: "quality-report".into(),
            category: DataCategory::Artifact,
            content: content.to_string().into_bytes(),
            media_type: "application/json".into(),
            contract: Some(ContractRef {
                name: "quality-report".into(),
                version,
            }),
            created_by: "tester".into(),
            execution_id: None,
            depends_on: vec![],
        })
        .unwrap();
}

fn mk_check(
    id: &str,
    verify: &str,
    state: &str,
    score: Option<f64>,
    floor: Option<f64>,
    weight: f64,
    violations: Vec<String>,
) -> CheckOutcome {
    CheckOutcome {
        id: id.into(),
        verify: verify.into(),
        state: state.into(),
        score,
        floor,
        weight,
        detects: vec!["process/untested-branch".into()],
        violations,
        evidence: vec![format!("crafted evidence for {id}")],
        reason: String::new(),
    }
}

/// A @4-shaped failing report built THROUGH the one typed model `assemble`
/// emits — two FAIL gates, one PASS gate, a MECHANICAL_FAIL check carrying
/// the marker violation, and the stored red-line list.
fn failing_doc() -> QualityReportDoc {
    QualityReportDoc {
        verdict: "RETURN".into(),
        recommendation_basis: "красные линии: spec-coverage=0.40<floor 0.70".into(),
        report_text: "crafted for delivery tests".into(),
        assessment_execution: "exec-crafted".into(),
        profile: ProfileIdentity {
            project: "devai".into(),
            version: 1,
        },
        workspace_revision: None,
        evaluation: EvaluationBlock {
            verdict: "FAIL".into(),
            score: 0.4,
            theta: 0.8,
            blocked_weight: 0.0,
            max_blocked_weight: 0.2,
            floors_breached: vec!["spec-coverage=0.40<floor 0.70".into()],
            pending_human: vec![],
            gates: vec![
                GateOutcome {
                    id: "build-green".into(),
                    state: ProbeState::Fail,
                    detail: "cargo build failed".into(),
                },
                GateOutcome {
                    id: "tests-green".into(),
                    state: ProbeState::Fail,
                    detail: String::new(),
                },
                GateOutcome {
                    id: "no-network".into(),
                    state: ProbeState::Pass,
                    detail: String::new(),
                },
            ],
            checks: vec![
                mk_check(
                    "arch/no-cycles",
                    "mechanical",
                    "MECHANICAL_FAIL",
                    Some(0.0),
                    Some(1.0),
                    0.5,
                    vec!["FINDING-MARKER-4e7b: context module crosses the layer boundary".into()],
                ),
                mk_check(
                    "spec-coverage",
                    "rubric",
                    "SCORED",
                    Some(0.40),
                    Some(0.70),
                    0.3,
                    vec![],
                ),
                mk_check(
                    "mech-green",
                    "mechanical",
                    "MECHANICAL_PASS",
                    Some(1.0),
                    None,
                    0.2,
                    vec![],
                ),
            ],
        },
    }
}

#[test]
fn qa_return_delivers_the_verbatim_rationale_and_stored_findings() {
    let (_d, store) = store();
    let t = Task::new(
        "Delivery",
        "the rework role must see the return text and the failing items",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    craft_report(
        &store,
        t.id,
        serde_json::to_value(failing_doc()).unwrap(),
        4,
    );
    returned(
        &store,
        t.id,
        "WIRING: контекст не доставлялся — чини доставку",
    );

    let section = rework_context(&store, &t, "implementation-agent", &policy())
        .expect("an inert read of a stored body never fails")
        .expect("a return against this role delivers a section");
    assert!(
        section.starts_with("Platform-assembled rejection context"),
        "the preamble states the data-only rule FIRST: {section}"
    );
    assert!(
        section.contains(
            "DECISION RATIONALES since the last successful `implementation-agent` execution"
        ),
        "window header names the role"
    );
    // verbatim text, provenance line naming the recorded event and its actor
    assert!(
        section.contains("WIRING: контекст не доставлялся — чини доставку"),
        "rationale quoted verbatim"
    );
    assert!(
        section.contains("[R1] quality.returned by tester at "),
        "the line names the event kind and author"
    );
    // findings quoted as stored — attach_findings fires on THIS trigger
    assert!(
        section.contains("QUALITY REPORT FINDINGS"),
        "the QA return flow also quotes the report"
    );
    assert!(
        section.contains("FINDING-MARKER-4e7b"),
        "the stored MECHANICAL_FAIL violation text is quoted"
    );
    assert!(
        section.contains("spec-coverage=0.40<floor 0.70"),
        "the stored red-line string is quoted verbatim"
    );
    assert!(
        section.contains("- gate `build-green`: FAIL — cargo build failed"),
        "stored FAIL gate + detail"
    );
    assert!(
        section.contains("- gate `tests-green`: FAIL\n"),
        "an empty stored detail adds no dash"
    );
    assert!(
        !section.contains("no-network"),
        "the PASS gate is not a failing item and is never quoted"
    );
}

#[test]
fn the_window_cuts_at_this_roles_last_completed_execution() {
    let (_d, store) = store();
    let t = Task::new(
        "Window",
        "old rejections must not haunt a role that already succeeded",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    returned(
        &store,
        t.id,
        "OLD-MARKER-a11 — answered by the completed rework",
    );
    completed(&store, t.id, "implementation-agent");
    returned(&store, t.id, "NEW-MARKER-b77");
    returned(&store, t.id, "NEW2-MARKER-c99");

    let s1 = rework_context(&store, &t, "implementation-agent", &policy())
        .unwrap()
        .unwrap();
    assert!(
        !s1.contains("OLD-MARKER-a11"),
        "rationales at or before the last COMPLETED finish are out of the window: {s1}"
    );
    let (i_new, i_new2) = (
        s1.find("NEW-MARKER-b77").unwrap(),
        s1.find("NEW2-MARKER-c99").unwrap(),
    );
    assert!(
        i_new < i_new2,
        "inside the window the order is seq order, oldest first"
    );
    assert!(
        s1.contains("[R1]") && s1.contains("[R2]") && !s1.contains("[R3]"),
        "two in-window returns, numbered"
    );

    // Byte-stability: identical store state assembles an identical section.
    let s2 = rework_context(&store, &t, "implementation-agent", &policy())
        .unwrap()
        .unwrap();
    assert_eq!(s1, s2, "determinism: same state => byte-identical section");

    // Another role's success does NOT move this role's cutoff.
    completed(&store, t.id, "quality-auditor");
    let s3 = rework_context(&store, &t, "implementation-agent", &policy())
        .unwrap()
        .unwrap();
    assert_eq!(
        s1, s3,
        "the cutoff is located per declared role, on `execution.finished` role+status"
    );
}

#[test]
fn a_never_succeeded_role_receives_every_recorded_trigger_oldest_first() {
    let (_d, store) = store();
    let t = Task::new(
        "NoSuccess",
        "no COMPLETED row yet => the whole recorded flow is in scope",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    craft_report(
        &store,
        t.id,
        serde_json::to_value(failing_doc()).unwrap(),
        4,
    );
    store
        .insert_event(
            Some(t.id),
            event_kind::REQUIREMENTS_REJECTED,
            "tester",
            serde_json::json!({ (json_field::RATIONALE): "SPEC-REJ-1" }),
        )
        .unwrap();
    store
        .insert_event(
            Some(t.id),
            event_kind::REQUIREMENTS_REJECTED,
            "tester",
            serde_json::json!({ (json_field::RATIONALE): "   " }),
        )
        .unwrap();
    store.insert_event(Some(t.id), event_kind::REVIEW_RECORDED, "reviewer", serde_json::json!({ (json_field::VERDICT): "NEEDS_REVISION", (json_field::SUMMARY): "REVIEW-REJ-2" })).unwrap();
    store.insert_event(Some(t.id), event_kind::REVIEW_RECORDED, "reviewer", serde_json::json!({ (json_field::VERDICT): "APPROVED", (json_field::SUMMARY): "APPROVED-SUMMARY-must-not-appear" })).unwrap();

    let s = rework_context(&store, &t, "requirements-analyst", &policy())
        .unwrap()
        .unwrap();
    assert!(
        s.contains("SPEC-REJ-1") && s.contains("REVIEW-REJ-2"),
        "both rationale sources (approval rejection + review rejection) reach the analyst"
    );
    assert!(
        s.find("SPEC-REJ-1").unwrap() < s.find("REVIEW-REJ-2").unwrap(),
        "oldest first"
    );
    assert!(
        !s.contains("APPROVED-SUMMARY"),
        "the verdict filter drops APPROVED reviews — only non-approved records feed"
    );
    assert!(
        s.contains("[R1]") && s.contains("[R2]") && !s.contains("[R3]"),
        "a blank stored payload contributes no line; numbering stays dense"
    );
    assert!(
        !s.contains("QUALITY REPORT FINDINGS"),
        "attach_findings is per-trigger: the analyst flow quotes no findings even against a failing report"
    );
}

#[test]
fn delivery_follows_the_declared_table_not_code() {
    let (_d, store) = store();
    let wf = store.root().join("config/workflow");
    // Swap the table: now the ANALYST reacts to QA returns (with findings)
    // and the implementer is off the list. No Rust changed — delivery moved.
    std::fs::write(
        wf.join("rework-context.yaml"),
        "- role: requirements-analyst\n  triggers:\n    - event: quality.returned\n      rationale_field: rationale\n      attach_findings: true\n",
    )
    .unwrap();
    let swapped = load_rework_policies(&wf).unwrap();
    let t = Task::new(
        "PolicyData",
        "which role reacts is data, not code",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    craft_report(
        &store,
        t.id,
        serde_json::to_value(failing_doc()).unwrap(),
        4,
    );
    returned(&store, t.id, "POLICY-SWAP-MARKER");

    let a = rework_context(&store, &t, "requirements-analyst", &swapped)
        .unwrap()
        .unwrap();
    assert!(
        a.contains("POLICY-SWAP-MARKER") && a.contains("QUALITY REPORT FINDINGS"),
        "the declared role now receives both"
    );
    assert!(
        rework_context(&store, &t, "implementation-agent", &swapped)
            .unwrap()
            .is_none(),
        "the undeclared role now receives nothing"
    );

    // The factory table says the opposite — proving the move came from data.
    let f = rework_context(
        &store,
        &t,
        "implementation-agent",
        &factory_rework_policies(),
    )
    .unwrap()
    .unwrap();
    assert!(f.contains("POLICY-SWAP-MARKER"));
    assert!(
        rework_context(
            &store,
            &t,
            "requirements-analyst",
            &factory_rework_policies()
        )
        .unwrap()
        .is_none()
    );
}

#[test]
fn report_shape_drift_is_an_explicit_error_never_empty_quotes() {
    let (_d, store) = store();
    let t = Task::new(
        "Drift",
        "a body the model cannot recognise must be named, not hushed",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    returned(&store, t.id, "DRIFT-MARKER");
    // A CURRENT @4 body that does not fit the typed model: loud Err, never
    // a rationale section silently missing its findings or faking them.
    craft_report(
        &store,
        t.id,
        serde_json::json!({ "verdict": "RETURN", "evaluation": "not-an-object" }),
        4,
    );
    let err = rework_context(&store, &t, "implementation-agent", &policy()).unwrap_err();
    assert!(
        matches!(err, RuntimeError::ReportShape(_)),
        "typed shape error: {err}"
    );
    assert!(
        err.to_string().contains("cannot recognise"),
        "the message names the disease: {err}"
    );

    // A legacy @2 CURRENT report — even one carrying an `evaluation` key —
    // is dispatched by the STORED contract ref, not by sniffing content:
    // nothing is parsed from it, the rationale-only section still stands.
    craft_report(
        &store,
        t.id,
        serde_json::json!({ "verdict": "RETURN", "evaluation": { "verdict": "FAIL" }, "findings": ["LEGACY-MUST-NOT-BE-QUOTED"] }),
        2,
    );
    let s = rework_context(&store, &t, "implementation-agent", &policy())
        .unwrap()
        .unwrap();
    assert!(
        s.contains("DRIFT-MARKER"),
        "rationale delivery survives a legacy report"
    );
    assert!(
        !s.contains("QUALITY REPORT FINDINGS") && !s.contains("LEGACY-MUST-NOT-BE-QUOTED"),
        "the @2 machine block (if any) predates the delivery contract: quoted NOTHING"
    );
}

#[test]
fn stored_findings_are_quoted_never_rederived_on_read() {
    let (_d, store) = store();
    let t = Task::new(
        "NoRecompute",
        "which items fail is the assembler's answer, stored",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    returned(&store, t.id, "RECOMPUTE-MARKER");
    // Report A: a SCORED check deep under its stored floor, but the stored
    // red-line list says nothing was breached and states are green. The
    // reader re-runs NO predicate — `score < floor` is not its call.
    let mut doc_a = failing_doc();
    doc_a.evaluation.verdict = "PASS".into();
    doc_a.evaluation.floors_breached = vec![];
    doc_a.evaluation.gates = vec![GateOutcome {
        id: "build-green".into(),
        state: ProbeState::Pass,
        detail: String::new(),
    }];
    doc_a.evaluation.checks = vec![mk_check(
        "low-scored",
        "rubric",
        "SCORED",
        Some(0.20),
        Some(0.90),
        1.0,
        vec![],
    )];
    craft_report(&store, t.id, serde_json::to_value(&doc_a).unwrap(), 4);
    let s = rework_context(&store, &t, "implementation-agent", &policy())
        .unwrap()
        .unwrap();
    assert!(s.contains("RECOMPUTE-MARKER"));
    assert!(
        !s.contains("low-scored"),
        "a stored SCORED value is never re-judged against its floor on read: {s}"
    );
    assert!(
        !s.contains("QUALITY REPORT FINDINGS"),
        "the stored list says nothing failing: no invented sub-block"
    );

    // Report B: all-green states but a NON-empty stored red-line list —
    // the stored strings are quoted verbatim, authority rests with what
    // was decided once at assembly.
    let mut doc_b = doc_a.clone();
    doc_b.evaluation.floors_breached = vec!["ghost/check=0.10<floor 0.50".into()];
    craft_report(&store, t.id, serde_json::to_value(&doc_b).unwrap(), 4);
    let s = rework_context(&store, &t, "implementation-agent", &policy())
        .unwrap()
        .unwrap();
    assert!(
        s.contains("QUALITY REPORT FINDINGS"),
        "the stored list is the answer — quoted even against green states"
    );
    assert!(
        s.contains("- ghost/check=0.10<floor 0.50"),
        "verbatim, in stored form"
    );
}

#[test]
fn a_first_run_gets_no_empty_section_and_qa_wait_adds_nothing() {
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    let t = Task::new(
        "Inert",
        "no rejections recorded: prompts stay byte-identical",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    for role in [
        "requirements-analyst",
        "requirements-reviewer",
        "implementation-agent",
        "quality-auditor",
    ] {
        assert!(
            rework_context(&store, &t, role, &policy())
                .unwrap()
                .is_none(),
            "{role}: nothing recorded => NO section at all (R-008)"
        );
    }

    drive_to_report(&store, &t, &ws);
    // Every stage completed; the task waits at QA with an approved review
    // and a report — NONE of that is a rejection: still no section anywhere.
    for role in [
        "requirements-analyst",
        "requirements-reviewer",
        "implementation-agent",
        "quality-auditor",
    ] {
        assert!(
            rework_context(&store, &t, role, &policy())
                .unwrap()
                .is_none(),
            "{role}: a QA WAIT and a green cycle add nothing"
        );
    }
    // Engine-level proof: the FIRST implementation execution's body shows
    // its prompt carried no REWORK CONTEXT label at all.
    let arts = store.artifacts(t.id).unwrap();
    let rep = arts
        .iter()
        .find(|a| a.kind == ArtifactKind::ImplementationReport)
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&store.read_blob(&rep.sha256).unwrap()).unwrap();
    assert!(
        !body["notes"]
            .as_str()
            .unwrap()
            .contains(REWORK_CONTEXT_LABEL),
        "first-run prompt gained no empty section: {}",
        body["notes"]
    );
}

#[test]
fn the_rework_execution_actually_receives_the_assembled_section() {
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    let t = Task::new(
        "RealRework",
        "the delivered context must reach the real rework prompt",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    drive_to_report(&store, &t, &ws);
    let cur = store.get_task(&t.id.to_string()).unwrap();
    let m = workflow::return_to_rework(
        &cur,
        Some("THE-WIRING-MARKER — return text as the human wrote it"),
        "tester",
        Utc::now(),
    )
    .unwrap();
    store.commit(cur.lock_version, &m).unwrap();

    let engine = engine_in(&store, &ws, false);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.steps.len(), 1, "the rework stage ran");
    // The mock echoes input-section LABELS into the report body — the
    // ratified observation surface; contents are pinned by the assertions
    // above, the label here proves the ENGINE wires the section in.
    let arts = store.artifacts(t.id).unwrap();
    let rep = arts
        .iter()
        .find(|a| {
            a.kind == ArtifactKind::ImplementationReport && a.status == ArtifactStatus::Current
        })
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&store.read_blob(&rep.sha256).unwrap()).unwrap();
    assert!(
        body["notes"]
            .as_str()
            .unwrap()
            .contains(REWORK_CONTEXT_LABEL),
        "the rework execution's prompt carried the section: {}",
        body["notes"]
    );
    // And the content delivered to that very execution is the verbatim
    // return text plus the real report body read through the typed model
    // (the engine-assembled @4 parses — producer/consumer share one shape).
    let section = rework_context(&store, &t, "implementation-agent", &policy())
        .unwrap()
        .unwrap();
    assert!(
        section.contains("THE-WIRING-MARKER — return text as the human wrote it"),
        "verbatim"
    );
    // The rework execution row stays bound to a workspace revision as before.
    let impl_execs: Vec<_> = store
        .list_executions(Some(t.id))
        .unwrap()
        .into_iter()
        .filter(|e| e.role_slug == "implementation-agent")
        .collect();
    assert_eq!(impl_execs.len(), 2, "first pass + rework");
    assert!(
        impl_execs[1].workspace_revision.is_some(),
        "the delivered-context execution is still revision-bound"
    );
}

#[test]
fn an_absent_workflow_config_still_delivers_from_the_factory_bytes() {
    let (_d, store) = store();
    std::fs::remove_dir_all(store.root().join("config/workflow")).unwrap();
    // Absent directory is the FALLBACK signal, never an error and never a
    // silent delivery-less home (R-007): the engine re-reads the embedded
    // factory bytes — the same table, not a code copy of the map.
    let loaded = load_rework_policies(&store.root().join("config/workflow")).unwrap();
    assert!(
        loaded.is_empty(),
        "absent dir => empty => caller falls back"
    );
    let (_wsdir, ws) = git_workspace();
    let t = Task::new(
        "Fallback",
        "a home predating the file still gets delivery",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    drive_to_report(&store, &t, &ws); // the whole cycle runs on a home WITHOUT the workflow dir
    let cur = store.get_task(&t.id.to_string()).unwrap();
    let m = workflow::return_to_rework(&cur, Some("FACTORY-FALLBACK-MARKER"), "tester", Utc::now())
        .unwrap();
    store.commit(cur.lock_version, &m).unwrap();
    let engine = engine_in(&store, &ws, false);
    engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    let arts = store.artifacts(t.id).unwrap();
    let rep = arts
        .iter()
        .find(|a| {
            a.kind == ArtifactKind::ImplementationReport && a.status == ArtifactStatus::Current
        })
        .unwrap();
    let body: serde_json::Value =
        serde_json::from_slice(&store.read_blob(&rep.sha256).unwrap()).unwrap();
    assert!(
        body["notes"]
            .as_str()
            .unwrap()
            .contains(REWORK_CONTEXT_LABEL),
        "delivery moved on the factory fallback, not silently disabled: {}",
        body["notes"]
    );
    // The fallback table is the DECLARED table — attach_findings intact.
    let s = rework_context(
        &store,
        &t,
        "implementation-agent",
        &factory_rework_policies(),
    )
    .unwrap()
    .unwrap();
    assert!(s.contains("FACTORY-FALLBACK-MARKER"));
}

#[test]
fn present_but_malformed_workflow_config_is_a_loud_error() {
    let (_d, store) = store();
    let wf = store.root().join("config/workflow");
    std::fs::write(wf.join("zz-typo.yaml"), "- role: implementation-agent\n  triggers:\n    - event: quality.retruned\n      rationale_field: rationale\n").unwrap();
    let (_wsdir, ws) = git_workspace();
    let t = Task::new(
        "LoudConfig",
        "a broken table must stop delivery loudly",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    // Construction still succeeds — Engine::new is infallible by contract…
    let engine = Engine::new(&store)
        .with_workspace(ws.to_path_buf())
        .with_rework_config_dir(&wf);
    // …but the first stage that could deliver fails EXPLICITLY, naming the
    // file and the offending value — never masquerading as "nothing to say".
    let err = engine
        .advance(&t.id.to_string(), "mock", "tester")
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("zz-typo.yaml") && msg.contains("quality.retruned"),
        "the operator is told what to fix: {msg}"
    );
}

#[test]
fn delivered_context_is_inert_data_and_assembly_is_read_only() {
    let (_d, store) = store();
    let t = Task::new(
        "ReadOnly",
        "assembly must not move a single stored byte",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    craft_report(
        &store,
        t.id,
        serde_json::to_value(failing_doc()).unwrap(),
        4,
    );
    returned(&store, t.id, "INERT-MARKER");

    let events_before = store.all_events_for_task(t.id).unwrap();
    let artifacts_before = store.artifacts(t.id).unwrap();
    let decisions_before = store.list_decisions(t.id).unwrap();
    let lock_before = store.get_task(&t.id.to_string()).unwrap().lock_version;

    let s1 = rework_context(&store, &t, "implementation-agent", &policy())
        .unwrap()
        .unwrap();
    let s2 = rework_context(&store, &t, "implementation-agent", &policy())
        .unwrap()
        .unwrap();
    assert_eq!(s1, s2, "byte-stable");

    assert_eq!(
        store.all_events_for_task(t.id).unwrap().len(),
        events_before.len(),
        "delivery writes no events"
    );
    assert_eq!(
        store.artifacts(t.id).unwrap().len(),
        artifacts_before.len(),
        "delivery publishes no artifacts"
    );
    assert_eq!(
        store.list_decisions(t.id).unwrap().len(),
        decisions_before.len(),
        "delivery records no decisions"
    );
    assert_eq!(
        store.get_task(&t.id.to_string()).unwrap().lock_version,
        lock_before,
        "delivery moves no state and bumps no lock"
    );

    // The section rides as ONE MORE input label; the output-contract tail
    // is byte-identical with or without it — quoted text changes what the
    // role READS, never what it must PRODUCE (R-007 AC4).
    let base = vec![("REQUIREMENTS_SPEC v1".to_string(), "spec body".to_string())];
    let with = {
        let mut v = base.clone();
        v.push((REWORK_CONTEXT_LABEL.to_string(), s1.clone()));
        v
    };
    let p0 = build_user_prompt(
        "T",
        "B",
        &base,
        Some(("implementation-report", 2)),
        Some("schema"),
    );
    let p1 = build_user_prompt(
        "T",
        "B",
        &with,
        Some(("implementation-report", 2)),
        Some("schema"),
    );
    let tail = |p: &str| p[p.find("OUTPUT CONTRACT").unwrap()..].to_string();
    assert_eq!(
        tail(&p0),
        tail(&p1),
        "the contract block is untouched by delivery"
    );
    assert!(
        p1.contains("\n--- REWORK CONTEXT ---\n"),
        "rendered as a plain input section"
    );
    assert!(
        s1.starts_with("Platform-assembled rejection context"),
        "the preamble itself states the data-only rule"
    );
}

#[test]
fn the_empty_rework_gate_trips_the_return_text_into_the_next_window() {
    // Coverage block: the gate parks the task with a QUESTION instead of a
    // fresh return, so without the declared `qa.rework_gate_tripped` trigger
    // a gate-demanded rework would arrive with no rationale at all. The gate
    // event lands AFTER the role's last COMPLETED (its own finish precedes
    // the publish/gate commit), carrying the cycle's original return text.
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    let t = Task::new(
        "GateCover",
        "gate tripped by an identical rework must hand the same text forward",
        None,
        Utc::now(),
    );
    store.create_task(&t).unwrap();
    drive_to_report(&store, &t, &ws);
    let cur = store.get_task(&t.id.to_string()).unwrap();
    let m = workflow::return_to_rework(
        &cur,
        Some("GATE-CYCLE-RATIONALE — п1; п2"),
        "tester",
        Utc::now(),
    )
    .unwrap();
    store.commit(cur.lock_version, &m).unwrap();

    // The noop rework runs and the gate trips (task parks).
    let engine = engine_in(&store, &ws, false);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.steps.len(), 1, "the rework stage ran");
    assert_eq!(
        r.gate,
        Gate::Hold(TaskState::WaitingForInput),
        "the gate parked the cycle"
    );
    let held = store.get_task(&t.id.to_string()).unwrap();

    // The OLD return is now behind the rework's COMPLETED row — out of the
    // window. The gate event is the carrier that puts it back IN: the next
    // execution of the role must see the cycle's text again.
    let section = rework_context(&store, &held, "implementation-agent", &policy())
        .unwrap()
        .unwrap();
    assert!(
        section.contains("GATE-CYCLE-RATIONALE — п1; п2"),
        "the gate trigger re-delivers the cycle's rationale verbatim"
    );
    assert!(
        section.contains(event_kind::QA_REWORK_GATE_TRIPPED),
        "and names the gate event as its source"
    );
    let rationales: Vec<_> = section.match_indices("[R").collect();
    assert_eq!(
        rationales.len(),
        1,
        "exactly one carrier — the old quality.returned is cut, the gate supplies its text (no double-quote)"
    );
}
