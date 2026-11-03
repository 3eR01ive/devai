//! End-to-end proof of the minimal process: the engine drives a real task
//! through every autonomous stage with role executions, stops at both human
//! gates, and cannot pass them — offline, deterministically, on `mock`.

use std::path::PathBuf;

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
            &["-c", "user.email=devai@test.local", "-c", "user.name=devai-test", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "bound state"],
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
    Engine::new(store).with_workspace(workspace()).with_profile(tiny_profile())
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
    let t = Task::new("Minimal process", "Run devai's own roles end to end", None, Utc::now());
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
    assert_eq!(store.get_task(&t.id.to_string()).unwrap().lock_version, before.lock_version);

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
        let a = arts.iter().find(|a| a.kind == ArtifactKind::QualityReport).unwrap().clone();
        let body = store.read_blob(&a.sha256).unwrap();
        (a, body)
    };
    // The report is pinned to the strengthened contract version.
    assert_eq!(
        qa_artifact.contract.as_ref().map(|c| (c.name.as_str(), c.version)),
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
            let r = e
                .workspace_revision
                .as_ref()
                .unwrap_or_else(|| panic!("{} execution must record a workspace_revision", e.role_slug));
            assert!(r.git_head.is_some(), "{}: git HEAD component", e.role_slug);
            assert!(r.dirty_digest.is_some(), "{}: dirty digest component", e.role_slug);
            assert!(r.profile.as_deref().is_some_and(|p| p.contains('@')), "{}: profile name@version", e.role_slug);
            assert!(!r.roles.is_empty(), "{}: roles slug@version list", e.role_slug);
            assert!(r.is_comparable(), "{}: git workspace → comparable", e.role_slug);
        }
        let qa_exec = execs.iter().find(|e| e.role_slug == "quality-auditor").unwrap();
        let embedded: devai_core::types::WorkspaceRevision =
            serde_json::from_value(rep["workspace_revision"].clone()).expect("@4 embeds workspace_revision");
        assert_eq!(embedded, qa_exec.workspace_revision.clone().unwrap(), "artifact revision == execution row revision");
        assert_eq!(embedded.git_head.as_deref(), revision::git_head(&workspace()).as_deref(), "HEAD == git rev-parse HEAD of the probe workspace");
    }
    assert_eq!(rep["evaluation"]["verdict"], "INCONCLUSIVE");
    assert_eq!(rep["evaluation"]["gates"].as_array().unwrap().len(), 2);
    let states: Vec<&str> = rep["evaluation"]["gates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| g["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["PASS", "BLOCKED"], "blocked probe never folds into PASS");
    assert!(rep["evaluation"]["pending_human"].as_array().unwrap().contains(&serde_json::json!("hum/c")));
    let checks = rep["evaluation"]["checks"].as_array().unwrap();
    assert_eq!(checks.len(), 3);
    assert!(checks.iter().any(|c| c["id"] == "mech/a" && c["state"] == "MECHANICAL_PASS"));
    assert!(checks.iter().any(|c| c["id"] == "rub/b" && c["state"] == "BLOCKED"));
    assert!(checks.iter().any(|c| c["id"] == "hum/c" && c["state"] == "PENDING_HUMAN"));
    // QA rule "evidence per criterion": every criteria entry of the stored
    // report cites concrete evidence — the probe command+exit for mechanical
    // results, the auditor's gap record / pending signature marker otherwise.
    for c in checks.iter() {
        let ev = c["evidence"]
            .as_array()
            .unwrap_or_else(|| panic!("check {} must serialize evidence", c["id"]));
        assert!(
            ev.iter().any(|e| e.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false)),
            "check {} ({}) must cite at least one concrete evidence item, got {ev:?}",
            c["id"],
            c["state"].as_str().unwrap_or("?"),
        );
    }
    assert!(rep["report_text"].as_str().unwrap().starts_with("ВЕРДИКТ: ВЕРДИКТ НЕ ГОТОВ"));
    assert!(r2.steps[1].summary.as_deref().unwrap_or_default().contains("ESCALATE"));

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
    let engine = mk_engine(&store);
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
    assert!(store.get_contract("quality-report", 2).unwrap().is_some(), "quality-report@2 must stay loadable");
    assert!(store.get_contract("quality-report", 3).unwrap().is_some(), "quality-report@3 must stay loadable");
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
    missing["evaluation"]["checks"][1].as_object_mut().unwrap().remove("evidence");
    let err = devai_runtime::validate_against_contract(&missing.to_string(), Some(&schema)).unwrap_err();
    assert!(err.to_string().contains("$.evaluation.checks[1].evidence: missing required key"), "{err}");

    // ...and so is an empty evidence array.
    let mut empty = good.clone();
    empty["evaluation"]["checks"][0]["evidence"] = serde_json::json!([]);
    let err = devai_runtime::validate_against_contract(&empty.to_string(), Some(&schema)).unwrap_err();
    assert!(err.to_string().contains("$.evaluation.checks[0].evidence: array has 0 item(s), minimum 1"), "{err}");

    // @4 binding: a report without the workspace_revision block is rejected...
    let mut unbound = good.clone();
    unbound.as_object_mut().unwrap().remove("workspace_revision");
    let err = devai_runtime::validate_against_contract(&unbound.to_string(), Some(&schema)).unwrap_err();
    assert!(err.to_string().contains("$.workspace_revision: missing required key"), "{err}");

    // ...and so is a block missing any of its four components (nulls allowed,
    // absence not).
    let mut half = good.clone();
    half["workspace_revision"].as_object_mut().unwrap().remove("roles");
    let err = devai_runtime::validate_against_contract(&half.to_string(), Some(&schema)).unwrap_err();
    assert!(err.to_string().contains("$.workspace_revision.roles: missing required key"), "{err}");

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
    let t = Task::new("Paused", "Engine must not run roles on a held task", None, Utc::now());
    store.create_task(&t).unwrap();
    let engine = mk_engine(&store);
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
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

