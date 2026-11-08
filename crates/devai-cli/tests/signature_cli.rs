//! Rework R2: the CLI mirror `devai task signature <ID> [--json]` renders the
//! SAME `task.signature` object the window's card renders — every block of
//! the human output is exercised here (`print_signature_spec`, `_review`,
//! `_previous`, `_fact`), each in both of its branches, so no render path of
//! the mirror is dead code that only a help snapshot would pass.
//!
//! The state is built straight in the store (the same rows the engine and
//! the human write), then read through the real binary.

use std::path::Path;
use std::process::Command;

use chrono::Utc;
use devai_core::types::{
    ArtifactKind, ContractRef, DataCategory, Decision, Task, TaskState, Verdict, decision_kind,
    new_id,
};
use devai_store::{ArtifactDraft, Store};
use tempfile::TempDir;

fn devai(home: &Path, args: &[&str]) -> String {
    let out = Command::new(env!("CARGO_BIN_EXE_devai"))
        .arg("--home")
        .arg(home)
        .arg("--actor")
        .arg("signature-human")
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

fn publish(
    store: &Store,
    task_id: devai_core::types::Id,
    kind: ArtifactKind,
    name: &str,
    body: &str,
) {
    store
        .publish_artifact(ArtifactDraft {
            task_id,
            kind,
            name: name.into(),
            category: DataCategory::Artifact,
            content: body.as_bytes().to_vec(),
            media_type: "application/json".into(),
            contract: Some(ContractRef {
                name: match kind {
                    ArtifactKind::RequirementsSpec => "requirements-specification".into(),
                    _ => "review-report".into(),
                },
                version: 4,
            }),
            created_by: "analyst".into(),
            execution_id: None,
            depends_on: vec![],
        })
        .unwrap();
}

/// An APPROVAL task with NO spec, NO report and NO signature: every block of
/// the mirror takes its empty branch, and the two rights are judged
/// independently (reject is the state alone).
#[test]
fn signature_of_a_bare_approval_task_names_every_empty_block() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);

    let task_id;
    {
        let store = Store::open(h).unwrap();
        let mut t = Task::new(
            "Signature CLI · пусто",
            "подписывать нечего",
            None,
            Utc::now(),
        );
        t.state = TaskState::Approval;
        task_id = t.id;
        store.create_task(&t).unwrap();
    }
    let id = task_id.to_string();

    let human = devai(h, &["task", "signature", &id]);
    assert!(human.contains("may_approve: false"), "{human}");
    assert!(
        human.contains("may_reject: true"),
        "reject = state alone: {human}"
    );
    assert!(human.contains("approve denied: no_spec"), "{human}");
    assert!(
        human.contains("утверждать нечего"),
        "the spec block names its empty branch: {human}"
    );
    assert!(
        human.contains("отчёта ревьювера нет"),
        "the reviewer block is never absent: {human}"
    );
    assert!(human.contains("прошлой версии нет"), "{human}");
    assert!(human.contains("ещё не подписывалась"), "{human}");
    assert!(!human.contains("reject denied"), "{human}");

    // --json is the same object, machine-shaped.
    let json = devai(h, &["task", "signature", &id, "--json"]);
    let v: serde_json::Value = serde_json::from_str(&json).expect("--json is one JSON object");
    assert_eq!(v["may_approve"], false);
    assert_eq!(v["may_reject"], true);
    assert_eq!(v["may_approve_denied"], "no_spec");
    assert!(v["spec"].is_null());
    assert!(v["review"].is_null());
    assert_eq!(v["review_note"], "отчёта ревьювера нет");
    assert_eq!(v["previous_note"], "прошлой версии нет");
    assert!(v["signature"].is_null());
}

