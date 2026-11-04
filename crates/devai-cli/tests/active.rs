//! `devai task active [--json]` (REQ-ACT-01..06): the cross-task snapshot of
//! RUNNING executions. OQ-1 as ratified: the surface is a separate `task
//! active` subcommand — NOT a flag on `exec list` (execution rows and
//! running work are different entities). Pins the decisive properties:
//! * RUNNING-only, across ALL tasks of the current store, no post-filtering
//!   by the caller;
//! * the stage is the task's live engine state (never a CLI-local table);
//! * the worktree link appears only after the platform recorded the
//!   `worktree.prepared` fact — before it, a dash, never an invented path;
//! * `stale?` is the ratified limit of OQ-2: a mark on rows the STORE itself
//!   proves contradictory (terminal task, still-RUNNING execution) — no pid
//!   detection here, real orphan detection belongs to the guard task;
//! * rows come elapsed-descending (oldest work first), deterministically
//!   (OQ-3 ratified); the row carries no task title (OQ-6 ratified);
//! * `--json` prints exactly one parseable snapshot object (OQ-4 ratified);
//! * snapshot semantics: repeated calls agree, nothing is written, an idle
//!   store exits 0 with the empty marker.

use std::path::Path;
use std::process::Command;

use devai_core::types::event_kind;
use devai_store::Store;
use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("active-human")
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
    serde_json::from_str(s.trim())
        .unwrap_or_else(|e| panic!("devai {args:?} --json must print exactly one JSON object ({e}): {s}"))
}

fn write(home: &Path, name: &str, content: &str) -> String {
    let p = home.join(name);
    std::fs::write(&p, content).unwrap();
    p.display().to_string()
}

/// The line-prefix without its `elapsed ...` tail: the one token two
/// snapshots of an unchanged state are allowed to differ by (REQ-ACT-04 AC3).
fn strip_elapsed(out: &str) -> Vec<&str> {
    out.lines().map(|l| l.split(" elapsed ").next().unwrap_or(l)).collect()
}

