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

fn devai(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("pk-human")
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

/// An ACTIVE task (ANALYSIS), created through the store door so no launch is
/// triggered: these tests read the status, they do not run roles.
fn active_task(store: &Store, title: &str) -> Task {
    let mut t = Task::new(title, "остановка видна в статусе", None, Utc::now());
    t.state = TaskState::Analysis;
    store.create_task(&t).unwrap();
    store.get_task(&t.id.to_string()).unwrap()
}

fn park_event(store: &Store, task: &Task, brake: &str, counter: u32, limit: u32, reason: &str) {
    store
        .insert_event(
            Some(task.id),
            event_kind::PROCESS_PARKED,
            "platform",
            serde_json::json!({
                "brake": brake,
                "counter": counter,
                "limit": limit,
                "reason": reason,
            }),
        )
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

/// REQ-PK-08 case 2 (rounds brake park) + REQ-PK-03 AC2: the counter and the
/// verbatim reason are printed, and editing the DECLARATION file changes the
/// printed phrase with no rebuild.
#[test]
fn status_words_a_rounds_brake_park_and_follows_the_declaration_file() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);
    let store = Store::open(h).unwrap();
    let t = active_task(&store, "тормоз кругов");
    let reason = "brake: 3 round(s) of `implementation-agent` without a new artifact (limit 3)";
    let m = workflow::park(Process::factory(), &t, Some(reason), "platform", Utc::now()).unwrap();
    store.commit(t.lock_version, &m).unwrap();
    park_event(&store, &t, "rounds_without_shift", 3, 3, reason);

    let prefix = &t.id.to_string()[..12];
    let text = devai(h, &["task", "status", prefix]);
    assert!(
        text.contains("тормоз кругов"),
        "the declared phrase:\n{text}"
    );
    assert!(text.contains("3 из 3"), "the counter:\n{text}");
    assert!(
        text.contains(reason),
        "the stored reason is printed verbatim:\n{text}"
    );
    assert!(text.contains("делать:"), "{text}");

    let v = devai_json(h, &["task", "status", prefix]);
    assert_eq!(v["stop"]["kind"], "rounds_without_shift");
    assert_eq!(v["stop"]["counter"], 3);
    assert_eq!(v["stop"]["limit"], 3);
    assert_eq!(v["stop"]["reason"], reason);

    // The operator edits the words — no rebuild, no code change.
    let file = h.join("config").join("workflow").join("park-reasons.yaml");
    let original = std::fs::read_to_string(&file).unwrap();
    let edited = original.replace("сработал тормоз кругов", "круги кончились без артефакта");
    assert_ne!(original, edited, "the factory phrase is in the file");
    std::fs::write(&file, edited).unwrap();
    let text = devai(h, &["task", "status", prefix]);
    assert!(text.contains("круги кончились"), "{text}");
    assert!(!text.contains("сработал тормоз кругов"), "{text}");

    // A kind with no declared phrase is a LOUD config error, never a machine
    // token in the phrase's place (REQ-PK-03 AC3).
    std::fs::write(
        &file,
        original.replace("    manual_pause:", "    manual_pauze:"),
    )
    .unwrap();
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(h)
        .args(["task", "status", prefix])
        .env_remove("DEVAI_HOME")
        .env_remove("DEVAI_CONFIG")
        .output()
        .unwrap();
    assert!(
        !out.status.success(),
        "an incomplete declaration must refuse the read"
    );
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("manual_pause"),
        "the missing value is named: {err}"
    );
}

/// REQ-PK-08 case 3 (launch that did not happen) + REQ-PK-01 AC2: the same
/// answer carries the class, time, verbatim reason and action, says the task
/// keeps going, and has NO stop block — nothing is invented for a task that
/// is not standing still.
#[test]
fn status_words_a_failed_launch_without_a_stop_block() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);
    let store = Store::open(h).unwrap();
    let t = active_task(&store, "запуск не состоялся");
    let reason = "launch failed: no runner at `/opt/nope/runner`: No such file or directory";
    store
        .insert_event(
            Some(t.id),
            event_kind::EXECUTION_LAUNCH_FAILED,
            "platform",
            serde_json::json!({
                "failure_class": "spawn",
                "consecutive": 1,
                "limit": 3,
                "parked": false,
                "reason": reason,
            }),
        )
        .unwrap();

    let prefix = &t.id.to_string()[..12];
    let text = devai(h, &["task", "status", prefix]);
    assert!(
        text.contains("--- запуск не состоялся:"),
        "the failed launch is a section:\n{text}"
    );
    assert!(
        text.contains("породить процесс"),
        "the class in declared words:\n{text}"
    );
    assert!(text.contains("1 из 3"), "the counter pair:\n{text}");
    assert!(
        text.contains("продолжает идти"),
        "the task is NOT stopped by it:\n{text}"
    );
    assert!(text.contains(reason), "the verbatim reason:\n{text}");
    assert!(text.contains("делать:"), "{text}");
    assert!(
        !text.contains("--- остановка:"),
        "no stop block for a task that is not held:\n{text}"
    );

    let v = devai_json(h, &["task", "status", prefix]);
    assert_eq!(v["stop"], serde_json::Value::Null);
    let f = &v["launch_failure"];
    assert_eq!(f["failure_class"], "spawn");
    assert_eq!(f["consecutive"], 1);
    assert_eq!(f["limit"], 3);
    assert_eq!(f["parked"], false);
    assert_eq!(f["continues"], true);
    assert_eq!(f["reason"], reason);
}
