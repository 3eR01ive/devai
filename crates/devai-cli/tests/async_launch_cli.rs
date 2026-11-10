//! End-to-end: `devai task run` is ASYNC and the role runs in a platform-owned
//! process (REQ-AR-01/04/07).
//!
//! The real CLI binary is driven here: the command returns a handle, the run
//! completes in a detached runner, the stored rows carry the run number, and
//! `--wait` is the only thing that stays. The session proof (setsid) lives in
//! `devai-runtime`'s `async_launch.rs`; this test pins the operator surface.

use std::path::Path;
use std::process::Command;
use std::time::{Duration, Instant};

use serde_json::Value;
use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--json")
        .args(args)
        .output()
        .expect("spawn devai")
}

fn ok(out: std::process::Output) -> String {
    assert!(
        out.status.success(),
        "devai failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn new_task(home: &Path, title: &str) -> String {
    let v: Value = serde_json::from_str(&ok(devai(home, &["task", "new", title]))).unwrap();
    v["id"].as_str().unwrap().to_string()
}

fn executions(home: &Path, task: &str) -> Vec<Value> {
    let v: Value =
        serde_json::from_str(&ok(devai(home, &["exec", "list", "--task", task]))).unwrap();
    v.as_array().cloned().unwrap_or_default()
}

fn wait_until_settled(home: &Path, task_id: &str, run_id: &str) -> Vec<Value> {
    // The RUN is settled when the platform has stored its outcome — not when
    // «no row is RUNNING» (a chain has a row-less moment between stages).
    let deadline = Instant::now() + Duration::from_secs(60);
    let outcome = home
        .join("evidence")
        .join(task_id)
        .join(run_id)
        .join("outcome.json");
    loop {
        if outcome.is_file() {
            return executions(home, task_id);
        }
        assert!(
            Instant::now() < deadline,
            "the run never settled ({})",
            outcome.display()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[test]
fn task_run_returns_a_handle_and_the_detached_runner_finishes_the_chain() {
    let home = TempDir::new().unwrap();
    ok(devai(home.path(), &["init"]));
    let id = new_task(home.path(), "async cli");

    // The command itself: a handle, not the whole run.
    let out = ok(devai(home.path(), &["task", "run", &id, "--agent", "mock"]));
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["outcome"].as_str(), Some("started"), "{v}");
    let run_id = v["run_id"].as_str().expect("run number").to_string();
    let exec_id = v["execution_id"].as_str().expect("execution handle");
    assert_eq!(
        v["state"].as_str(),
        Some("DRAFT"),
        "the state the launch was asked in"
    );

    // The detached runner drives the chain to the human gate, and every row
    // carries the run number the caller received (REQ-AR-04/07).
    let rows = wait_until_settled(home.path(), &id, &run_id);
    assert!(rows.len() >= 2, "a chain ran: {rows:?}");
    for r in &rows {
        assert_eq!(r["run_id"].as_str(), Some(run_id.as_str()));
    }
    assert!(
        rows.iter().any(|r| r["id"].as_str() == Some(exec_id)),
        "the handle names a real execution"
    );

    // Later executions of the SAME run are listed by the run number.
    let by_run: Value =
        serde_json::from_str(&ok(devai(home.path(), &["exec", "list", "--run", &run_id]))).unwrap();
    assert_eq!(by_run.as_array().unwrap().len(), rows.len());

    // The task reached the human gate; nothing is left RUNNING by design.
    let state: Value =
        serde_json::from_str(&ok(devai(home.path(), &["task", "get", &id]))).unwrap();
    assert_eq!(state["state"].as_str(), Some("APPROVAL"), "{state}");

    // The runner's own log lives in the evidence tree, not in a checkout.
    let ev = home.path().join("evidence").join(&id).join(&run_id);
    assert!(
        !executions(home.path(), &id).is_empty() && ev.is_dir(),
        "run evidence dir exists: {}",
        ev.display()
    );
}

#[test]
fn the_wait_flag_is_the_only_thing_that_stays() {
    let home = TempDir::new().unwrap();
    ok(devai(home.path(), &["init"]));
    let id = new_task(home.path(), "wait for it");
    let out = ok(devai(
        home.path(),
        &["task", "run", &id, "--agent", "mock", "--wait"],
    ));
    let v: Value = serde_json::from_str(&out).unwrap();
    // With --wait the caller receives the SETTLED run report (the same shape
    // a synchronous `task run --json` produced), not a handle.
    assert_eq!(v["gate"].as_str(), Some("WAITING_APPROVAL"), "{v}");
    assert!(
        v["steps"]
            .as_array()
            .map(|s| !s.is_empty())
            .unwrap_or(false),
        "the waited run reports its steps: {v}"
    );
    let state: Value =
        serde_json::from_str(&ok(devai(home.path(), &["task", "get", &id]))).unwrap();
    assert_eq!(state["state"].as_str(), Some("APPROVAL"));

    // A human state launches nothing, and says why (the typed «нельзя»).
    let out = ok(devai(home.path(), &["task", "run", &id, "--agent", "mock"]));
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v["outcome"].as_str(), Some("refused"), "{v}");
    assert!(
        v["reason"].as_str().unwrap_or("").contains("APPROVAL"),
        "the refusal names the state: {v}"
    );
}

#[test]
fn exec_reap_says_what_it_closed_and_closes_nothing_it_cannot_prove() {
    let home = TempDir::new().unwrap();
    ok(devai(home.path(), &["init"]));
    let out = ok(devai(home.path(), &["exec", "reap"]));
    let v: Value = serde_json::from_str(&out).unwrap();
    assert_eq!(v.as_array().map(|a| a.len()), Some(0), "clean home: {v}");
}
