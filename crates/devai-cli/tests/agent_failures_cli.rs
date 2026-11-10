//! `devai agent list` and `devai task progress` present agent failures as
//! data (task 01a1b60c, REQ-AF-05 AC3, REQ-AF-07 AC1/AC2/AC6): the same
//! words the API publishes, one derived status line, the accumulation count
//! and the class action — and NO repair verb. The failure rows are written
//! through the store exactly as the engine writes them at completion; the
//! CLI is only a reader here.

use std::path::Path;
use std::process::Command;

use chrono::Utc;
use devai_core::types::*;
use devai_store::Store;
use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("afcli-human")
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

fn record(store: &Store, task: &Task, class: &str, tail: &str, at: Timestamp) {
    store
        .record_agent_failure(&AgentFailureRecord {
            id: new_id(),
            task_id: task.id,
            execution_id: Some(new_id()),
            agent_name: "hermes".into(),
            driver: "hermes".into(),
            llm_provider: "deepseek".into(),
            model: "deepseek-v4".into(),
            class: class.into(),
            exit_code: Some(2),
            duration_ms: Some(900),
            tail: tail.into(),
            output_empty: tail.is_empty(),
            driver_log_tail: Some("журнал: 404 Model not exist".into()),
            driver_log_note: None,
            message: "`hermes` exited with exit status: 2".into(),
            basis: "text:model not exist".into(),
            threshold: 3,
            window_minutes: 10,
            at,
        })
        .unwrap();
}

/// REQ-AF-07: the status line names the agent, the availability, the class in
/// words, the number of failures and a time — in the human and the machine
/// form alike; the CLI offers no repair verb.
#[test]
fn agent_list_shows_the_unavailable_status_line() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);

    let store = Store::open(h).unwrap();
    store
        .put_agent(&AgentInstance {
            id: new_id(),
            name: "hermes".into(),
            provider: "hermes".into(),
            model: "deepseek-v4".into(),
            runtime: "deepseek".into(),
            reasoning: "high".into(),
            created_at: Utc::now(),
            active: false,
        })
        .unwrap();
    let t = Task::new("отказ агента", "видимый отказ", None, Utc::now());
    store.create_task(&t).unwrap();
    let now = Utc::now();
    record(&store, &t, "provider_model", "404 Model not exist", now);
    record(
        &store,
        &t,
        "provider_model",
        "404 Model not exist",
        now + chrono::Duration::minutes(1),
    );

    let text = devai(h, &["agent", "list"]);
    assert!(
        text.contains("hermes: недоступен"),
        "the status line names the availability:\n{text}"
    );
    assert!(
        text.contains("модель исчезла у провайдера"),
        "the class is in words, not the raw token:\n{text}"
    );
    assert!(
        text.contains("2 отказов"),
        "the accumulated count is visible:\n{text}"
    );
    assert!(
        text.contains("failures=2") && text.contains("series=2"),
        "the machine counters ride the same row:\n{text}"
    );
    assert!(
        !text.to_lowercase().contains("почин"),
        "the platform names the state, it does not offer a repair:\n{text}"
    );

    // Machine form: the same values as fields (REQ-AF-07 AC6).
    let rows = devai_json(h, &["agent", "list"]);
    let hermes = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["name"] == "hermes")
        .expect("hermes is listed");
    assert_eq!(hermes["availability"], "unavailable");
    assert_eq!(hermes["last_class"], "provider_model");
    assert_eq!(hermes["last_class_word"], "модель исчезла у провайдера");
    assert_eq!(hermes["failures"], 2);
    assert!(hermes["line"].as_str().unwrap().contains("недоступен"));
}

/// REQ-AF-05 AC3: BELOW the threshold the failure is a progress line — the
/// class, `k из N`, the action and the stored tail; nothing is parked.
#[test]
fn task_progress_shows_a_below_threshold_failure_line() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);
    let created = devai_json(
        h,
        &[
            "task",
            "new",
            "Отказ агента",
            "--description",
            "виден строкой в прогрессе",
        ],
    );
    let id = created["id"].as_str().unwrap().to_string();

    let store = Store::open(h).unwrap();
    let t = store.get_task(&id).unwrap();
    record(&store, &t, "network", "connection refused", Utc::now());

    let text = devai(h, &["task", "progress", &id[..12]]);
    assert!(text.contains("отказ агента:"), "{text}");
    assert!(text.contains("сеть недоступна"), "the class word:{text}");
    assert!(text.contains("1 из 3"), "k из N:{text}");
    assert!(text.contains("проверить сеть"), "the class action:{text}");
    assert!(text.contains("connection refused"), "the tail:{text}");

    let v = devai_json(h, &["task", "progress", &id[..12]]);
    let f = &v["agent_failure"];
    assert_eq!(f["class"], "network");
    assert_eq!(f["series_count"], 1);
    assert_eq!(f["threshold"], 3);
    assert_eq!(f["parked"], false);
    assert_eq!(v["task"]["state"], "DRAFT");
}