fn git_out(dir: &std::path::Path, args: &[&str]) -> String {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git is available");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
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
        &["-c", "user.email=devai@test.local", "-c", "user.name=devai-test", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "bound state"],
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
    assert_eq!(r.gate, Gate::WaitingAccept, "first QA run: gate is inert (no prior report)");
    assert_eq!(r.steps.len(), 2, "implementation + auditor");
}

#[test]
fn qa_reeval_on_unchanged_workspace_is_asked_not_silently_rerun() {
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    let t = Task::new("Freshness", "Re-audit needs an explicit decision", None, Utc::now());
    store.create_task(&t).unwrap();
    drive_to_report(&store, &t, &ws);

    let execs_before = store.list_executions(Some(t.id)).unwrap().len();
    let arts_before = store.artifacts(t.id).unwrap().len();

    // REQ-006: even these MOCK executions bind the real git state — mock and
    // real rows share one shape.
    for e in store.list_executions(Some(t.id)).unwrap() {
        let r = e.workspace_revision.expect("every engine execution records a revision");
        assert!(r.is_comparable(), "{} bound to real git state", e.role_slug);
    }

    // The exact dogfood failure mode: re-run with NOTHING changed. The gate
    // must stop — no execution, no artifact, no token spend, state unmoved.
    let engine = engine_in(&store, &ws, false);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.gate, Gate::ReportFresh, "fresh workspace: auditor must not silently re-run");
    assert!(r.steps.is_empty(), "no stage ran");
    assert_eq!(store.list_executions(Some(t.id)).unwrap().len(), execs_before, "no execution started");
    assert_eq!(store.artifacts(t.id).unwrap().len(), arts_before, "no artifact published");
    assert_eq!(store.get_task(&t.id.to_string()).unwrap().state, TaskState::Qa, "workflow did not move");
    let kinds: Vec<String> = store.all_events_for_task(t.id).unwrap().into_iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&event_kind::QA_FRESH_AWAITING_DECISION.to_string()), "the ask is recorded: {kinds:?}");
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
    assert_eq!(store.list_executions(Some(t.id)).unwrap().len(), execs_before + 1);
    // The override is persisted, never silent: a decision row + an event.
    let decs = store.list_decisions(t.id).unwrap();
    assert!(decs.iter().any(|d| d.kind == "qa_reaudit_override"), "override decision recorded");
    let kinds: Vec<String> = store.all_events_for_task(t.id).unwrap().into_iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&event_kind::QA_REAUDIT_FORCED.to_string()), "forced re-audit event: {kinds:?}");
}

