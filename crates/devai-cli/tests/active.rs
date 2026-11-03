//! `devai exec list --active [--json]` (REQ-ACT-01..06): the cross-task
//! snapshot of RUNNING executions. Pins the decisive properties:
//! * RUNNING-only, across ALL tasks, no post-filtering by the caller;
//! * the stage is the task's live engine state (never a CLI-local table);
//! * the worktree link appears only after the platform recorded the
//!   `worktree.prepared` fact — before it, a dash, never an invented path;
//! * `--json` prints exactly one parseable snapshot object;
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
fn active_overview_lifecycle_through_the_real_binary() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);

    // OQ-1 (interim answer: flag on the existing `exec list`): the surface
    // is registered and visible in help.
    let help = devai(h, &["exec", "--help"]);
    assert!(help.contains("list"), "`exec --help` must list `list`:\n{help}");
    let list_help = devai(h, &["exec", "list", "--help"]);
    assert!(list_help.contains("--active"), "`exec list --help` must expose --active:\n{list_help}");

    // Two tasks driven to DIFFERENT engine states: ANALYSIS and IMPLEMENTATION.
    let a = devai_json(h, &["task", "new", "Alpha active", "--description", ""]);
    let id_a = a["id"].as_str().unwrap().to_string();
    let short_a = id_a[..12].to_string();
    devai(h, &["task", "start", &short_a]); // DRAFT → ANALYSIS

    let b = devai_json(h, &["task", "new", "Beta active", "--description", ""]);
    let id_b = b["id"].as_str().unwrap().to_string();
    let short_b = id_b[..12].to_string();
    devai(h, &["task", "start", &short_b]);
    let spec = write(h, "spec.json", "{\"requirements\": []}");
    devai(h, &["artifact", "publish", &short_b, "--kind", "REQUIREMENTS_SPEC", "--file", &spec]);
    devai(h, &["task", "review", &short_b, "--verdict", "approved"]);
    devai(h, &["task", "approve", &short_b]); // → IMPLEMENTATION

    // REQ-ACT-01: one command, RUNNING rows across ALL tasks, no caller filter.
    let started_a = devai(h, &["exec", "start", &short_a, "--role", "requirements-analyst", "--agent", "mock"]);
    let exec_a = started_a.split_whitespace().nth(1).unwrap().to_string();
    let started_b = devai(h, &["exec", "start", &short_b, "--role", "implementation-agent", "--agent", "mock"]);
    let exec_b = started_b.split_whitespace().nth(1).unwrap().to_string();

    let v = devai(h, &["exec", "list", "--active"]);
    assert!(v.contains(&short_a) && v.contains("requirements-analyst@") && v.contains("elapsed"), "alpha row present:\n{v}");
    assert!(v.contains(&short_b) && v.contains("implementation-agent@"), "beta row present:\n{v}");
    // REQ-ACT-02/05: stage is the task's live engine state, per task.
    assert!(v.contains("ANALYSIS") && v.contains("IMPLEMENTATION"), "live stages from TaskState:\n{v}");
    // REQ-ACT-02: no `worktree.prepared` fact yet → dash, not an invented dir.
    assert!(v.contains('—'), "unprepared worktree renders as a dash:\n{v}");
    assert!(!v.contains(&format!("worktrees/{id_a}")), "no fabricated worktree path:\n{v}");
    // REQ-ACT-06: this is NOT the per-task pipeline — no checkmarks, no ops.
    assert!(!v.contains('✓') && !v.contains("allowed operations"), "detail stays with task progress:\n{v}");
    // REQ-ACT-06 AC2: the row's task id is sufficient to drill down.
    let p = devai(h, &["task", "progress", &short_b]);
    assert!(p.contains("Beta active"), "short id from the active view resolves in progress:\n{p}");

    // Flag composition: --active narrows within one task.
    let v = devai(h, &["exec", "list", "--active", "--task", &short_b]);
    assert!(v.contains(&short_b) && !v.contains(&short_a), "--task filter still RUNNING-only:\n{v}");

    // REQ-ACT-03: --json is exactly one snapshot object, flat RUNNING rows.
    let j = devai_json(h, &["exec", "list", "--active"]);
    assert!(j["snapshot_at"].is_string(), "snapshot_at present: {j}");
    let rows = j["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 2, "both RUNNING rows, none else: {rows:?}");
    assert_eq!(rows[0]["task_id"], id_a.as_str(), "interim OQ-3: longest-running first: {rows:?}");
    for (row, id, exec, slug, state) in [(&rows[0], &id_a, &exec_a, "requirements-analyst", "ANALYSIS"), (&rows[1], &id_b, &exec_b, "implementation-agent", "IMPLEMENTATION")] {
        assert!(row["id"].as_str().unwrap().starts_with(exec.as_str()), "row id matches execution {exec}: {row}");
        assert_eq!(row["task_id"], id.as_str());
        assert_eq!(row["role_slug"], slug);
        assert!(row["role_version"].as_i64().unwrap() >= 1);
        assert_eq!(row["role"], format!("{slug}@{}", row["role_version"].as_i64().unwrap()));
        assert_eq!(row["agent_name"], "mock");
        assert!(row["started_at"].is_string() && row["elapsed_seconds"].as_i64().unwrap() >= 0);
        assert_eq!(row["state"], state, "stage comes from the live TaskState: {row}");
        assert_eq!(row["branch"], serde_json::Value::Null, "no worktree.prepared fact → null: {row}");
        assert_eq!(row["worktree"], serde_json::Value::Null);
    }

    // REQ-ACT-04: idempotent snapshot — unchanged state, same output modulo
    // the allowed elapsed growth.
    let first = devai(h, &["exec", "list", "--active"]);
    let second = devai(h, &["exec", "list", "--active"]);
    assert_eq!(strip_elapsed(&first), strip_elapsed(&second), "two snapshots of unchanged state agree");

    // REQ-ACT-04: the view wrote nothing — both rows are still RUNNING and
    // finish through the normal path afterwards.
    devai(h, &["exec", "finish", &exec_a, "--summary", "done"]);
    let v = devai(h, &["exec", "list", "--active"]);
    assert!(!v.contains(&short_a), "COMPLETED rows drop out (AC2): {v}");
    assert!(v.contains(&short_b));
    let plain = devai(h, &["exec", "list"]);
    assert!(plain.contains("COMPLETED"), "plain exec list keeps showing history — --active is additive: {plain}");
    let j = devai_json(h, &["exec", "list", "--active"]);
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
    let v = devai(h, &["exec", "list", "--active"]);
    assert!(v.contains(&format!("rework/{id_b}")), "branch shown once the fact exists:\n{v}");
    assert!(v.contains(&h.join("worktrees").join(&id_b).display().to_string()), "worktree path shown:\n{v}");
    assert!(!v.contains('—'), "the only remaining row (beta) is prepared — the dash is gone");
    let j = devai_json(h, &["exec", "list", "--active"]);
    let rows = j["rows"].as_array().unwrap();
    assert_eq!(rows[0]["branch"], format!("rework/{id_b}").as_str());
    assert_eq!(rows[0]["worktree"], h.join("worktrees").join(&id_b).display().to_string().as_str());

    // REQ-ACT-04: empty activity is the idle state, not an error — exit 0,
    // empty marker, json with an empty row list.
    devai(h, &["exec", "finish", &exec_b, "--summary", "done"]);
    let v = devai(h, &["exec", "list", "--active"]);
    assert_eq!(v.trim(), "(no running executions)", "empty marker printed:\n{v}");
    let j = devai_json(h, &["exec", "list", "--active"]);
    assert!(j["rows"].as_array().unwrap().is_empty(), "json says so too: {j}");
}

/// The docs/cli-snapshot/help_exec.txt artifact must equal the binary's real
/// help (same discipline as the task-progress snapshot); refresh with:
/// `cargo test -p devai-cli --test active -- --ignored`
#[test]
fn help_exec_snapshot_has_no_drift() {
    let home = TempDir::new().unwrap();
    let help = devai(home.path(), &["exec", "--help"]);
    let path = snapshot_path();
    let stored = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert_eq!(stored.trim_end(), help.trim_end(), "help snapshot drifted from `devai exec --help`");
    assert!(stored.contains("list"), "snapshot must list the list subcommand");
}

#[test]
#[ignore = "rewrites the docs snapshot; run deliberately to refresh it"]
fn update_help_exec_snapshot() {
    let home = TempDir::new().unwrap();
    let help = devai(home.path(), &["exec", "--help"]);
    std::fs::write(snapshot_path(), help).expect("write snapshot");
}

fn snapshot_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/cli-snapshot/help_exec.txt")
}
