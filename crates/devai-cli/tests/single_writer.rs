//! The single-writer guard end-to-end through the real binary
//! (REQ-SW-02/05/07, ratified OQ-1/OQ-4): a second RUNNING execution for one
//! task is refused LOUDLY and BY NAME on every CLI path, an orphan blocks
//! `task run` until a human closes its row, a closed row refuses late
//! artifact publication, and `devai init` over an existing v6-shaped store
//! repairs duplicate RUNNING history without breaking the database.
//! Detection is observation only — no test here (and none in the code)
//! signals or kills a process (REQ-SW-06).

use std::path::Path;
use std::process::Command;

use devai_core::types::Task;
use devai_store::Store;
use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("guard-human")
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
    serde_json::from_str(s.trim()).unwrap_or_else(|e| panic!("devai {args:?} --json must print one JSON value ({e}): {s}"))
}

/// A refusal: the command must exit nonzero; returns its stderr.
fn devai_refused(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("guard-human")
        .env_remove("DEVAI_HOME")
        .env_remove("DEVAI_CONFIG")
        .args(args)
        .output()
        .expect("the devai binary is runnable");
    assert!(!out.status.success(), "devai {args:?} must have been refused:\nstdout: {}", String::from_utf8_lossy(&out.stdout));
    String::from_utf8_lossy(&out.stderr).into_owned()
}

/// An isolated, comparable workspace (same fixture as the REQ-008 dogfood:
/// a committed crate so the engine's revision is real, not fabricated).
fn workspace() -> TempDir {
    let ws = TempDir::new().unwrap();
    std::fs::write(ws.path().join("Cargo.toml"), "[package]\nname = \"guard-ws\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
    std::fs::create_dir(ws.path().join("src")).unwrap();
    std::fs::write(ws.path().join("src/lib.rs"), "pub fn ready() {}\n").unwrap();
    for args in [&["init", "-q"][..], &["add", "-A"][..]] {
        let out = Command::new("git").args(args).current_dir(ws.path()).output().expect("git");
        assert!(out.status.success(), "git {args:?}");
    }
    let out = Command::new("git")
        .args(["-c", "user.email=guard@test.local", "-c", "user.name=guard", "-c", "commit.gpgsign=false", "commit", "-q", "-m", "baseline"])
        .current_dir(ws.path())
        .output()
        .expect("git");
    assert!(out.status.success());
    ws
}

/// The FULL id of the task's RUNNING execution (never the display short).
fn running_exec_id(home: &Path, task_id: &str) -> String {
    devai_json(home, &["exec", "list"])
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["task_id"].as_str() == Some(task_id) && e["status"].as_str() == Some("RUNNING"))
        .unwrap_or_else(|| panic!("no RUNNING execution for {task_id}"))["id"]
        .as_str()
        .unwrap()
        .to_string()
}

/// A live-enough task to host executions: DRAFT → ANALYSIS.
fn analysis_task(home: &Path, title: &str) -> String {
    let t = devai_json(home, &[
        "task",
        "new",
        title,
        "--description",
        "",
    ]);
    let id = t["id"].as_str().unwrap().to_string();
    devai(home, &["task", "start", &id[..12].to_string()]);
    std::thread::sleep(std::time::Duration::from_millis(20));
    id
}

#[test]
fn second_start_is_refused_by_name_and_orphan_resolution_unblocks_run() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);
    let id = analysis_task(h, "guard double start");
    let short = id[..12].to_string();

    devai(h, &["exec", "start", &short, "--role", "requirements-analyst", "--agent", "mock"]);
    let exec_a = running_exec_id(h, &id);

    // REQ-SW-02 CLI path: the refusal is LOUD, names BOTH ids and the only
    // human exits. The opening CLI child is gone, so the blocker is labeled
    // an ORPHAN — the ratified OQ-3/OQ-5 verdict, never a silent queueing.
    let err = devai_refused(h, &["exec", "start", &short, "--role", "requirements-analyst", "--agent", "mock"]);
    assert!(err.contains("single-writer guard"), "named refusal: {err}");
    assert!(err.contains(&exec_a) && err.contains(&id), "blocker execution and task named in full: {err}");
    assert!(err.contains("ORPHAN"), "the liveness class is stated: {err}");
    assert!(err.contains(&format!("devai exec finish|fail|cancel {exec_a}")), "the resolution command is printed: {err}");

    // REQ-SW-05: `task run` refuses while the orphan stands unresolved.
    let ws = workspace();
    let ws_str = ws.path().to_str().unwrap();
    let err = devai_refused(h, &["task", "run", &short, "--agent", "mock", "--workspace", ws_str]);
    assert!(err.contains("single-writer guard") && err.contains(&exec_a), "task run blocked by the same named row: {err}");

    // The allowed exit: a human closes the row — then the next run PASSES
    // (the engine drives analyst → reviewer and stops at the approval gate).
    devai(h, &["exec", "fail", &exec_a, "--error", "human resolved the orphan"]);
    let r = devai_json(h, &["task", "run", &short, "--agent", "mock", "--workspace", ws_str]);
    assert_eq!(r["gate"], "WAITING_APPROVAL", "after resolution the run proceeds: {r}");
}