/// The full card: a CURRENT spec with structure and questions, a report
/// without a human paragraph (summary branch), a READABLE pre-protocol
/// previous version (the R1 state, not a fallback paragraph) and a stored
/// approval decision row.
#[test]
fn signature_prints_every_populated_block_and_never_the_fallback_paragraph() {
    let home = TempDir::new().unwrap();
    let h = home.path();
    devai(h, &["init"]);

    let task_id;
    {
        let store = Store::open(h).unwrap();
        let mut t = Task::new(
            "Signature CLI · полная",
            "бизнес-описание задачи",
            None,
            Utc::now(),
        );
        t.state = TaskState::Approval;
        t.requirements_seq = 1;
        task_id = t.id;
        store.create_task(&t).unwrap();
        publish(
            &store,
            task_id,
            ArtifactKind::RequirementsSpec,
            "spec-v1.json",
            r#"{"version":2,"requirements":[{"id":"REQ-1","statement":"Старое"}]}"#,
        );
        publish(
            &store,
            task_id,
            ArtifactKind::RequirementsSpec,
            "spec-v2.json",
            r#"{"human_paragraph":"Абзац спеки.","requirements":[{"id":"REQ-1","statement":"Сделать","acceptance_criteria":["AC1"],"constraints":["C1"]}],"open_questions":["OQ-1: вопрос?"]}"#,
        );
        publish(
            &store,
            task_id,
            ArtifactKind::ReviewReport,
            "review-v1.json",
            r#"{"verdict":"approved","summary":"Коротко одобряю."}"#,
        );
        store
            .record_decision(&Decision {
                id: new_id(),
                task_id,
                kind: decision_kind::REQUIREMENTS_APPROVAL.into(),
                verdict: Verdict::Approved,
                actor: "signature-human".into(),
                rationale: Some("подписываю".into()),
                refs: vec![],
                created_at: Utc::now(),
                payload: serde_json::Value::Null,
            })
            .unwrap();
    }
    let id = task_id.to_string();

    let human = devai(h, &["task", "signature", &id]);
    // spec block
    assert!(human.contains("--- spec v2"), "{human}");
    assert!(human.contains("Абзац спеки."), "{human}");
    assert!(human.contains("[REQ-1] Сделать"), "{human}");
    assert!(human.contains("AC: AC1"), "{human}");
    assert!(human.contains("· C1"), "{human}");
    assert!(human.contains("OQ-1: вопрос?"), "{human}");
    // reviewer block: @1 has no human_paragraph → the stored summary.
    assert!(human.contains("verdict: approved"), "{human}");
    assert!(human.contains("Коротко одобряю."), "{human}");
    // previous block: the pre-protocol state is typed and NAMED; the
    // business description is never printed as that version's paragraph.
    assert!(human.contains("--- previous: v1"), "{human}");
    assert!(
        human.contains("абзац: прошлая версия — pre_protocol"),
        "the pre-protocol side is typed in the mirror: {human}"
    );
    assert!(
        human.contains("у прошлой версии нет своего абзаца"),
        "the missing paragraph is stated, not invented: {human}"
    );
    assert!(
        !human.contains("абзац  бизнес-описание задачи"),
        "the fallback never becomes a diff line: {human}"
    );
    assert!(
        human.contains(" REQ-1  Старое"),
        "the readable requirement still diffs: {human}"
    );
    // stored fact
    assert!(
        human.contains("--- signature: approval by signature-human"),
        "{human}"
    );
    assert!(human.contains("(decision)"), "{human}");

    let json = devai(h, &["task", "signature", &id, "--json"]);
    let v: serde_json::Value = serde_json::from_str(&json).expect("--json is one JSON object");
    assert_eq!(v["spec"]["artifact"]["status"], "CURRENT");
    assert_eq!(v["spec"]["requirements"][0]["id"], "REQ-1");
    assert_eq!(v["review"]["paragraph_source"], "summary");
    assert_eq!(v["previous"]["diff"]["paragraph_previous"], "pre_protocol");
    assert_eq!(v["previous"]["diff"]["paragraph_current"], "stored");
    assert!(v["previous"]["diff"]["paragraph_note"].is_string());
    assert_eq!(v["signature"]["kind"], "approval");
    assert_eq!(v["signature"]["rationale"], "подписываю");
}
