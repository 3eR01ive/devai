//! `exec get` WITHOUT `--json` (rework R7): the HUMAN rendering of a stored
//! execution — status, the saved output tails and the last-activity line — is
//! exactly what the operator reads when asking «а точно идёт?». Only the
//! `--json` shape had a test, so the human form could drift unnoticed.
//!
//! The decisive case is the RUNNER's own record (`role_output_tail.txt`):
//! `__supervise` writes it into the RUN's evidence dir while `exec get` used
//! to look in the EXECUTION's dir, so the block was never printed on a real
//! launch (R1 of the rework). A hand-made file in the read dir would hide
//! that, so this test drives a REAL `task run --agent mock` and asserts the
//! record is shown, physically lives under the run dir, and is not under the
//! execution dir.
//!
//! Pinned for a real run:
//!
//! * the identity block (id, task, role, run, agent, status, attempt,
//!   started) comes first, one label per line, and `run:` carries the run
//!   number the launch handed out;
//! * the runner's exit record is printed after the identity block and before
//!   `activity:` (the mock agent writes no role output of its own, so this
//!   test pins the record — the block that used to be unreachable);
//! * `activity:` comes LAST — the answer to «when did it last say anything»,
//!   never a guess.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

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

/// The evidence dirs the platform uses: `<home>/evidence/<task>/<run>` for the
/// runner's records and `<home>/evidence/<task>/<execution>` for a row's own.
fn evidence_dir(home: &Path, task: &str, id: &str) -> PathBuf {
    home.join("evidence").join(task).join(id)
}

/// Run one real chain to the human gate and return `(run_id, execution_id)`.
/// The runner writes its exit record last, so waiting for THAT file is
/// waiting for the run to be settled — the artifact this test is about.
fn launch_mock_run(home: &Path, task: &str) -> (String, String) {
    let v = devai_json(home, &["task", "run", task, "--agent", "mock"]);
    assert_eq!(v["outcome"].as_str(), Some("started"), "{v}");
    let run_id = v["run_id"].as_str().expect("run number").to_string();
    let exec_id = v["execution_id"]
        .as_str()
        .expect("execution handle")
        .to_string();
    let record = evidence_dir(home, task, &run_id).join("role_output_tail.txt");
    let deadline = Instant::now() + Duration::from_secs(60);
    while !record.is_file() {
        assert!(
            Instant::now() < deadline,
            "the runner never recorded its exit ({})",
            record.display()
        );
        std::thread::sleep(Duration::from_millis(100));
    }
    (run_id, exec_id)
}

#[test]
fn exec_get_prints_the_runners_exit_record_written_under_the_run_dir() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);
    let id = devai_json(h, &["task", "new", "human exec get", "--description", ""])["id"]
        .as_str()
        .unwrap()
        .to_string();
    let (run_id, exec_id) = launch_mock_run(h, &id);

    let out = devai(h, &["exec", "get", &exec_id]);

    // The identity block, in its fixed order, one label per line.
    let labels = [
        format!("id:        {exec_id}"),
        format!("task:      {id}"),
        "role:      ".to_string(),
        format!("run:       {run_id}"),
        "status:    COMPLETED".to_string(),
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

    // The runner's record is PRINTED, in its own block, with the exit code
    // the platform wrote — the read dir is the write dir.
    let record_header = "── output tail (role_output_tail.txt) ──";
    assert!(
        at(&out, record_header) > previous,
        "the runner's record follows the identity block:\n{out}"
    );
    assert!(
        out.contains("exit_code:"),
        "the block shows the stored exit code:\n{out}"
    );
    // Where the record physically is: under the RUN, never under the row.
    let run_record = evidence_dir(h, &id, &run_id).join("role_output_tail.txt");
    let exec_record = evidence_dir(h, &id, &exec_id).join("role_output_tail.txt");
    assert!(
        run_record.is_file(),
        "the runner writes its record under the run dir ({})",
        run_record.display()
    );
    assert!(
        !exec_record.exists(),
        "no runner record may exist under the execution dir ({})",
        exec_record.display()
    );

    // Last fact: activity, derived from the files just shown.
    assert!(
        at(&out, "activity:  ") > at(&out, record_header),
        "`activity:` closes the rendering:\n{out}"
    );
    assert!(
        !out.contains("no output yet"),
        "the run left evidence, so the activity is measured from it:\n{out}"
    );
}

#[test]
fn a_manual_execution_with_no_run_prints_no_runner_record_and_falls_back_to_start() {
    // The other branch of the same rendering: a row started by hand has no
    // run, so there is no runner dir to read and no record to show. The
    // activity line then names the start — «no output» is a fact, not zero.
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);
    let id = devai_json(h, &["task", "new", "manual exec get", "--description", ""])["id"]
        .as_str()
        .unwrap()
        .to_string();
    devai(h, &["task", "start", &id]);
    statement_ok(h, &id);
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
        .expect("a RUNNING execution")["id"]
        .as_str()
        .unwrap()
        .to_string();

    let out = devai(h, &["exec", "get", &exec_id]);

    assert!(
        out.contains("run:       -"),
        "no run on a manual row:\n{out}"
    );
    assert!(
        !out.contains("role_output_tail.txt"),
        "there is no runner record to print:\n{out}"
    );
    assert!(
        out.contains("activity:  no output yet; started "),
        "the start is the last known fact:\n{out}"
    );
}

/// Process v4: a fresh task stops on the statement checker. The checker's
/// verdict is a published artifact — the same command a role's reply takes —
/// so every CLI fixture walks the real edge instead of a test hook.
fn statement_ok(home: &std::path::Path, id: &str) {
    let body = r#"{"verdict":"APPROVED","human_paragraph":"Сейчас постановка проверена: названы пользователь, наблюдаемый результат, границы и объём работы, поэтому аналитику не придётся ничего домысливать по дороге. После этой проверки задача переходит к анализу, где появится первая версия требований с вопросами к человеку.","checklist":[{"item":"user","ok":true},{"item":"observable","ok":true},{"item":"boundaries","ok":true},{"item":"volume","ok":true},{"item":"acceptance","ok":true},{"item":"single_milestone","ok":true}]}"#;
    let p = home.join("statement-review.json");
    std::fs::write(&p, body).unwrap();
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .args(["artifact", "publish", id, "--kind", "STATEMENT_REVIEW", "--file"])
        .arg(&p)
        .output()
        .expect("the devai binary is runnable");
    assert!(
        out.status.success(),
        "statement review publish for {id} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}
