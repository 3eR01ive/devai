//! REQ-QH-05 through the real `devai` binary (rework R3): the CLI REPORT
//! channel must print the stored freshness marker of an outdated quality
//! report instead of silently passing it off as the current decision — and
//! must print nothing when the report is CURRENT.
//!
//! The state is built straight in the store (the same rows the engine
//! writes): a returned task's report is moved CURRENT→STALE by
//! `workflow::return_to_rework` (ratified OQ-QH-3), and a RUNNING auditor
//! execution is the only thing that earns the «в работе» wording.

use std::path::Path;
use std::process::Command;

use chrono::Utc;
use devai_core::types::{
    ArtifactKind, ContractRef, DataCategory, ExecutionStatus, RoleExecution, Task, TaskState,
    new_id,
};
use devai_core::workflow;
use devai_store::{ArtifactDraft, Store};
use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("freshness-human")
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

const REPORT_BODY: &str = "ВЕРДИКТ: FAIL — стенд-маркер отчёта";

fn publish_report(store: &Store, task_id: devai_core::types::Id) {
    store
        .publish_artifact(ArtifactDraft {
            task_id,
            kind: ArtifactKind::QualityReport,
            name: "quality-report".into(),
            category: DataCategory::Artifact,
            content: serde_json::json!({ "report_text": REPORT_BODY })
                .to_string()
                .into_bytes(),
            media_type: "application/json".into(),
            contract: Some(ContractRef {
                name: "quality-report".into(),
                version: 5,
            }),
            created_by: "tester".into(),
            execution_id: None,
            depends_on: vec![],
        })
        .unwrap();
}

fn start_running_audit(store: &Store, task_id: devai_core::types::Id) -> String {
    let exec = RoleExecution {
        id: new_id(),
        task_id,
        role_slug: "quality-auditor".into(),
        role_version: 1,
        agent_name: Some("mock".into()),
        status: ExecutionStatus::Running,
        inputs: vec![],
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
    };
    store.start_execution(&exec).unwrap();
    devai_core::types::short_id(&exec.id)
}

#[test]
fn report_channel_marks_a_returned_report_stale_and_names_the_running_reaudit() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);

    let task_id;
    {
        let store = Store::open(h).unwrap();
        let mut t = Task::new(
            "Freshness CLI",
            "the report channel must not pass stale data off as the decision",
            None,
            Utc::now(),
        );
        t.state = TaskState::Qa;
        task_id = t.id;
        store.create_task(&t).unwrap();
        publish_report(&store, task_id);
    }
    let id = task_id.to_string();

    // 1. CURRENT report in QA: the channel renders it with NO marker.
    let fresh = devai(h, &["task", "report", &id]);
    assert!(
        fresh.contains(REPORT_BODY),
        "the stored report text is shown:\n{fresh}"
    );
    assert!(
        !fresh.contains("УСТАРЕЛ"),
        "a CURRENT report carries no staleness marker:\n{fresh}"
    );

    // 2. The human returns it: the report row moves to STALE (data change),
    //    and no re-audit has been launched yet.
    {
        let store = Store::open(h).unwrap();
        let cur = store.get_task(&id).unwrap();
        assert_eq!(cur.state, TaskState::Qa);
        let m = workflow::return_to_rework(
            &cur,
            &store.artifacts(task_id).unwrap(),
            Some("ещё раз, с тестами"),
            "freshness-human",
            Utc::now(),
        )
        .unwrap();
        store.commit(cur.lock_version, &m).unwrap();
    }
    let returned = devai(h, &["task", "report", &id]);
    assert!(
        returned.contains("УСТАРЕЛ"),
        "the outdated report is announced, not shown as the decision:\n{returned}"
    );
    assert!(
        returned.contains("ре-аудит ещё не запущен"),
        "no execution exists yet — the marker says exactly that:\n{returned}"
    );
    assert!(
        returned.contains(REPORT_BODY),
        "the stored body still follows the marker (never hidden):\n{returned}"
    );

    // 3. A re-audit is now in flight: «в работе» is earned by the RUNNING row
    //    and the line names it — never an invented status.
    let exec_short = {
        let store = Store::open(h).unwrap();
        start_running_audit(&store, task_id)
    };
    let running = devai(h, &["task", "report", &id]);
    assert!(
        running.contains("УСТАРЕЛ") && running.contains("в работе"),
        "a live re-audit is announced as such:\n{running}"
    );
    assert!(
        running.contains(&exec_short),
        "the marker names the running execution {exec_short}:\n{running}"
    );
}