#[test]
fn any_workspace_change_marks_the_report_stale_and_reaudit_proceeds() {
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    let t = Task::new("Stale", "changed workspace re-audits without asking", None, Utc::now());
    store.create_task(&t).unwrap();
    drive_to_report(&store, &t, &ws);

    // REQ-WT-02/03: with the task branch live, the audit measures the
    // PLATFORM WORKTREE — that is the state a report binds, so staleness is
    // proven by changing the worktree. (Editing the main checkout instead
    // would now honestly NOT invalidate the report: it measures something
    // else — see req008_dogfood.)
    let wt = _d.path().join("worktrees").join(t.id.to_string());
    assert!(wt.is_dir(), "the writing stage created the worktree under the data home");

    // A tracked edit changes the dirty digest → the report no longer binds
    // the current state → the audit runs normally.
    std::fs::write(wt.join("tracked.txt"), b"beta\n").unwrap();
    let engine = engine_in(&store, &ws, false);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.gate, Gate::WaitingAccept, "stale report: auditor re-runs without an override");
    assert_eq!(r.steps.len(), 1);

    // A NEW untracked (non-ignored) file changes the digest too: the report
    // just published binds the pre-scratch state, so this run is stale again.
    std::fs::write(wt.join("scratch.txt"), b"untracked\n").unwrap();
    let engine = engine_in(&store, &ws, false);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.gate, Gate::WaitingAccept, "untracked addition invalidates freshness");
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
    let t = Task::new("NoGit", "unbound tree cannot host a writing stage", None, Utc::now());
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
    let err = engine.advance(&t.id.to_string(), "mock", "tester").unwrap_err();
    assert!(err.to_string().contains("not a git repository"), "the error names the rule it enforces: {err}");
    let impl_exec = store
        .list_executions(Some(t.id))
        .unwrap()
        .into_iter()
        .find(|e| e.role_slug == "implementation-agent")
        .expect("the writing execution is recorded");
    assert_eq!(impl_exec.status, ExecutionStatus::Failed, "a refused writing stage is honest history");
    assert_eq!(store.get_task(&t.id.to_string()).unwrap().state, TaskState::Implementation, "the refusal moves no state");
    assert!(
        store.artifacts(t.id).unwrap().iter().all(|a| a.kind != ArtifactKind::ImplementationReport),
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
    let m = workflow::implementation_reported(&cur, Some("manual"), "tester", Utc::now()).unwrap();
    store.commit(cur.lock_version, &m).unwrap();

    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.gate, Gate::WaitingAccept);
    let execs = store.list_executions(Some(t.id)).unwrap();
    let qa = execs.iter().find(|e| e.role_slug == "quality-auditor").unwrap();
    let rev = qa.workspace_revision.as_ref().expect("non-git executions still record a revision");
    assert!(rev.git_head.is_none() && rev.dirty_digest.is_none(), "no git → NULL head+digest");
    assert!(!rev.is_comparable());

    // Re-run: the auditor runs again (gate inert), it never claims freshness.
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.gate, Gate::WaitingAccept);
    assert_eq!(r.steps.len(), 1, "non-git workspace: re-audit proceeds, never blocked as fresh");
}

#[test]
fn report_and_execution_rows_never_drift_and_bumps_mark_stale() {
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    let t = Task::new("Drift", "artifact embeds the execution row's revision", None, Utc::now());
    store.create_task(&t).unwrap();
    drive_to_report(&store, &t, &ws);

    // REQ-004: the embedded object equals the stored execution row verbatim.
    let arts = store.artifacts(t.id).unwrap();
    let a = arts.iter().filter(|x| x.kind == ArtifactKind::QualityReport).max_by_key(|x| x.seq).unwrap().clone();
    let rep: serde_json::Value = serde_json::from_slice(&store.read_blob(&a.sha256).unwrap()).unwrap();
    let qa = store
        .list_executions(Some(t.id))
        .unwrap()
        .into_iter()
        .find(|e| e.role_slug == "quality-auditor")
        .unwrap();
    let embedded: devai_core::types::WorkspaceRevision = serde_json::from_value(rep["workspace_revision"].clone()).unwrap();
    assert_eq!(embedded, qa.workspace_revision.clone().unwrap());

    // A role definition bump changes the roles component → stale (A-002:
    // ALL four components must match for fresh). Simulate the init diff-bump:
    // insert a new role version (history row stays).
    let roles = store.list_roles().unwrap();
    let impl_role = roles.iter().find(|r| r.slug == "implementation-agent").cloned().unwrap();
    let mut bumped = impl_role.clone();
    bumped.id = new_id();
    bumped.version = impl_role.version + 1;
    store.put_role(&bumped).unwrap();

    let engine = engine_in(&store, &ws, false);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.gate, Gate::WaitingAccept, "role version bump alone invalidates freshness");
    assert_eq!(r.steps.len(), 1);
}