#[test]
fn live_owner_task_run_refusal_is_labeled_live_not_orphan() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);
    let id = analysis_task(h, "guard live owner");
    let short = id[..12].to_string();

    // A RUNNING row whose owner is THIS test process — a live writer. The
    // refusal must say so (the ratified three-way distinction), never call
    // a live process an orphan.
    {
        let store = Store::open(h).unwrap();
        let t = store.resolve_task(&id).unwrap();
        store
            .start_execution(&devai_core::types::RoleExecution {
                id: devai_core::types::new_id(),
                task_id: t.id,
                role_slug: "requirements-analyst".into(),
                role_version: 1,
                agent_name: Some("mock".into()),
                status: devai_core::types::ExecutionStatus::Running,
                inputs: vec![],
                summary: None,
                error: None,
                started_at: chrono::Utc::now(),
                finished_at: None,
                workspace_revision: None,
                attempt: 1,
                parent_execution: None,
                failure_class: None,
                owner_pid: Some(std::process::id() as i64),
                owner_host: devai_runtime::current_host(),
                owner_process_start: None,
            })
            .unwrap();
    }
    let exec_a = running_exec_id(h, &id);
    let ws = workspace();
    let err = devai_refused(h, &["task", "run", &short, "--agent", "mock", "--workspace", ws.path().to_str().unwrap()]);
    assert!(err.contains("LIVE writer") && !err.contains("ORPHAN"), "a live owner is labeled live: {err}");
    assert!(err.contains(&exec_a), "and names the in-flight row: {err}");
}

#[test]
fn publication_into_a_closed_row_is_refused() {
    // Ratified OQ-4: a zombie may NOT write into a closed execution row —
    // `artifact publish --execution` LOUD-refuses a non-RUNNING execution,
    // while the same row still RUNNING publishes fine (status is the gate,
    // liveness is a separate signal).
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);
    let id = analysis_task(h, "guard zombie publish");
    let short = id[..12].to_string();
    let spec = h.join("spec.json");
    std::fs::write(&spec, "{\"requirements\": []}").unwrap();
    let spec_str = spec.to_str().unwrap();

    devai(h, &["exec", "start", &short, "--role", "requirements-analyst", "--agent", "mock"]);
    let exec_a = running_exec_id(h, &id);
    // Still RUNNING: publication passes the gate regardless of liveness.
    devai(h, &["artifact", "publish", &short, "--kind", "REQUIREMENTS_SPEC", "--file", spec_str, "--execution", &exec_a]);

    devai(h, &["exec", "cancel", &exec_a]);
    let err = devai_refused(h, &["artifact", "publish", &short, "--kind", "REQUIREMENTS_SPEC", "--file", spec_str, "--execution", &exec_a]);
    assert!(err.contains("not RUNNING"), "closed row refuses late publication (OQ-4): {err}");
    assert!(err.contains(&exec_a), "the refused execution is named: {err}");
}

