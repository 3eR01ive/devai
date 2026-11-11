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

use std::path::Path;
use std::process::Command;

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

#[test]
fn liveness_reaches_every_surface_with_the_same_words() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);

    // A task driven to ANALYSIS and one execution opened by a CLI process
    // that has already exited — the orphan of the fuel case.
    let t = devai_json(h, &["task", "new", "Liveness surface", "--description", ""]);
    let id = t["id"].as_str().unwrap().to_string();
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

    // --- exec list -------------------------------------------------------
    let rows = devai_json(h, &["exec", "list"]);
    let row = rows.as_array().unwrap().iter().find(|r| {
        r["task_id"].as_str() == Some(id.as_str()) && r["status"].as_str() == Some("RUNNING")
    });
    let row = row.unwrap_or_else(|| panic!("no RUNNING row for {id}: {rows}"));
    let exec_id = row["id"].as_str().unwrap().to_string();
    assert_eq!(
        row["liveness"], "dead",
        "REQ-LV-01: the owner process is gone — the row says so: {row}"
    );
    assert_eq!(row["liveness_line"], DEAD, "one sentence, platform-owned");
    let human = devai(h, &["exec", "list"]);
    assert!(
        human.contains(DEAD),
        "`devai exec list` prints the verdict (REQ-LV-06 AC1):\n{human}"
    );

    // --- exec liveness (the single-row read) -----------------------------
    let one = devai_json(h, &["exec", "liveness", &exec_id]);
    assert_eq!(one["liveness"], "dead");
    assert_eq!(one["line"], DEAD, "the same words on the one-row read");

    // --- task status (the card's machine form) ---------------------------
    let status = devai_json(h, &["task", "status", &id]);
    let running = status["running_executions"]
        .as_array()
        .unwrap_or_else(|| panic!("task.status must carry running_executions: {status}"));
    assert_eq!(running.len(), 1, "one RUNNING execution: {status}");
    assert_eq!(running[0]["id"].as_str(), Some(exec_id.as_str()));
    assert_eq!(running[0]["liveness"], "dead");
    assert_eq!(running[0]["liveness_line"], DEAD);
    assert!(
        running[0]["owner_pid"].is_i64(),
        "the owner pid is stored data, not a guess: {}",
        running[0]
    );
    let status_human = devai(h, &["task", "status", &id]);
    assert!(
        status_human.contains(DEAD),
        "the card's CLI form prints the verdict:\n{status_human}"
    );

    // --- task progress / task active -------------------------------------
    let progress = devai(h, &["task", "progress", &id]);
    assert!(
        progress.contains(DEAD),
        "the progress read carries the same words:\n{progress}"
    );
    let active = devai(h, &["task", "active"]);
    assert!(
        active.contains(DEAD),
        "the active list carries the same words (REQ-LV-06 AC2):\n{active}"
    );
    let active_json = devai_json(h, &["task", "active"]);
    assert_eq!(
        active_json["rows"][0]["liveness_line"], DEAD,
        "and the same fact in its json: {active_json}"
    );

    // Observation, not action: the row is still RUNNING and untouched — a
    // read never closes an execution (auto-closing is a later increment).
    let after = devai_json(h, &["exec", "get", &exec_id]);
    assert_eq!(
        after["status"], "RUNNING",
        "the check wrote nothing: {after}"
    );

    // --- a FINISHED row is not checked at all (REQ-LV-06 AC4) ------------
    devai(h, &["exec", "finish", &exec_id, "--summary", "done"]);
    let rows = devai_json(h, &["exec", "list"]);
    let done = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"].as_str() == Some(exec_id.as_str()))
        .expect("the finished row is still listed");
    assert!(
        done["liveness"].is_null() && done["liveness_line"].is_null(),
        "a finished execution carries no liveness mark: {done}"
    );
    let human = devai(h, &["exec", "list"]);
    assert!(
        !human.contains(DEAD),
        "no verdict is invented for history:\n{human}"
    );

    // --- «проверить нельзя» is its own word ------------------------------
    // A row written straight through the store without owner columns (the
    // pre-owner shape): unverifiable, NEVER dead.
    use devai_core::types::{ExecutionStatus, RoleExecution, new_id};
    use devai_store::Store;
    let g = devai_json(h, &["task", "new", "Ownerless", "--description", ""]);
    let id_g = g["id"].as_str().unwrap().to_string();
    devai(h, &["task", "start", &id_g]);
    let ownerless = new_id();
    {
        let store = Store::open(h).unwrap();
        store
            .start_execution(&RoleExecution {
                id: ownerless,
                task_id: uuid::Uuid::parse_str(&id_g).unwrap(),
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
    }
    let rows = devai_json(h, &["exec", "list"]);
    let row = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["id"].as_str() == Some(ownerless.to_string().as_str()))
        .expect("the ownerless RUNNING row is listed");
    assert_eq!(
        row["liveness"], "unknown",
        "not checkable is never death (REQ-LV-02): {row}"
    );
    assert_eq!(row["liveness_line"], UNKNOWN);
    let human = devai(h, &["exec", "list"]);
    assert!(
        human.contains(UNKNOWN),
        "the terminal says «проверить нельзя», not «мёртво»:\n{human}"
    );
    assert_eq!(
        devai_json(h, &["task", "status", &id_g])["running_executions"][0]["liveness"],
        "unknown",
        "and the card's read agrees with the list"
    );
}