#[test]
fn task_active_lifecycle_through_the_real_binary() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);

    // OQ-1 (ratified): the surface is `task active`, registered and visible
    // in help; `exec list` stays a plain execution listing — no --active.
    let help = devai(h, &["task", "--help"]);
    assert!(help.contains("active"), "`task --help` must list `active`:\n{help}");
    let list_help = devai(h, &["exec", "list", "--help"]);
    assert!(!list_help.contains("--active"), "`exec list` keeps no --active flag:\n{list_help}");
    let rejected = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home").arg(h).env_remove("DEVAI_HOME").env_remove("DEVAI_CONFIG")
        .args(["exec", "list", "--active"])
        .output()
        .unwrap();
    assert!(!rejected.status.success(), "`exec list --active` must not exist (OQ-1: entities not mixed)");

    // Two tasks driven to DIFFERENT engine states: ANALYSIS and IMPLEMENTATION.
    let a = devai_json(h, &["task", "new", "Alpha active", "--description", ""]);
    let id_a = a["id"].as_str().unwrap().to_string();
    let short_a = id_a[..12].to_string();
    devai(h, &["task", "start", &short_a]); // DRAFT → ANALYSIS

    // UUIDv7's leading bits are the creation timestamp; the 12-char display
    // short therefore covers one 16ms bucket. Two tasks created back-to-back
    // can share it, and the whole lifecycle below addresses tasks by that
    // short — separate the creations past one bucket so shorts provably
    // differ and prefix resolution stays deterministic.
    std::thread::sleep(std::time::Duration::from_millis(20));
    let b = devai_json(h, &["task", "new", "Beta active", "--description", ""]);
    let id_b = b["id"].as_str().unwrap().to_string();
    let short_b = id_b[..12].to_string();
    assert_ne!(short_a, short_b, "task display shorts must be distinct to drive the lifecycle");
    devai(h, &["task", "start", &short_b]);
    let spec = write(h, "spec.json", "{\"requirements\": []}");
    devai(h, &["artifact", "publish", &short_b, "--kind", "REQUIREMENTS_SPEC", "--file", &spec]);
    devai(h, &["task", "review", &short_b, "--verdict", "approved"]);
    devai(h, &["task", "approve", &short_b]); // → IMPLEMENTATION

    // REQ-ACT-01: one command, RUNNING rows across ALL tasks, no caller
    // filter. A is started first → larger elapsed → must lead the view
    // (OQ-3 ratified: elapsed desc, древнейшие сверху).
    devai(h, &["exec", "start", &short_a, "--role", "requirements-analyst", "--agent", "mock"]);
    // started_at is a millisecond clock: two executions started in the same
    // ms would have EQUAL elapsed, and the elapsed-desc ordering would fall
    // to the id tie-break on random bits. The gap makes "A started first" a
    // fact the view must then display, not a coin flip the test guesses at.
    std::thread::sleep(std::time::Duration::from_millis(20));
    devai(h, &["exec", "start", &short_b, "--role", "implementation-agent", "--agent", "mock"]);
    // FULL execution ids, never the printed 12-char display short: two
    // executions started within the same UUIDv7 millisecond bucket share
    // that prefix, and a test must not depend on which row a colliding
    // prefix resolves to (the store now refuses to guess — get_execution
    // fails loudly, mirroring resolve_task).
    let execs = devai_json(h, &["exec", "list"]);
    let full_running = |task_id: &str| -> String {
        execs
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["task_id"].as_str() == Some(task_id) && e["status"].as_str() == Some("RUNNING"))
            .unwrap_or_else(|| panic!("no RUNNING execution for task {task_id} in {execs}"))["id"]
            .as_str()
            .unwrap()
            .to_string()
    };
    let exec_a = full_running(&id_a);
    let exec_b = full_running(&id_b);

    let v = devai(h, &["task", "active"]);
    assert!(v.contains(&short_a) && v.contains("requirements-analyst@") && v.contains("elapsed"), "alpha row present:\n{v}");
    assert!(v.contains(&short_b) && v.contains("implementation-agent@"), "beta row present:\n{v}");
    // REQ-ACT-02/05: stage is the task's live engine state, per task.
    assert!(v.contains("ANALYSIS") && v.contains("IMPLEMENTATION"), "live stages from TaskState:\n{v}");
    // REQ-ACT-02: no `worktree.prepared` fact yet → dash, not an invented dir.
    assert!(v.contains('—'), "unprepared worktree renders as a dash:\n{v}");
    assert!(!v.contains(&format!("worktrees/{id_a}")), "no fabricated worktree path:\n{v}");
    // REQ-ACT-06: this is NOT the per-task pipeline — no checkmarks, no ops.
    assert!(!v.contains('✓') && !v.contains("allowed operations"), "detail stays with task progress:\n{v}");
    // OQ-6 (ratified): the row carries ids, never the task title.
    assert!(!v.contains("Alpha active") && !v.contains("Beta active"), "no titles in the rows:\n{v}");
    // OQ-2 (ratified): live tasks with RUNNING work are NOT marked stale.
    assert!(!v.contains("stale?"), "nothing provably stale yet:\n{v}");
    // REQ-ACT-06 AC2: the row's task id is sufficient to drill down.
    let p = devai(h, &["task", "progress", &short_b]);
    assert!(p.contains("Beta active"), "short id from the active view resolves in progress:\n{p}");

    // REQ-ACT-03 / OQ-4 (ratified): --json is exactly one snapshot object,
    // snapshot_at + a flat list of RUNNING rows.
    let j = devai_json(h, &["task", "active"]);
    assert!(j["snapshot_at"].is_string(), "snapshot_at present: {j}");
    let rows = j["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "both RUNNING rows, none else: {rows:?}");
    assert_eq!(rows[0]["task_id"], id_a.as_str(), "OQ-3: elapsed desc, longest-running first: {rows:?}");
    for (row, id, exec, slug, state) in [(&rows[0], &id_a, &exec_a, "requirements-analyst", "ANALYSIS"), (&rows[1], &id_b, &exec_b, "implementation-agent", "IMPLEMENTATION")] {
        assert_eq!(row["id"].as_str().unwrap(), exec.as_str(), "row id matches execution {exec}: {row}");
        assert_eq!(row["task_id"], id.as_str());
        assert_eq!(row["role_slug"], slug);
        assert!(row["role_version"].as_i64().unwrap() >= 1);
        assert_eq!(row["role"], format!("{slug}@{}", row["role_version"].as_i64().unwrap()));
        assert_eq!(row["agent_name"], "mock");
        assert!(row["started_at"].is_string() && row["elapsed_seconds"].as_i64().unwrap() >= 0);
        assert_eq!(row["state"], state, "stage comes from the live TaskState: {row}");
        assert_eq!(row["stale"], serde_json::Value::Bool(false), "a live task's RUNNING row is not stale: {row}");
        assert_eq!(row["branch"], serde_json::Value::Null, "no worktree.prepared fact → null: {row}");
        assert_eq!(row["worktree"], serde_json::Value::Null);
        assert!(row.get("task_title").is_none(), "OQ-6: no title travels in the row: {row}");
    }

    // REQ-ACT-04: idempotent snapshot — unchanged state, same output modulo
    // the allowed elapsed growth.
    let first = devai(h, &["task", "active"]);
    let second = devai(h, &["task", "active"]);
    assert_eq!(strip_elapsed(&first), strip_elapsed(&second), "two snapshots of unchanged state agree");

    // REQ-ACT-04: the view wrote nothing — rows still finish through the
    // normal path afterwards.
    devai(h, &["exec", "finish", &exec_a, "--summary", "done"]);
    let v = devai(h, &["task", "active"]);
    assert!(!v.contains(&short_a), "COMPLETED rows drop out (AC2): {v}");
    assert!(v.contains(&short_b));
    let plain = devai(h, &["exec", "list"]);
    assert!(plain.contains("COMPLETED"), "plain exec list keeps showing history — task active is the separate view: {plain}");
    let j = devai_json(h, &["task", "active"]);
    assert_eq!(j["rows"].as_array().unwrap().len(), 1, "json mirrors the RUNNING-only filter: {j}");

    // REQ-ACT-02: the worktree link is backed by the platform FACT — the
    // `worktree.prepared` event the engine records for writing stages.
    let store = Store::open(h).unwrap();
    let uid_b = uuid::Uuid::parse_str(&id_b).unwrap();
    store
        .insert_event(
            Some(uid_b),
            event_kind::WORKTREE_PREPARED,
            "implementation-agent",
            serde_json::json!({
                "execution": exec_b,
                "branch": format!("rework/{id_b}"),
                "worktree": h.join("worktrees").join(&id_b).display().to_string(),
            }),
        )
        .unwrap();
    let v = devai(h, &["task", "active"]);
    assert!(v.contains(&format!("rework/{id_b}")), "branch shown once the fact exists:\n{v}");
    assert!(v.contains(&h.join("worktrees").join(&id_b).display().to_string()), "worktree path shown:\n{v}");
    assert!(!v.contains('—'), "the only remaining row (beta) is prepared — the dash is gone");
    let j = devai_json(h, &["task", "active"]);
    let rows = j["rows"].as_array().unwrap();
    assert_eq!(rows[0]["branch"], format!("rework/{id_b}").as_str());
    assert_eq!(rows[0]["worktree"], h.join("worktrees").join(&id_b).display().to_string().as_str());

    // OQ-2 (ratified): `stale?` marks exactly what the store itself proves
    // contradictory — the task was cancelled while its execution is still
    // RUNNING (terminal state + RUNNING row). No pid was consulted: the
    // store keeps none; real orphan detection is the guard task's ownership.
    devai(h, &["task", "cancel", &short_b, "--reason", "human cancel with work in flight"]);
    let v = devai(h, &["task", "active"]);
    assert!(v.contains("stale?") && v.contains("CANCELLED"), "terminal task with a RUNNING row is marked:\n{v}");
    let j = devai_json(h, &["task", "active"]);
    let rows = j["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 1, "the cancelled task's execution is still RUNNING in the store: {rows:?}");
    assert_eq!(rows[0]["stale"], serde_json::Value::Bool(true), "json carries the same mark: {rows:?}");
    assert_eq!(rows[0]["state"], "CANCELLED");

    // REQ-ACT-04: empty activity is the idle state, not an error — exit 0,
    // empty marker, json with an empty row list.
    devai(h, &["exec", "cancel", &exec_b]);
    let v = devai(h, &["task", "active"]);
    assert_eq!(v.trim(), "(no running executions)", "empty marker printed:\n{v}");
    let j = devai_json(h, &["task", "active"]);
    assert!(j["rows"].as_array().unwrap().is_empty(), "json says so too: {j}");
}

/// The docs/cli-snapshot artifacts must equal the binary's real help (same
/// discipline as the task-progress snapshot): `task` gained `active`, `exec`
/// lost `--active`. Refresh with:
/// `cargo test -p devai-cli --test active -- --ignored`
#[test]
fn help_snapshots_have_no_drift() {
    let home = TempDir::new().unwrap();
    let h = home.path();

    let task_help = devai(h, &["task", "--help"]);
    let stored = read_snapshot(task_snapshot_path());
    assert_eq!(stored.trim_end(), task_help.trim_end(), "help_task snapshot drifted from `devai task --help`");
    assert!(stored.contains("active"), "task help snapshot must list `active`");

    let exec_help = devai(h, &["exec", "--help"]);
    let stored = read_snapshot(exec_snapshot_path());
    assert_eq!(stored.trim_end(), exec_help.trim_end(), "help_exec snapshot drifted from `devai exec --help`");
    assert!(!stored.contains("--active"), "the ratified surface removed the exec flag");
}

#[test]
#[ignore = "rewrites the docs snapshots; run deliberately to refresh them"]
fn update_help_snapshots() {
    let home = TempDir::new().unwrap();
    std::fs::write(task_snapshot_path(), devai(home.path(), &["task", "--help"])).expect("write task snapshot");
    std::fs::write(exec_snapshot_path(), devai(home.path(), &["exec", "--help"])).expect("write exec snapshot");
}

fn read_snapshot(path: std::path::PathBuf) -> String {
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn task_snapshot_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/cli-snapshot/help_task.txt")
}

fn exec_snapshot_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/cli-snapshot/help_exec.txt")
}