#[test]
fn profile_version_bump_marks_report_stale() {
    // Freshness compares ALL FOUR components: an evaluation-profile bump
    // (the QA rules themselves changed) invalidates even a bit-identical
    // git tree — the old verdict was rendered under different rules.
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    let t = Task::new("Profile bump", "new rules force a fresh audit", None, Utc::now());
    store.create_task(&t).unwrap();
    drive_to_report(&store, &t, &ws);

    let mut p = tiny_profile();
    p.version = 2;
    let engine = Engine::new(&store).with_workspace(&ws).with_profile(p);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.gate, Gate::WaitingAccept, "profile identity changed: not fresh");
    assert_eq!(r.steps.len(), 1);
    // The new report binds the NEW profile identity.
    let arts = store.artifacts(t.id).unwrap();
    let a = arts.iter().filter(|x| x.kind == ArtifactKind::QualityReport).max_by_key(|x| x.seq).unwrap();
    let rep: serde_json::Value = serde_json::from_slice(&store.read_blob(&a.sha256).unwrap()).unwrap();
    assert_eq!(rep["workspace_revision"]["profile"], "devai@2");
}

#[test]
fn legacy_unbound_report_leaves_the_gate_inert() {
    // @1..@3 reports carry no binding: freshness cannot be CLAIMED for them
    // and must not be DENIED either — the audit simply proceeds, so an
    // existing install upgrades without getting stuck behind a fake ask.
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    let t = Task::new("Legacy", "pre-@4 CURRENT report: gate inert", None, Utc::now());
    store.create_task(&t).unwrap();
    drive_to_report(&store, &t, &ws);

    // Re-publish the CURRENT report as a @3-shaped doc WITHOUT the revision
    // block (the highest seq among CURRENT wins the freshness check).
    let arts = store.artifacts(t.id).unwrap();
    let a = arts.iter().filter(|x| x.kind == ArtifactKind::QualityReport).max_by_key(|x| x.seq).unwrap().clone();
    let mut rep: serde_json::Value = serde_json::from_slice(&store.read_blob(&a.sha256).unwrap()).unwrap();
    rep.as_object_mut().unwrap().remove("workspace_revision");
    store
        .publish_artifact(devai_store::ArtifactDraft {
            task_id: t.id,
            kind: ArtifactKind::QualityReport,
            name: "quality-report (legacy @3 shape)".into(),
            category: DataCategory::Artifact,
            content: rep.to_string().into_bytes(),
            media_type: "application/json".into(),
            contract: Some(ContractRef { name: "quality-report".into(), version: 3 }),
            created_by: "user".into(),
            execution_id: None,
            depends_on: vec![],
        })
        .unwrap();

    let engine = engine_in(&store, &ws, false);
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.gate, Gate::WaitingAccept, "unbound report: audit proceeds, never a freshness ask");
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
    let status_before = git_out(&ws, &["status", "--porcelain=v1", "-z", "--untracked-files=all"]);
    let index_before = git_out(&ws, &["ls-files", "-s"]);

    let t = Task::new("Isolation", "writing runs on a platform branch", None, Utc::now());
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
    assert!(wt.is_dir(), "the platform created the isolation worktree under the data home");
    assert_eq!(git_out(&wt, &["branch", "--show-current"]), branch, "REQ-WT-02: the writing checkout is the task branch");
    assert_eq!(git_out(&wt, &["rev-parse", "HEAD"]), head, "REQ-WT-01 AC1: the branch starts at the bound commit");
    assert!(!wt.join("main-only.txt").exists(), "OQ-3: the main checkout's dirt is not transported");

    // REQ-WT-08 AC2: after a whole writing stage the main checkout is
    // byte-identical — only the two enumerated channels were touched.
    assert_eq!(git_out(&ws, &["rev-parse", "HEAD"]), head, "main HEAD unmoved");
    assert_eq!(git_out(&ws, &["status", "--porcelain=v1", "-z", "--untracked-files=all"]), status_before, "main working tree unmoved");
    assert_eq!(git_out(&ws, &["ls-files", "-s"]), index_before, "main index unmoved");

    // REQ-WT-04: the published report names branch+worktree+commit STAMPED
    // by the platform — the agent's own claims are overwritten with ground
    // truth and the stamped body must pass implementation-report@2 again.
    let arts = store.artifacts(t.id).unwrap();
    let impl_a = arts.iter().find(|a| a.kind == ArtifactKind::ImplementationReport).unwrap();
    assert_eq!(
        impl_a.contract.as_ref().map(|c| (c.name.as_str(), c.version)),
        Some(("implementation-report", 2)),
        "the role's declared @2 contract is what got published"
    );
    let body: serde_json::Value = serde_json::from_slice(&store.read_blob(&impl_a.sha256).unwrap()).unwrap();
    assert_eq!(body["branch"], branch);
    assert_eq!(body["worktree"], wt.display().to_string());
    // The mock changed nothing: the commit that captures the work is the
    // tip that already equals it — no empty commit is faked (REQ-WT-04 AC3).
    assert_eq!(body["commit"], head, "the named commit resolves on the task branch");
    let schema = store.get_contract("implementation-report", 2).unwrap().expect("implementation-report@2 must be seeded");
    assert!(devai_runtime::validate_against_contract(&body.to_string(), Some(&schema)).is_ok(), "stamped body passes @2");

    // REQ-WT-05: the run's evidence sink lives under the home, declared to
    // the adapter — created for the writing execution.
    let impl_exec = store
        .list_executions(Some(t.id))
        .unwrap()
        .into_iter()
        .find(|e| e.role_slug == "implementation-agent")
        .unwrap();
    assert_eq!(impl_exec.status, ExecutionStatus::Completed);
    let ev = _d.path().join("evidence").join(t.id.to_string()).join(impl_exec.id.to_string());
    assert!(ev.is_dir(), "platform-declared evidence dir created for the writing execution");

    // The repository facts are immutable history, not just artifact claims.
    let kinds: Vec<String> = store.all_events_for_task(t.id).unwrap().into_iter().map(|e| e.kind).collect();
    assert!(kinds.contains(&event_kind::WORKTREE_PREPARED.to_string()), "worktree.prepared recorded: {kinds:?}");
    assert!(kinds.contains(&event_kind::WORKTREE_COMMITTED.to_string()), "worktree.committed recorded: {kinds:?}");
}