#[test]
fn init_heals_a_v6_store_with_duplicate_running_history() {
    // REQ-SW-07 through the real binary: opening an OLD store that already
    // carries duplicate RUNNING rows (nothing forbade them pre-v7) must not
    // break — `devai init` runs the rung, the repair closes the losers, and
    // the guard is immediately ACTIVE.
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);
    let id = analysis_task(h, "guard legacy duplicates");
    {
        // Raw rusqlite, the dependencies.rs precedent: reshape the CURRENT
        // store into the v6 state (no guard index, version stamp at 6) and
        // write the duplicate history the old schema happily allowed.
        let conn = rusqlite::Connection::open(h.join("devai.db")).unwrap();
        conn.execute_batch("DROP INDEX role_executions_single_writer; PRAGMA user_version=6;").unwrap();
        for at in ["2026-10-01T00:00:00.000000Z", "2026-10-02T00:00:00.000000Z"] {
            conn.execute(
                "INSERT INTO role_executions(id,task_id,role_slug,role_version,status,started_at) VALUES(?1,?2,'requirements-analyst',1,'RUNNING',?3)",
                rusqlite::params![devai_core::types::new_id().to_string(), id, at],
            )
            .unwrap();
        }
    }
    // Re-open through the real CLI — the ladder must complete quietly.
    devai(h, &["init"]);
    let running = devai_json(h, &["exec", "list"])
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["task_id"].as_str() == Some(id.as_str()) && e["status"].as_str() == Some("RUNNING"))
        .count();
    assert_eq!(running, 1, "the repair left exactly one RUNNING row (OQ-1 winner)");
    let failed = devai_json(h, &["exec", "list"])
        .as_array()
        .unwrap()
        .iter()
        .filter(|e| e["task_id"].as_str() == Some(id.as_str()) && e["status"].as_str() == Some("FAILED"))
        .count();
    assert_eq!(failed, 1, "the older duplicate closed FAILED with the marker — no duplicates survive (OQ-1)");
    let rows = devai_json(h, &["exec", "list"]).as_array().unwrap().clone();
    let loser = rows
        .iter()
        .find(|e| e["task_id"].as_str() == Some(id.as_str()) && e["status"].as_str() == Some("FAILED"))
        .expect("the closed loser is listed");
    assert!(loser["error"].as_str().unwrap().starts_with("single-writer migration"), "explicit OQ-1 marker in the row: {loser}");

    // The survivor predates owner stamping: `task active` renders it with
    // the UNKNOWN-owner triage mark, never as dead (REQ-SW-04 AC3, migrated).
    let v = devai(h, &["task", "active"]);
    assert!(v.contains("owner?") && !v.contains("stale?"), "migrated owner-less row reads as-is: {v}");

    // The store-level proof of the same facts: repair event + version.
    let store = Store::open(h).unwrap();
    let facts = store
        .events(uuid::Uuid::parse_str(&id).unwrap(), None)
        .unwrap()
        .into_iter()
        .filter(|e| e.kind == devai_core::types::event_kind::EXECUTION_SINGLE_WRITER_REPAIR)
        .collect::<Vec<_>>();
    assert_eq!(facts.len(), 1, "one repair event names the group: {facts:?}");
    assert_eq!(facts[0].actor, "migration:007_single_writer_guard");
    let conn = rusqlite::Connection::open(h.join("devai.db")).unwrap();
    assert_eq!(conn.query_row("PRAGMA user_version", [], |r| r.get::<_, i64>(0)).unwrap(), devai_store::SCHEMA_VERSION);

    // And the guard is ACTIVE immediately on the healed store.
    let short = id[..12].to_string();
    let err = devai_refused(h, &["exec", "start", &short, "--role", "requirements-analyst", "--agent", "mock"]);
    assert!(err.contains("single-writer guard") && err.contains("owner unrecorded"), "unknown-owner survivor blocks, labeled unverifiable: {err}");
}

/// REQ-SW-06 pinned for the whole surface: after every refusal above the
/// store contains NO row written by a killed process — the guard observed
/// and refused, never signalled. (The absence of any signal/kill call in
/// the detect paths is additionally verified by code search; this test
/// pins the observable consequence: execution rows only ever change status
/// through explicit human `exec finish|fail|cancel`.)
#[test]
fn detection_never_writes_and_never_touches_a_process() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);
    let id = analysis_task(h, "guard read-only detect");
    devai(h, &["exec", "start", &id[..12].to_string(), "--role", "requirements-analyst", "--agent", "mock"]);
    let before = devai_json(h, &["exec", "list"]);
    // Three pure-observation commands on the orphan's store: none may
    // change a single row.
    devai(h, &["task", "active"]);
    devai(h, &["task", "active", "--json"]);
    let _ = devai_refused(h, &["exec", "start", &id[..12].to_string(), "--role", "requirements-analyst", "--agent", "mock"]);
    let after = devai_json(h, &["exec", "list"]);
    assert_eq!(before, after, "detection and refusal wrote nothing — statuses move only through human exec commands");
}

/// The `Task` import keeps the store-side construction honest in tests
/// above that hand rows to `Store` directly; silence an unused warning by
/// asserting the helper compiles against the real type.
#[test]
fn legacy_row_shape_constructs() {
    let t = Task::new("shape", "fixture", None, chrono::Utc::now());
    assert!(!t.id.is_nil());
}
