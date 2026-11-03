//! REQ-RG-01..06 through the real `devai` binary — the 04.11 precedent
//! (01a1a14e-2c07) played back end to end: QA returned three concrete
//! points, the REWORK execution brought nothing (the report names the SAME
//! platform-stamped commit as before), and the platform carried it silently
//! into QA. The gate makes that passage loud — the task parks on a blocking,
//! answerable question with an immutable trip event, and the ONLY way into QA
//! over the unchanged commit is the human `task ratify-empty-rework` escape,
//! whose decision the transition event cites. The ratification belongs to its
//! cycle's report pair: the next unchanged rework trips the gate again.

use std::path::Path;
use std::process::Command;

use devai_core::types::{decision_kind, event_kind, json_field};
use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("rework-operator")
        .env_remove("DEVAI_HOME")
        .env_remove("DEVAI_CONFIG")
        .args(args)
        .output()
        .expect("the devai binary is runnable");
    assert!(
        out.status.success(),
        "devai {args:?} exited {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn devai_json(home: &Path, args: &[&str]) -> serde_json::Value {
    let mut full = vec!["--json"];
    full.extend(args.iter().copied());
    let s = devai(home, &full);
    serde_json::from_str(s.trim()).unwrap_or_else(|e| panic!("devai {args:?} --json must print JSON ({e}): {s}"))
}

fn devai_fail(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("rework-operator")
        .env_remove("DEVAI_HOME")
        .env_remove("DEVAI_CONFIG")
        .args(args)
        .output()
        .expect("the devai binary is runnable");
    assert!(!out.status.success(), "devai {args:?} must fail, got stdout: {}", String::from_utf8_lossy(&out.stdout));
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .expect("git is available");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

fn git_ok(dir: &Path, args: &[&str]) {
    let out = Command::new("git").args(args).current_dir(dir).output().expect("git is available");
    assert!(out.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&out.stderr));
}

