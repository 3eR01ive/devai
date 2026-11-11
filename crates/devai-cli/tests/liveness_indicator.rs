//! Живость — факт, а не строка RUNNING (REQ-LV-01/02/05/06, инкремент 1).
//!
//! Каждый прогон здесь идёт через РЕАЛЬНЫЙ бинарник, потому что проверяется
//! именно поверхность: `exec list` и `task status`/`task progress`/`task
//! active` обязаны нести один и тот же ответ о живости и одну и ту же фразу
//! платформы. Владелец строки, открытой `devai exec start`, — процесс CLI,
//! который к моменту чтения уже вышел: это ПРОВЕРЯЕМАЯ смерть, ровно тот
//! случай, из-за которого задача выглядела работающей.
//!
//! Что пиннится:
//! * завершённая строка не проверяется вовсе и лишней метки не несёт;
//! * «проверить нельзя» (нет записанного владельца) отделимо от «мёртв» и
//!   от «жив» словом, а не догадкой;
//! * проверка — наблюдение: она не меняет строку исполнения.
//!
//! Общее окружение (дом, задача в ANALYSIS, осиротевшая RUNNING-строка)
//! собирается один раз в [`orphaned`]: тело каждого теста остаётся коротким
//! утверждением об ОДНОЙ поверхности (ревью R2, cl/short-units).

use std::path::Path;
use std::process::Command;

use devai_core::types::{ExecutionStatus, RoleExecution, new_id};
use devai_store::Store;
use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("liveness-human")
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
    serde_json::from_str(s.trim()).unwrap_or_else(|e| {
        panic!("devai {args:?} --json must print exactly one JSON document ({e}): {s}")
    })
}

/// The platform's ONE sentence for a proven death (devai-api::liveness_line).
const DEAD: &str = "исполнение мертво (можно перезапустить)";
const UNKNOWN: &str = "живость проверить нельзя";

/// A home with ONE task in ANALYSIS and one RUNNING execution whose owner (the
/// `exec start` CLI process) is already gone — the fuel case of the task.
struct Orphan {
    home: TempDir,
    task: String,
    exec: String,
}

fn orphaned() -> Orphan {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);
    let t = devai_json(h, &["task", "new", "Liveness surface", "--description", ""]);
    let task = t["id"].as_str().unwrap().to_string();
    devai(h, &["task", "start", &task]);
    devai(
        h,
        &[
            "exec",
            "start",
            &task,
            "--role",
            "requirements-analyst",
            "--agent",
            "mock",
        ],
    );
    let rows = devai_json(h, &["exec", "list"]);
    let exec = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| {
            r["task_id"].as_str() == Some(task.as_str()) && r["status"].as_str() == Some("RUNNING")
        })
        .and_then(|r| r["id"].as_str())
        .unwrap_or_else(|| panic!("no RUNNING row for {task}: {rows}"))
        .to_string();
    Orphan { home, task, exec }
}

/// A RUNNING row written straight through the store WITHOUT owner columns (the
/// pre-owner shape): unverifiable, never dead. Returns its id.
fn ownerless_row(h: &Path, task: &str) -> String {
    let id = new_id();
    Store::open(h)
        .unwrap()
        .start_execution(&RoleExecution {
            id,
            task_id: uuid::Uuid::parse_str(task).unwrap(),
            role_slug: "requirements-analyst".into(),
            role_version: 1,
            agent_name: Some("mock".into()),
            status: ExecutionStatus::Running,
            inputs: vec![],
            summary: None,
            error: None,
            started_at: chrono::Utc::now(),
            finished_at: None,
            workspace_revision: None,
            attempt: 1,
            parent_execution: None,
            failure_class: None,
            owner_pid: None,
            owner_host: None,
            owner_process_start: None,
            bundle: None,
            run_id: None,
        })
        .unwrap();
    id.to_string()
}

fn row_of<'a>(rows: &'a serde_json::Value, exec: &str) -> &'a serde_json::Value {
    rows.as_array()
        .unwrap_or_else(|| panic!("a row list was expected: {rows}"))
        .iter()
        .find(|r| r["id"].as_str() == Some(exec))
        .unwrap_or_else(|| panic!("no row {exec} in {rows}"))
}

