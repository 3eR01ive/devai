//! `devai usage [--task ID] [--day|--week|--month] [--by …]` (task 01a1b851,
//! REQ-USG-09) through the REAL binary — the surface the audit found unpinned
//! (ux/contract-stability): the words the operator reads are asserted here, and
//! they are the numbers of the ONE read the card and the screen also print.
//!
//! Pinned:
//! * with no task the command prints TODAY's spend, the number of runs and the
//!   number of runs with an UNKNOWN spend — and says a day without runs is
//!   «считать нечего», not a silent zero;
//! * with `--task` it prints the card's sentence «израсходовано X из Y», the
//!   model, the run count and the unknown count;
//! * an unknown task is a stated refusal with a non-zero exit;
//! * `--by task` prints one row per task with its own numbers.

use std::path::Path;
use std::process::Command;

use chrono::Utc;
use devai_core::types::*;
use devai_store::{Store, UsageRecord};
use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> String {
    let out = devai_out(home, args);
    assert!(
        out.status.success(),
        "devai {args:?} exited {:?}\nstdout: {}\nstderr: {}",
        out.status.code(),
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn devai_out(home: &Path, args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("usage-human")
        .env_remove("DEVAI_HOME")
        .env_remove("DEVAI_CONFIG")
        .args(args)
        .output()
        .expect("the devai binary is runnable")
}

/// A home with one task and two closed runs: one measured (1_100 tokens, a
/// model) and one that closed with no report at all (the honest UNKNOWN).
fn home_with_two_runs() -> (TempDir, String) {
    let home = TempDir::new().unwrap();
    devai(home.path(), &["init"]);
    let store = Store::open(home.path()).unwrap();
    let t = Task::new("usage cli title", "usage cli", None, Utc::now());
    store.create_task(&t).unwrap();

    for (prompt, completion) in [(1_000_i64, 100_i64), (0, 0)] {
        let e = RoleExecution {
            id: new_id(),
            task_id: t.id,
            role_slug: "implementation-agent".into(),
            role_version: 1,
            agent_name: Some("hermes".into()),
            status: ExecutionStatus::Running,
            inputs: Vec::new(),
            summary: None,
            error: None,
            started_at: Utc::now(),
            finished_at: None,
            workspace_revision: None,
            attempt: 1,
            parent_execution: None,
            failure_class: None,
            owner_pid: None,
            owner_host: None,
            owner_process_start: None,
            bundle: None,
            run_id: Some(new_id()),
        };
        store.start_execution(&e).unwrap();
        store
            .finish_execution(&e, ExecutionStatus::Completed, None, None, Utc::now())
            .unwrap();
        if prompt > 0 {
            store
                .record_execution_usage(&UsageRecord {
                    execution_id: e.id,
                    task_id: t.id,
                    role_slug: "implementation-agent".into(),
                    run_id: e.run_id.map(|r| r.to_string()),
                    provider: Some("hermes".into()),
                    model: Some("deepseek-flash".into()),
                    prompt_tokens: Some(prompt),
                    completion_tokens: Some(completion),
                    total_tokens: Some(prompt + completion),
                    duration_ms: Some(1_000),
                    cost_usd: None,
                    measured: true,
                    unknown_reason: None,
                })
                .unwrap();
        }
    }
    (home, t.id.to_string())
}

#[test]
fn the_day_read_prints_the_totals_with_the_unknown_runs_named() {
    let (home, _id) = home_with_two_runs();
    let text = devai(home.path(), &["usage"]);
    assert!(
        text.contains("за текущие сутки: 1100 токенов · прогонов 2 · неизвестен расход у 1"),
        "the day line carries the numbers and the unknown count: {text}"
    );
    assert!(text.contains("вход 1000 · выход 100"), "{text}");
    assert!(
        text.contains("неизвестна (нет цены модели в конфиге)"),
        "a run with no price is «неизвестна», never $0.00: {text}"
    );
    // The grouping row repeats the same numbers (one source).
    let by_role = devai(home.path(), &["usage", "--by", "role"]);
    assert!(
        by_role.contains("implementation-agent: 1100 токенов · прогонов 2 · неизвестно 1"),
        "{by_role}"
    );
}

#[test]
fn the_task_read_prints_the_card_sentence_models_and_runs() {
    let (home, id) = home_with_two_runs();
    let text = devai(home.path(), &["usage", "--task", &id]);
    assert!(
        text.contains("израсходовано 1100 из 50000000"),
        "the card sentence, verbatim: {text}"
    );
    assert!(text.contains("модель: deepseek-flash"), "{text}");
    assert!(text.contains("прогонов 2"), "{text}");
    assert!(
        text.contains("расход неизвестен у 1 из 2 прогонов (не ноль)"),
        "{text}"
    );
}

#[test]
fn an_unknown_task_and_an_empty_period_are_explained_not_blank() {
    let (home, _id) = home_with_two_runs();
    // An unknown task: a typed refusal, a non-zero exit, the id named.
    let out = devai_out(home.path(), &["usage", "--task", "00000000"]);
    assert!(!out.status.success(), "an unknown task must not exit 0");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(err.contains("00000000"), "the refusal names the id: {err}");

    // An empty period explains itself instead of printing an empty table — and
    // a period with runs prints the task row by its own numbers.
    let week = devai(home.path(), &["usage", "--week", "--by", "task"]);
    assert!(
        week.contains("с начала недели: 1100 токенов · прогонов 2 · неизвестен расход у 1"),
        "{week}"
    );
    assert!(week.contains("usage cli: 1100 токенов"), "{week}");
}

#[test]
fn the_machine_read_is_one_json_object_of_the_same_read() {
    let (home, id) = home_with_two_runs();
    let text = devai(home.path(), &["--json", "usage", "--task", &id]);
    let v: serde_json::Value = serde_json::from_str(text.trim()).expect("one JSON object");
    assert_eq!(v["spent"], 1_100);
    assert_eq!(v["runs"], 2);
    assert_eq!(v["unknown"], 1);
    assert_eq!(v["limit"], 50_000_000);
}

/// ux/contract-stability: the command's OWN surface is pinned, the same way
/// `docs/cli-snapshot/help_task.txt` pins `devai task` — a flag that vanishes or
/// a period that leaves the command is caught by the suite, not by a hand smoke
/// run. Strings above pin the WORDS, this pins the SHAPE.
#[test]
fn the_usage_help_is_the_pinned_snapshot() {
    let home = TempDir::new().unwrap();
    let real = devai(home.path(), &["usage", "--help"]);
    let stored = std::fs::read_to_string(usage_snapshot_path())
        .expect("docs/cli-snapshot/help_usage.txt is part of the tree");
    assert_eq!(
        stored.trim_end(),
        real.trim_end(),
        "help_usage snapshot drifted from `devai usage --help`"
    );
    for flag in ["--task", "--day", "--week", "--month", "--by", "--json"] {
        assert!(
            stored.contains(flag),
            "the pinned surface lists {flag}: {stored}"
        );
    }
}

#[test]
#[ignore = "rewrites the docs snapshot; run deliberately to refresh it"]
fn update_usage_help_snapshot() {
    let home = TempDir::new().unwrap();
    std::fs::write(
        usage_snapshot_path(),
        devai(home.path(), &["usage", "--help"]),
    )
    .expect("write the usage help snapshot");
}

fn usage_snapshot_path() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/cli-snapshot/help_usage.txt")
}
