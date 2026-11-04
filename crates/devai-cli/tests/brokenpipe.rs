//! REQ-PIPE-01/02: every stdout path of the CLI dies quietly on a closed
//! reader — no `println!` panic text, exit code 0 — while logical errors
//! keep their loud stderr + nonzero exit.
//!
//! The deterministic pin: a >64KiB stdout write can only complete if someone
//! reads the pipe. With the read end closed, the writer is blocked in the
//! kernel and provably takes the EPIPE path — the old `println!` panicked
//! there (`io/stdio.rs`, os error 32; the live 2026-11-02 case was
//! `devai task status | head`). The `bash -c … | head -N` cases reproduce
//! the exact reported shapes for status/progress/list/events/help.

use std::io::{BufRead, BufReader, Read};
use std::path::Path;
use std::process::{Command, Stdio};

use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("pipe-human")
        .env_remove("DEVAI_HOME")
        .env_remove("DEVAI_CONFIG")
        .args(args)
        .output()
        .expect("the devai binary is runnable");
    assert!(out.status.success(), "devai {args:?}:\n{}", String::from_utf8_lossy(&out.stderr));
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// Run the binary with stdout piped, consume `read_lines` lines (0 = read
/// nothing), then DROP the read end: every further write hits EPIPE.
/// Returns (exit code, captured stderr).
fn run_then_close_reader(home: &Path, args: &[&str], read_lines: usize) -> (Option<i32>, String) {
    let mut child = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("pipe-human")
        .env_remove("DEVAI_HOME")
        .env_remove("DEVAI_CONFIG")
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("the devai binary is runnable");
    {
        if let Some(stdout) = child.stdout.as_mut() {
            let mut br = BufReader::new(stdout);
            for _ in 0..read_lines {
                let mut line = String::new();
                if br.read_line(&mut line).unwrap_or(0) == 0 {
                    break;
                }
            }
        }
        child.stdout.take(); // drop the read end: the pipe is now closed
    }
    let mut err = Vec::new();
    if let Some(mut stderr) = child.stderr.take() {
        stderr.read_to_end(&mut err).unwrap();
    }
    let status = child.wait().expect("wait");
    (status.code(), String::from_utf8_lossy(&err).into_owned())
}

fn assert_quiet_pipe_death(code: Option<i32>, stderr: &str, case: &str) {
    assert_eq!(code, Some(0), "{case}: closed stdout must exit quietly (code 0), got {code:?}");
    assert!(!stderr.contains("panicked"), "{case}: no panic text on stderr:\n{stderr}");
    assert!(!stderr.contains("Broken pipe"), "{case}: no BrokenPipe chatter:\n{stderr}");
    assert!(stderr.trim().is_empty(), "{case}: stderr must be silent:\n{stderr}");
}

/// Setup: home + one task in ANALYSIS, returning its id.
fn seeded(home: &Path) -> String {
    devai(home, &["init"]);
    devai(home, &["task", "new", "Pipe smoke", "--description", "closed reader"]);
    let id = devai(home, &["task", "list"])
        .lines()
        .find(|l| l.contains("Pipe smoke"))
        .unwrap()
        .split_whitespace()
        .next()
        .unwrap()
        .to_string();
    devai(home, &["task", "start", &id]);
    devai(home, &["task", "progress", &id]);
    id
}

#[test]
fn huge_line_writer_on_closed_pipe_dies_quietly() {
    // The deterministic `println!`-path regression: a pretty-printed ~300KiB
    // contract schema cannot fit the 64KiB pipe buffer, so with the reader
    // closed the CLI provably takes the EPIPE path (old code: panic 101).
    let home = TempDir::new().unwrap();
    let id = seeded(home.path());
    let mut big = String::from("{\"pad\":\"");
    big.push_str(&"a".repeat(300 * 1024));
    big.push_str("\"}");
    let file = home.path().join("big-contract.json");
    std::fs::write(&file, &big).unwrap();
    devai(home.path(), &["contract", "add", "--name", "big", "--version", "1", "--file", &file.display().to_string()]);
    let _ = &id;
    let (code, err) = run_then_close_reader(home.path(), &["contract", "get", "big"], 0);
    // NOTE: `contract get` is the println!-shaped path (formatted JSON line).
    assert_quiet_pipe_death(code, &err, "contract get (300KiB pretty line, reader closed)");
}

#[test]
fn raw_bytes_path_on_closed_pipe_dies_quietly() {
    // `artifact cat` writes bytes directly: even here a closed reader is
    // silence, not an error line (old code: "devai: Broken pipe" exit 1).
    let home = TempDir::new().unwrap();
    let id = seeded(home.path());
    let blob = home.path().join("blob.bin");
    std::fs::write(&blob, vec![b'x'; 300 * 1024]).unwrap();
    let listed = devai(home.path(), &["artifact", "publish", &id, "--kind", "OTHER", "--file", &blob.display().to_string()]);
    let art_id = listed.split_whitespace().nth(2).unwrap().to_string();
    let (code, err) = run_then_close_reader(home.path(), &["artifact", "cat", &art_id], 0);
    assert_quiet_pipe_death(code, &err, "artifact cat (300KiB raw, reader closed)");
}

