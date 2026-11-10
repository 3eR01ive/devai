//! `exec get` WITHOUT `--json` (rework R7): the HUMAN rendering of a stored
//! execution — status, the saved output tails and the last-activity line — is
//! exactly what the operator reads when asking «а точно идёт?». Only the
//! `--json` shape had a test, so the human form could drift unnoticed. This
//! pins WHAT is printed and in WHICH ORDER:
//!
//! * the identity block (id, task, role, run, agent, status, attempt,
//!   started) comes first, one label per line;
//! * a closed row would add `finished:`/`summary:`/`error:` — an open one
//!   prints none of them;
//! * every evidence file that exists is announced with its own header and its
//!   last lines, in the fixed order: role_output.txt → the runner's own exit
//!   record → the live stdout → the live stderr;
//! * `activity:` comes LAST and is derived from those files — the answer to
//!   «when did it last say anything», never a guess.

use std::path::Path;
use std::process::Command;

use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("exec-get-human")
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
        .unwrap_or_else(|e| panic!("devai {args:?} --json must print one JSON value ({e}): {s}"))
}

/// Position of a marker in the rendered output; a missing marker is a failure
/// that PRINTS the whole rendering (so a drift is readable, not a bare index).
fn at(out: &str, marker: &str) -> usize {
    out.find(marker)
        .unwrap_or_else(|| panic!("`{marker}` must be printed, got:\n{out}"))
}

#[test]
fn exec_get_prints_status_tails_and_activity_in_the_documented_order() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);
    let id = devai_json(h, &["task", "new", "human exec get", "--description", ""])["id"]
        .as_str()
        .unwrap()
        .to_string();
    // A task in a role state can host an execution row.
    devai(h, &["task", "start", &id]);
    devai(
        h,
        &[
            "exec",
            "start",
            &id,
            "--role",
            "requirements-analyst",
            "--agent",
            "mock",
        ],
    );
    let exec_id = devai_json(h, &["exec", "list", "--task", &id])
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["status"].as_str() == Some("RUNNING"))
        .unwrap_or_else(|| panic!("no RUNNING execution for {id}"))["id"]
        .as_str()
        .unwrap()
        .to_string();

    // The four evidence files the view reads, each with a recognisable line.
    let ev = h.join("evidence").join(&id).join(&exec_id);
    std::fs::create_dir_all(&ev).unwrap();
    std::fs::write(
        ev.join("role_output.txt"),
        "exit: exit status: 0\nsettled-tail-line\n",
    )
    .unwrap();
    std::fs::write(
        ev.join("role_output_tail.txt"),
        "exit_code: 0\nrunner-record-line\n",
    )
    .unwrap();
    std::fs::write(ev.join("role_stdout.txt"), "live-stdout-line\n").unwrap();
    std::fs::write(ev.join("role_stderr.txt"), "live-stderr-line\n").unwrap();

    let out = devai(h, &["exec", "get", &exec_id]);

    // The identity block, in its fixed order, one label per line.
    let labels = [
        format!("id:        {exec_id}"),
        format!("task:      {id}"),
        "role:      requirements-analyst@".to_string(),
        "run:       -".to_string(),
        "agent:     mock".to_string(),
        "status:    RUNNING".to_string(),
        "attempt:   1".to_string(),
        "started:   ".to_string(),
    ];
    let mut previous = 0usize;
    for label in &labels {
        let pos = at(&out, label);
        assert!(
            pos >= previous,
            "`{label}` must follow the previous label:\n{out}"
        );
        previous = pos;
    }
    // A row that is still RUNNING says nothing about finishing.
    for absent in ["finished:", "summary:", "error:", "failure:"] {
        assert!(
            !out.contains(absent),
            "an open row must not print `{absent}`:\n{out}"
        );
    }

    // The tails: header then the file's own lines, in the fixed order.
    let order = [
        ("── output tail (role_output.txt) ──", "settled-tail-line"),
        (
            "── output tail (role_output_tail.txt) ──",
            "runner-record-line",
        ),
        (
            "── output tail (role_stdout.txt (live)) ──",
            "live-stdout-line",
        ),
        (
            "── output tail (role_stderr.txt (live)) ──",
            "live-stderr-line",
        ),
    ];
    let mut previous = 0usize;
    for (header, body) in order {
        let pos = at(&out, header);
        assert!(pos >= previous, "`{header}` is out of order:\n{out}");
        assert!(
            at(&out, body) > pos,
            "`{header}` prints the file's own line `{body}`:\n{out}"
        );
        previous = pos;
    }

    // Last fact: activity, derived from the files just shown.
    assert!(
        at(&out, "activity:  ") > previous,
        "`activity:` closes the rendering:\n{out}"
    );
    assert!(
        !out.contains("no output yet"),
        "the files exist, so the activity is measured from them:\n{out}"
    );
}