#[test]
fn unchanged_rework_parks_loudly_and_only_a_human_ratifies_the_no_op() {
    let home = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();
    git_ok(ws.path(), &["init", "-q"]);
    std::fs::write(ws.path().join("tracked.txt"), b"alpha\n").unwrap();
    git_ok(ws.path(), &["add", "tracked.txt"]);
    git_ok(
        ws.path(),
        &["-c", "user.email=devai@test.local", "-c", "user.name=devai-test", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "baseline"],
    );
    let head = git(ws.path(), &["rev-parse", "HEAD"]);

    devai(home.path(), &["init"]);
    let created = devai_json(home.path(), &["task", "new", "Empty rework gate", "--description", "REQ-RG end to end via the real binary"]);
    let id = created["id"].as_str().unwrap().to_string();
    let ws_str = ws.path().to_str().unwrap();

    // First passage: ANALYSIS → … → APPROVAL (human gate), then to QA.
    devai(home.path(), &["task", "run", &id, "--agent", "mock", "--workspace", ws_str]);
    devai(home.path(), &["task", "approve", &id, "--rationale", "gate test"]);
    devai(home.path(), &["task", "run", &id, "--agent", "mock", "--workspace", ws_str]);
    assert_eq!(devai_json(home.path(), &["task", "status", &id])["task"]["state"], "QA");

    // QA returns with three concrete points — the precedent's shape.
    devai(home.path(), &["task", "return", &id, "--rationale", "п1; п2; п3"]);

    // The rework execution changes NOTHING: the mock touches no file, so the
    // platform stamp names the same branch tip as the first report (A1).
    devai(home.path(), &["task", "run", &id, "--agent", "mock", "--workspace", ws_str]);

    // LOUD, not silent: the task parks in WAITING_FOR_INPUT returning to
    // REWORK with exactly one blocking, answerable question (REQ-RG-02).
    let st = devai_json(home.path(), &["task", "status", &id]);
    assert_eq!(st["task"]["state"], "WAITING_FOR_INPUT", "QA is withheld from an unchanged rework");
    assert_eq!(st["task"]["return_state"], "REWORK", "the re-entry point is REWORK");
    let open = st["open_questions"].as_array().unwrap();
    assert_eq!(open.len(), 1, "the gate ships one question");
    assert_eq!(open[0]["blocking"], true);
    assert!(open[0]["text"].as_str().unwrap().contains("ratify-empty-rework"), "the question names the escape");
    // REQ-RG-05 AC2: the operator surface shows the next step.
    assert!(st["allowed_operations"].as_array().unwrap().iter().any(|v| v == "answer"));
    // The unchanged rework committed nothing: the task branch sits at the bound head.
    assert_eq!(git(ws.path(), &["rev-parse", &format!("rework/{id}")]), head);
    // QA is unreachable while unratified: accept fails loudly.
    let err = devai_fail(home.path(), &["task", "accept", &id]);
    assert!(err.contains("WAITING_FOR_INPUT") || err.contains("not allowed"), "accept is refused on the gate hold: {err}");

    // The trip is an immutable event naming both reports, both shas, the QA
    // points the skipped rework was supposed to fix (REQ-RG-02 AC1).
    let store = devai_store::Store::open(home.path()).unwrap();
    let t = store.resolve_task(&id).unwrap();
    let events = store.all_events_for_task(t.id).unwrap();
    let trip = events.iter().rev().find(|e| e.kind == event_kind::QA_REWORK_GATE_TRIPPED).expect("the gate recorded its trip");
    assert_eq!(trip.payload[json_field::PREV_COMMIT], head);
    assert_eq!(trip.payload[json_field::CURRENT_COMMIT], head);
    assert_eq!(trip.payload[json_field::QA_RETURN_RATIONALE], "п1; п2; п3");
    let prev_report = trip.payload[json_field::PREV_REPORT].as_str().unwrap().to_string();
    let current_report = trip.payload[json_field::CURRENT_REPORT].as_str().unwrap().to_string();
    assert_ne!(prev_report, current_report, "the pair is (previous report, published report)");

    // The human escape (REQ-RG-03): ratification is an explicit CLI act —
    // recorded as a Decision with the invoking operator as actor.
    let out = devai_json(home.path(), &["task", "ratify-empty-rework", &id, "--rationale", "правок не потребовалось — пункты уже удовлетворены"]);
    assert_eq!(out["state"], "QA", "the ratified no-op passage reaches QA");
    assert_eq!(out["recorded_now"], true);
    assert_eq!(out["prev_report"], prev_report);
    assert_eq!(out["current_report"], current_report);
    let decision_id = out["decision"].as_str().unwrap().to_string();
    let decs = store.list_decisions(t.id).unwrap();
    let d = decs.iter().find(|d| d.kind == decision_kind::REWORK_NOOP_RATIFIED).expect("the ratification is a stored decision");
    assert_eq!(d.actor, "rework-operator", "mandate of the operator who invoked it");
    assert_eq!(d.refs, vec![prev_report.clone(), current_report.clone()], "bound to exactly this pair (AC4 shape)");
    // The transition event cites the decision — audit sees a conscious passage.
    let events = store.all_events_for_task(t.id).unwrap();
    let reported = events.iter().rev().find(|e| e.kind == event_kind::IMPLEMENTATION_REPORTED).unwrap();
    assert_eq!(reported.payload[json_field::RATIFIED_BY], decision_id);
    assert_eq!(reported.payload[json_field::VERDICT], "IDENTICAL_RATIFIED");

    // No double-ratification of a passed cycle: from QA the command refuses.
    let err = devai_fail(home.path(), &["task", "ratify-empty-rework", &id]);
    assert!(err.contains("empty-rework gate"), "ratify applies only to the gate hold: {err}");

    // Not lifetime (REQ-RG-03 AC4): a NEW REWORK cycle over the same unchanged
    // commit trips the gate again — the cycle-1 decision references the old
    // pair and is inert here.
    devai(home.path(), &["task", "return", &id, "--rationale", "again unchanged"]);
    devai(home.path(), &["task", "run", &id, "--agent", "mock", "--workspace", ws_str]);
    assert_eq!(devai_json(home.path(), &["task", "status", &id])["task"]["state"], "WAITING_FOR_INPUT", "cycle 2 trips on its own pair");
    let events = store.all_events_for_task(t.id).unwrap();
    let trips: Vec<_> = events.iter().filter(|e| e.kind == event_kind::QA_REWORK_GATE_TRIPPED).collect();
    assert_eq!(trips.len(), 2, "one immutable trip per gated cycle, history never rewritten");
    let trip2_current = trips[1].payload[json_field::CURRENT_REPORT].as_str().unwrap().to_string();
    assert_ne!(trips[0].payload[json_field::CURRENT_REPORT], trips[1].payload[json_field::CURRENT_REPORT], "the second cycle is a different report");
    assert_eq!(trips[1].payload[json_field::PREV_REPORT], current_report, "the new pair chains: (R2 → R3)");

    // Ratify the second cycle too — the escape works per pair, never as a
    // standing exemption.
    let out = devai_json(home.path(), &["task", "ratify-empty-rework", &id, "--rationale", "still nothing to fix"]);
    assert_eq!(out["state"], "QA");
    assert_eq!(out["current_report"], trip2_current);
    // (serde_json's PartialEq<Value> covers String operands.)
}