#[test]
fn head_truncation_of_line_output_dies_quietly() {
    // One line consumed, then the reader closes — the shape of `| head -1`.
    let home = TempDir::new().unwrap();
    let id = seeded(home.path());
    for (args, case) in [
        (vec!["task", "status", &id], "task status"),
        (vec!["task", "progress", &id], "task progress"),
        (vec!["task", "list"], "task list"),
    ] {
        let (code, err) = run_then_close_reader(home.path(), &args, 1);
        // If the child finished before the close, the exit is still 0 with a
        // silent stderr — the assertion holds for both interleavings.
        assert_quiet_pipe_death(code, &err, &case);
    }
}

#[test]
fn literal_head_pipelines_exit_clean() {
    // The literally reported case: `devai task status <ID> | head -8`.
    // PIPESTATUS[0] is devai's own exit code under the pipe.
    let home = TempDir::new().unwrap();
    let id = seeded(home.path());
    let bin = env!("CARGO_BIN_EXE_devai");
    for (cmdline, case) in [
        (format!("{bin} --home {} task status {id} | head -8; exit ${{PIPESTATUS[0]}}", home.path().display()), "task status | head -8"),
        (format!("{bin} --home {} task progress {id} | head -1; exit ${{PIPESTATUS[0]}}", home.path().display()), "task progress | head -1"),
        (format!("{bin} --home {} task --help | head -2; exit ${{PIPESTATUS[0]}}", home.path().display()), "task --help | head -2 (clap path)"),
    ] {
        let out = Command::new("bash").arg("-c").arg(&cmdline).output().expect("bash is available");
        let err = String::from_utf8_lossy(&out.stderr);
        assert!(out.status.success(), "{case}: devai under `| head` must not fail: exit {:?}, stderr: {err}", out.status.code());
        assert!(!err.contains("panicked"), "{case}: no panic text: {err}");
    }
}

/// REQ-ACT-04 constraint: the cross-task `task active` view must not
/// regress the BrokenPipe silence of commit d89cc86. 1000 RUNNING rows are
/// seeded straight into the store — one per TASK: the v7 single-writer
/// guard (`role_executions_single_writer`, REQ-SW-01) makes 1000 RUNNING
/// rows on one task storable no more, and the cross-task view is exactly
/// the surface the count must overflow anyway. Both shapes — ~80KB of human
/// lines and a single ~150KB `--json` line — exceed the 64KiB pipe buffer:
/// with the read end dropped the writer provably blocks in the kernel and
/// takes the EPIPE path (the pre-d89cc86 code would panic here with exit
/// 101).
#[test]
fn task_active_on_closed_pipe_dies_quietly() {
    use devai_core::types::{new_id, ExecutionStatus, RoleExecution, Task};
    use devai_store::Store;

    let home = TempDir::new().unwrap();
    let id = seeded(home.path());
    let _ = &id;
    {
        let store = Store::open(home.path()).unwrap();
        let started_at = chrono::Utc::now();
        for _ in 0..1000 {
            // One task per row: ≤1 RUNNING per task IS the invariant.
            let t = Task::new("pipe overflow task", "", None, started_at);
            store.create_task(&t).unwrap();
            store
                .start_execution(&RoleExecution {
                    id: new_id(),
                    task_id: t.id,
                    role_slug: "requirements-analyst".into(),
                    role_version: 1,
                    agent_name: Some("mock".into()),
                    status: ExecutionStatus::Running,
                    inputs: vec![],
                    summary: None,
                    error: None,
                    started_at,
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
    }

    let (code, err) = run_then_close_reader(home.path(), &["task", "active"], 1);
    assert_quiet_pipe_death(code, &err, "task active (1000 RUNNING lines, reader closed)");
    let (code, err) = run_then_close_reader(home.path(), &["--json", "task", "active"], 0);
    assert_quiet_pipe_death(code, &err, "task active --json (one oversized line, reader closed)");
}

#[test]
fn logical_errors_stay_loud() {
    // REQ-PIPE-02: the quietness is pipe-only; a missing task keeps stderr
    // and exit 1 exactly as before.
    let home = TempDir::new().unwrap();
    devai(home.path(), &["init"]);
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home").arg(home.path())
        .env_remove("DEVAI_HOME").env_remove("DEVAI_CONFIG")
        .args(["task", "status", "no-such-task-id"])
        .output()
        .unwrap();
    assert_eq!(out.status.code(), Some(1), "logical error keeps exit 1");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("not found"), "and stays visible on stderr: {err}");
}
