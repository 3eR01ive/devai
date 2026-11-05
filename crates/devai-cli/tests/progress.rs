//! `devai task progress <ID> [--json]` (REQ-PROG-01..05): pipeline position,
//! running-executions snapshot and machine JSON — drawn from engine data,
//! never from a CLI-local state table. The decisive pin is view==engine:
//! `allowed_operations` in the JSON equals `workflow::allowed_ops` for the
//! same task (REQ-PROG-03 AC2), and the spine order equals
//! `workflow::pipeline_order` — the renderer holds no sequence of its own.

use std::path::Path;
use std::process::Command;

use devai_core::types::Task;
use devai_core::workflow;
use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("progress-human")
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
        .unwrap_or_else(|e| panic!("devai {args:?} --json must print one JSON object ({e}): {s}"))
}

fn write(home: &Path, name: &str, content: &str) -> String {
    let p = home.join(name);
    std::fs::write(&p, content).unwrap();
    p.display().to_string()
}

#[test]
fn progress_lifecycle_through_the_real_binary() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);

    // REQ-PROG-01: the subcommand is registered and visible in help.
    let help = devai(h, &["task", "--help"]);
    assert!(
        help.contains("progress"),
        "`task --help` must list progress:\n{help}"
    );

    let created = devai_json(
        h,
        &[
            "task",
            "new",
            "Progress lifecycle",
            "--description",
            "pipeline at a glance",
        ],
    );
    let id = created["id"].as_str().unwrap().to_string();
    let prefix = &id[..12];

    // DRAFT + prefix resolution (no copy-pasted full UUID needed).
    let v = devai(h, &["task", "progress", prefix]);
    assert!(
        v.contains("[DRAFT]") && v.contains("✓") == false,
        "DRAFT is the first node, nothing passed:\n{v}"
    );
    assert!(
        v.contains("(no running executions)"),
        "empty running snapshot must say so:\n{v}"
    );

    // Unknown id fails loudly like the existing commands (AC REQ-PROG-01).
    let bad = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(h)
        .env_remove("DEVAI_HOME")
        .env_remove("DEVAI_CONFIG")
        .args(["task", "progress", "definitely-not-a-task-id"])
        .output()
        .unwrap();
    assert!(!bad.status.success(), "unknown id must exit nonzero");
    let err = String::from_utf8_lossy(&bad.stderr);
    assert!(err.contains("not found"), "and name the failure: {err}");

    // Drive DRAFT → … → IMPLEMENTATION.
    devai(h, &["task", "start", prefix]);
    let spec = write(h, "spec.json", "{\"requirements\": []}");
    devai(
        h,
        &[
            "artifact",
            "publish",
            prefix,
            "--kind",
            "REQUIREMENTS_SPEC",
            "--file",
            &spec,
        ],
    );
    devai(h, &["task", "review", prefix, "--verdict", "approved"]);
    devai(h, &["task", "approve", prefix]);

    let v = devai(h, &["task", "progress", prefix]);
    assert!(
        v.contains(
            "✓ DRAFT → ✓ ANALYSIS → ✓ REVIEW → ✓ APPROVAL → [IMPLEMENTATION] → QA → ACCEPTED"
        ),
        "spine with current bracket and passed markers:\n{v}"
    );

    // REQ-PROG-03 AC2: what the view offers equals what the engine allows.
    let j = devai_json(h, &["task", "progress", prefix]);
    let task: Task = serde_json::from_value(j["task"].clone()).unwrap();
    assert_eq!(
        j["allowed_operations"],
        serde_json::to_value(workflow::allowed_ops(&task)).unwrap(),
        "progress ops must be exactly workflow::allowed_ops"
    );
    assert_eq!(
        j["pipeline"]["order"],
        serde_json::to_value(
            workflow::pipeline_order()
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>()
        )
        .unwrap(),
        "the spine order is the engine's, not a CLI copy"
    );
    assert_eq!(j["pipeline"]["position_index"], 4);
    assert_eq!(j["pipeline"]["off_spine"], false);

    // REQ-PROG-04: running executions snapshot (role, short id, elapsed).
    let started = devai(
        h,
        &[
            "exec",
            "start",
            prefix,
            "--role",
            "implementation-agent",
            "--agent",
            "mock",
        ],
    );
    let exec_short = started.split_whitespace().nth(1).unwrap().to_string();
    // (role version comes from the seeded config — assert the pair, not the digit)
    let v = devai(h, &["task", "progress", prefix]);
    assert!(
        v.contains("implementation-agent@") && v.contains(&exec_short) && v.contains("elapsed "),
        "running row visible:\n{v}"
    );
    assert!(!v.contains("(no running executions)"));
    let j = devai_json(h, &["task", "progress", prefix]);
    let runs = j["running_executions"].as_array().unwrap();
    assert_eq!(
        runs.len(),
        1,
        "only the RUNNING execution is in the snapshot: {runs:?}"
    );
    assert_eq!(runs[0]["role_slug"], "implementation-agent");
    assert!(runs[0]["role_version"].as_i64().unwrap() >= 1);
    assert_eq!(
        runs[0]["role"],
        format!(
            "implementation-agent@{}",
            runs[0]["role_version"].as_i64().unwrap()
        )
    );
    assert!(runs[0]["elapsed_seconds"].as_i64().unwrap() >= 0);
    assert!(runs[0]["started_at"].is_string());
    devai(h, &["exec", "finish", &exec_short, "--summary", "done"]);
    let v = devai(h, &["task", "progress", prefix]);
    assert!(
        v.contains("(no running executions)"),
        "finished rows drop out:\n{v}"
    );
    assert_eq!(
        devai_json(h, &["task", "progress", prefix])["running_executions"]
            .as_array()
            .unwrap()
            .len(),
        0
    );

    // IMPLEMENTATION → QA, then a hold: PAUSED shows the return stage (REQ-PROG-02).
    let rep = write(
        h,
        "impl-report.json",
        "{\"branch\":\"rework/x\",\"patch\":\"p\",\"test_results\":{}}",
    );
    devai(
        h,
        &[
            "artifact",
            "publish",
            prefix,
            "--kind",
            "IMPLEMENTATION_REPORT",
            "--file",
            &rep,
        ],
    );
    devai(h, &["task", "pause", prefix, "--reason", "hold smoke"]);
    let v = devai(h, &["task", "progress", prefix]);
    assert!(
        v.contains("PAUSED") && v.contains("(returns to QA)") && v.contains("[QA]"),
        "hold visible with its return stage:\n{v}"
    );
    assert!(
        v.contains("note:  PAUSED is off the spine"),
        "the off-spine marker is explicit:\n{v}"
    );
    let j = devai_json(h, &["task", "progress", prefix]);
    assert_eq!(j["pipeline"]["current"], "PAUSED");
    assert_eq!(j["pipeline"]["position"], "QA");
    assert_eq!(j["pipeline"]["position_index"], 5);
    assert_eq!(j["pipeline"]["off_spine"], true);
    devai(h, &["task", "resume", prefix]);

    // QA → REWORK: the loop state re-enters QA (engine knowledge, not CLI).
    devai(
        h,
        &["task", "return", prefix, "--rationale", "rework smoke"],
    );
    let v = devai(h, &["task", "progress", prefix]);
    assert!(
        v.contains("note:  REWORK is off the spine — it returns to [QA]"),
        "REWORK annotated as the QA loop:\n{v}"
    );
    let rep2 = write(
        h,
        "impl-report2.json",
        "{\"branch\":\"rework/x\",\"patch\":\"p2\",\"test_results\":{}}",
    );
    devai(
        h,
        &[
            "artifact",
            "publish",
            prefix,
            "--kind",
            "IMPLEMENTATION_REPORT",
            "--file",
            &rep2,
        ],
    );

    // ACCEPTED: end of the cycle, no open pipeline step, no operations.
    let qr = write(h, "quality.json", "{\"verdict\":\"PASS\"}");
    devai(
        h,
        &[
            "artifact",
            "publish",
            prefix,
            "--kind",
            "QUALITY_REPORT",
            "--file",
            &qr,
        ],
    );
    devai(h, &["task", "accept", prefix]);
    let v = devai(h, &["task", "progress", prefix]);
    assert!(
        v.contains("cycle ended in ACCEPTED"),
        "terminal shown as end of cycle:\n{v}"
    );
    assert!(
        !v.contains("[ACCEPTED]"),
        "terminal is NOT an open current step:\n{v}"
    );
    assert!(
        v.contains("(none — task is terminal)"),
        "ops come from the engine's empty ACCEPTED set:\n{v}"
    );
    let j = devai_json(h, &["task", "progress", prefix]);
    assert_eq!(j["pipeline"]["terminal"], true);
    assert_eq!(j["pipeline"]["position_index"], 6);

    // CANCELLED: terminal off the spine — no position at all.
    let t2 = devai_json(h, &["task", "new", "Cancel me", "--description", ""]);
    let id2 = t2["id"].as_str().unwrap().to_string();
    devai(h, &["task", "cancel", &id2, "--reason", "nope"]);
    let v = devai(h, &["task", "progress", &id2]);
    assert!(
        v.contains("cycle ended in CANCELLED"),
        "cancelled end of cycle:\n{v}"
    );
    let j = devai_json(h, &["task", "progress", &id2]);
    assert_eq!(j["pipeline"]["position"], serde_json::Value::Null);
}

/// REQ-PROG-01 AC3: docs/cli-snapshot/help_task.txt lists `progress` and
/// stays exactly equal to the binary's real help — the snapshot is a
/// test-checked artifact, drift fails here, refresh with:
/// `cargo test -p devai-cli --test progress -- --ignored`
#[test]
fn help_task_snapshot_has_no_drift() {
    let home = TempDir::new().unwrap();
    let help = devai(home.path(), &["task", "--help"]);
    let path = snapshot_path();
    let stored =
        std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()));
    assert_eq!(
        stored.trim_end(),
        help.trim_end(),
        "help snapshot drifted from `devai task --help`"
    );
    assert!(
        stored.contains("progress"),
        "snapshot must list the progress subcommand"
    );
}

#[test]
#[ignore = "rewrites the docs snapshot; run deliberately to refresh it"]
fn update_help_task_snapshot() {
    let home = TempDir::new().unwrap();
    let help = devai(home.path(), &["task", "--help"]);
    std::fs::write(snapshot_path(), help).expect("write snapshot");
}

fn snapshot_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/cli-snapshot/help_task.txt")
}