#[test]
fn qa_probes_measure_the_worktree_not_the_main_checkout() {
    let (_d, store) = store();
    let (_wsdir, ws) = git_workspace();
    let t = Task::new("Probe location", "REQ-WT-03: gates run where the work lives", None, Utc::now());
    store.create_task(&t).unwrap();
    let engine = engine_in(&store, &ws, false);
    engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    approved(&store, &t.id.to_string());

    // A gate that passes ONLY inside the task's branch checkout: run in the
    // main checkout the same command exits 1. A PASS is proof of location.
    let branch = devai_runtime::worktree::branch_name(t.id);
    let engine = Engine::new(&store)
        .with_workspace(ws.clone())
        .with_profile(tiny_profile_gate(&format!("git branch --show-current | grep -qx '{branch}'")));
    let r = engine.advance(&t.id.to_string(), "mock", "tester").unwrap();
    assert_eq!(r.gate, Gate::WaitingAccept, "implementation ran, then the audit measured its worktree");

    let arts = store.artifacts(t.id).unwrap();
    let a = arts.iter().find(|x| x.kind == ArtifactKind::QualityReport).unwrap();
    let rep: serde_json::Value = serde_json::from_slice(&store.read_blob(&a.sha256).unwrap()).unwrap();
    let gates = rep["evaluation"]["gates"].as_array().unwrap();
    assert_eq!(gates[0]["id"], "always-green");
    assert_eq!(gates[0]["state"], "PASS", "REQ-WT-03: the gate probe ran on the task branch — impossible outside the worktree");

    // REQ-WT-05: the transcript is a run artifact under <home>/evidence,
    // referenced from the prompt by path; nothing lands in either checkout
    // (the 2026-11-01 dogfood wrote an evidence file into the source tree).
    let qa_exec = store
        .list_executions(Some(t.id))
        .unwrap()
        .into_iter()
        .find(|e| e.role_slug == "quality-auditor")
        .unwrap();
    let transcript = _d.path().join("evidence").join(t.id.to_string()).join(qa_exec.id.to_string()).join("probes.txt");
    let txt = std::fs::read_to_string(&transcript).expect("probe transcript persisted under the evidence root");
    assert!(txt.contains("git branch --show-current"), "the transcript cites the command: {txt}");
    assert!(!ws.join("probes.txt").exists(), "no run artifact in the main checkout");
    assert!(!_d.path().join("worktrees").join(t.id.to_string()).join("probes.txt").exists(), "no run artifact in the worktree");
}