#[test]
fn exec_list_and_the_single_row_read_name_the_same_death() {
    let o = orphaned();
    let h = o.home.path();
    let rows = devai_json(h, &["exec", "list"]);
    let row = row_of(&rows, &o.exec);
    assert_eq!(
        row["liveness"], "dead",
        "REQ-LV-01: the owner process is gone — the row says so: {row}"
    );
    assert_eq!(row["liveness_line"], DEAD, "one sentence, platform-owned");
    assert!(
        devai(h, &["exec", "list"]).contains(DEAD),
        "`devai exec list` prints the verdict (REQ-LV-06 AC1)"
    );
    let one = devai_json(h, &["exec", "liveness", &o.exec]);
    assert_eq!(one["liveness"], "dead");
    assert_eq!(one["line"], DEAD, "the same words on the one-row read");
}

#[test]
fn the_card_and_the_active_list_carry_the_same_verdict() {
    let o = orphaned();
    let h = o.home.path();
    let status = devai_json(h, &["task", "status", &o.task]);
    let running = status["running_executions"]
        .as_array()
        .unwrap_or_else(|| panic!("task.status must carry running_executions: {status}"));
    assert_eq!(running.len(), 1, "one RUNNING execution: {status}");
    assert_eq!(running[0]["id"].as_str(), Some(o.exec.as_str()));
    assert_eq!(running[0]["liveness"], "dead");
    assert_eq!(running[0]["liveness_line"], DEAD);
    assert!(
        running[0]["owner_pid"].is_i64(),
        "the owner pid is stored data, not a guess: {}",
        running[0]
    );
    assert!(
        devai(h, &["task", "status", &o.task]).contains(DEAD),
        "the card's CLI form prints the verdict"
    );
    assert!(
        devai(h, &["task", "progress", &o.task]).contains(DEAD),
        "the progress read carries the same words"
    );
    assert!(
        devai(h, &["task", "active"]).contains(DEAD),
        "the active list carries the same words (REQ-LV-06 AC2)"
    );
    let active = devai_json(h, &["task", "active"]);
    assert_eq!(
        active["rows"][0]["liveness_line"], DEAD,
        "and the same fact in its json: {active}"
    );
}

#[test]
fn observing_a_row_never_closes_or_changes_it() {
    let o = orphaned();
    let h = o.home.path();
    // Четыре чтения подряд: проверка — наблюдение, а не действие.
    for _ in 0..4 {
        devai(h, &["exec", "list"]);
        devai(h, &["task", "status", &o.task]);
    }
    let after = devai_json(h, &["exec", "get", &o.exec]);
    assert_eq!(
        after["status"], "RUNNING",
        "the check wrote nothing: {after}"
    );
}

#[test]
fn a_finished_row_carries_no_liveness_mark() {
    let o = orphaned();
    let h = o.home.path();
    devai(h, &["exec", "finish", &o.exec, "--summary", "done"]);
    let rows = devai_json(h, &["exec", "list"]);
    let done = row_of(&rows, &o.exec);
    assert!(
        done["liveness"].is_null() && done["liveness_line"].is_null(),
        "a finished execution carries no liveness mark: {done}"
    );
    assert!(
        !devai(h, &["exec", "list"]).contains(DEAD),
        "no verdict is invented for history"
    );
}

#[test]
fn a_row_without_an_owner_is_unverifiable_never_dead() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);
    let t = devai_json(h, &["task", "new", "Ownerless", "--description", ""]);
    let task = t["id"].as_str().unwrap().to_string();
    devai(h, &["task", "start", &task]);
    let id = ownerless_row(h, &task);
    let rows = devai_json(h, &["exec", "list"]);
    let row = row_of(&rows, &id);
    assert_eq!(
        row["liveness"], "unknown",
        "not checkable is never death (REQ-LV-02): {row}"
    );
    assert_eq!(row["liveness_line"], UNKNOWN);
    assert!(
        devai(h, &["exec", "list"]).contains(UNKNOWN),
        "the terminal says «проверить нельзя», not «мёртво»"
    );
    assert_eq!(
        devai_json(h, &["task", "status", &task])["running_executions"][0]["liveness"],
        "unknown",
        "and the card's read agrees with the list"
    );
}
