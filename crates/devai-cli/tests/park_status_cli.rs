//! `devai task status` SPEAKS the stop and the failed launch (task 01a1ca39,
//! REQ-PK-05/08): human words in the human form, the same facts as fields in
//! `--json`, numbers that agree because both come from ONE read, and a phrase
//! that changes when the DECLARATION file changes — no rebuild, no code edit.
//!
//! Three cases, one test each: a park by the agent-failure threshold, a park
//! by the rounds brake, and a launch that did not happen without parking. The
//! history is written through the same store doors the producers use; the CLI
//! is only the reader under test.

use std::path::Path;
use std::process::Command;

use chrono::Utc;
use devai_core::process::Process;
use devai_core::types::*;
use devai_core::workflow;
use devai_store::Store;
use tempfile::TempDir;

fn devai_output(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("pk-human")
        .env_remove("DEVAI_HOME")
        .env_remove("DEVAI_CONFIG")
        .args(args)
        .output()
        .expect("the devai binary is runnable")
}

fn devai(home: &Path, args: &[&str]) -> String {
    let out = devai_output(home, args);
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

/// An ACTIVE task (ANALYSIS), created through the store door so no launch is
/// triggered: these tests read the status, they do not run roles.
fn active_task(store: &Store, title: &str) -> Task {
    let mut t = Task::new(title, "остановка видна в статусе", None, Utc::now());
    t.state = TaskState::Analysis;
    store.create_task(&t).unwrap();
    store.get_task(&t.id.to_string()).unwrap()
}

/// One execution ROW of the stage role — the input the ROUNDS brake counts.
fn stage_round(store: &Store, task: &Task, minutes_ago: i64) {
    let now = Utc::now();
    store
        .start_execution(&RoleExecution {
            id: new_id(),
            task_id: task.id,
            role_slug: "requirements-analyst".into(),
            role_version: 1,
            agent_name: Some("mock".into()),
            status: ExecutionStatus::Failed,
            inputs: Vec::new(),
            summary: None,
            error: Some("round without a new artifact".into()),
            started_at: now - chrono::Duration::minutes(minutes_ago),
            finished_at: Some(now),
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

/// REQ-PK-08 case 1 (agent-failure park): the human form names the reason, the
/// counter, the class in words and the action; the machine form carries the
/// same numbers; the raw event is never dumped; the stored question text is
/// quoted, not retold.
#[test]
fn status_words_an_agent_failure_park_in_both_forms() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);
    let store = Store::open(h).unwrap();
    let t = active_task(&store, "парковка по отказу агента");

    let now = Utc::now();
    let qid = new_id();
    let m = workflow::park_agent_failure(
        Process::factory(),
        &t,
        qid,
        workflow::AgentFailureParking {
            agent: "hermes",
            class: "provider_model",
            series_count: 3,
            threshold: 3,
            first_at: now - chrono::Duration::minutes(1),
            last_at: now,
            tail: "404 Model not exist",
            action: devai_core::agent_failure::action("provider_model"),
            execution_id: None,
        },
        "platform",
        now,
    )
    .unwrap();
    store.commit(t.lock_version, &m).unwrap();

    let prefix = &t.id.to_string()[..12];
    let text = devai(h, &["task", "status", prefix]);
    assert!(text.contains("--- остановка:"), "{text}");
    assert!(
        text.contains("отказ агента"),
        "the declared phrase is printed:\n{text}"
    );
    assert!(text.contains("3 из 3"), "the counter pair:\n{text}");
    assert!(text.contains("платформа"), "the actor in words:\n{text}");
    assert!(
        text.contains("модель исчезла у провайдера"),
        "the class in words, not the raw token:\n{text}"
    );
    assert!(
        text.contains("класс:   модель исчезла у провайдера"),
        "the class line is the declared phrase alone (ratified OQ-PK-6):\n{text}"
    );
    assert!(
        !text.contains("(provider_model)"),
        "the human form never labels with the machine token; --json carries both:\n{text}"
    );
    assert!(text.contains("делать:"), "the way out is printed:\n{text}");
    assert!(
        text.contains("Что случилось"),
        "the stored question text is quoted:\n{text}"
    );
    // The raw event payload is NOT what the human reads (REQ-PK-05 AC2).
    assert!(
        !text.contains("question_id") && !text.contains("\"counter\""),
        "no raw event JSON in the human form:\n{text}"
    );

    let v = devai_json(h, &["task", "status", prefix]);
    let s = &v["stop"];
    assert_eq!(s["kind"], "agent_failure");
    assert_eq!(s["counter"], 3);
    assert_eq!(s["limit"], 3);
    assert_eq!(s["counter_text"], "3 из 3");
    assert_eq!(s["failure_class"], "provider_model");
    assert_eq!(
        s["reason"], v["open_questions"][0]["text"],
        "the machine reason IS the stored question text — verbatim, one source"
    );
    assert!(s["action"].as_str().unwrap().len() > 3);
    assert_eq!(v["launch_failure"], serde_json::Value::Null);
}