#[test]
fn rework_that_actually_changes_something_is_never_touched_by_the_gate() {
    // REQ-RG-01 AC4 / REQ-RG-06: the green path stays green — a rework that
    // lands real work in the platform worktree publishes a NEW stamp and
    // rides to QA with no hold, no question, no delay.
    let home = TempDir::new().unwrap();
    let ws = TempDir::new().unwrap();
    git_ok(ws.path(), &["init", "-q"]);
    std::fs::write(ws.path().join("tracked.txt"), b"alpha\n").unwrap();
    git_ok(ws.path(), &["add", "tracked.txt"]);
    git_ok(
        ws.path(),
        &["-c", "user.email=devai@test.local", "-c", "user.name=devai-test", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "baseline"],
    );
    devai(home.path(), &["init"]);
    let created = devai_json(home.path(), &["task", "new", "Real rework", "--description", "green path via the real binary"]);
    let id = created["id"].as_str().unwrap().to_string();
    let ws_str = ws.path().to_str().unwrap();
    devai(home.path(), &["task", "run", &id, "--agent", "mock", "--workspace", ws_str]);
    devai(home.path(), &["task", "approve", &id, "--rationale", "gate test"]);
    devai(home.path(), &["task", "run", &id, "--agent", "mock", "--workspace", ws_str]);
    devai(home.path(), &["task", "return", &id, "--rationale", "fix п1"]);

    // Real fixes land in the platform worktree before the rework run commits.
    let wt = home.path().join("worktrees").join(&id);
    assert!(wt.is_dir(), "the rework runs inside the platform worktree");
    std::fs::write(wt.join("fixed.txt"), "п1 done\n").unwrap();
    devai(home.path(), &["task", "run", &id, "--agent", "mock", "--workspace", ws_str]);

    let st = devai_json(home.path(), &["task", "status", &id]);
    assert_eq!(st["task"]["state"], "QA", "new work passes through untouched");
    assert!(st["open_questions"].as_array().unwrap().is_empty(), "no gate question for a real rework");

    let store = devai_store::Store::open(home.path()).unwrap();
    let t = store.resolve_task(&id).unwrap();
    let events = store.all_events_for_task(t.id).unwrap();
    assert!(events.iter().all(|e| e.kind != event_kind::QA_REWORK_GATE_TRIPPED), "the gate never fired");
    let reported = events.iter().rev().find(|e| e.kind == event_kind::IMPLEMENTATION_REPORTED).unwrap();
    assert_eq!(reported.payload[json_field::VERDICT], "DISTINCT", "the comparison ran and said: different work");
    assert_ne!(
        reported.payload[json_field::CURRENT_COMMIT].as_str().unwrap(),
        reported.payload[json_field::PREV_COMMIT].as_str().unwrap(),
        "the new stamp differs from the previous report's"
    );
}
