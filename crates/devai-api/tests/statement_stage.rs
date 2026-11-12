//! The statement stage (process v4): the checker is the FIRST role of a fresh
//! task, and his verdict — not the computer's optimism — decides whether the
//! analysis has anything to work with.
//!
//! Two outcomes matter to the human:
//!   · APPROVED  → the analysis opens by itself;
//!   · QUESTIONS → the task waits for the human, and the questions he owes are
//!                 the checker's own words, one list, no JSON blob.

use devai_api::{Api, PublishReq, TaskCreateReq};
use devai_core::types::*;
use serde_json::json;
use tempfile::TempDir;

fn home() -> (TempDir, Api, devai_store::Store) {
    let dir = TempDir::new().unwrap();
    let api = Api::open(dir.path(), "statement-tester").unwrap();
    api.seed_config(None).unwrap();
    // A second handle on the same file: the test reads what was STORED, not
    // what the API claims.
    let store = devai_store::Store::open(dir.path()).unwrap();
    (dir, api, store)
}

fn fresh_task(api: &Api, title: &str) -> String {
    api.task_create(TaskCreateReq {
        title: title.into(),
        project_id: None,
        description: "человеку нужен результат, который он сможет проверить".into(),
        idempotency_key: None,
        parent: vec![],
        blocked_by: vec![],
    })
    .unwrap()
    .task
    .id
    .to_string()
}

fn approve_body() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "verdict": "APPROVED",
        "human_paragraph": "Сейчас постановка проверена: названы пользователь, наблюдаемый результат, границы и объём работы, поэтому аналитику не придётся ничего домысливать по дороге. После этой проверки задача переходит к анализу, где появится первая версия требований с вопросами к человеку.",
        "checklist": [
            {"item": "user", "ok": true},
            {"item": "observable", "ok": true},
            {"item": "boundaries", "ok": true},
            {"item": "volume", "ok": true},
            {"item": "acceptance", "ok": true},
            {"item": "single_milestone", "ok": true}
        ]
    }))
    .unwrap()
}

fn questions_body() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "verdict": "QUESTIONS",
        "human_paragraph": "Сейчас постановка не готова: три пункта нельзя проверить по написанному, поэтому после ответов задача вернётся к постановщику, а не пойдёт в анализ.",
        "checklist": [
            {"item": "user", "ok": false},
            {"item": "observable", "ok": true},
            {"item": "boundaries", "ok": false},
            {"item": "volume", "ok": false},
            {"item": "acceptance", "ok": true},
            {"item": "single_milestone", "ok": true}
        ],
        "questions": [
            {"item": "user", "text": "Кто именно пользуется результатом и как мы это увидим?"},
            {"item": "boundaries", "text": "Где заканчивается работа: что в неё НЕ входит?"},
            {"item": "volume", "text": "Сколько работы ожидается — один шаг или несколько?"}
        ]
    }))
    .unwrap()
}

fn start(api: &Api, id: &str) {
    api.task_start(id).unwrap();
}

#[test]
fn the_checker_is_the_first_role_of_a_fresh_task() {
    let (_d, api, store) = home();
    let id = fresh_task(&api, "становится в постановщика");
    let out = api.task_start(&id).unwrap();
    assert_eq!(out.task.state, TaskState::Statement);
}

#[test]
fn an_approved_statement_opens_the_analysis_by_itself() {
    let (_d, api, store) = home();
    let id = fresh_task(&api, "годная постановка");
    start(&api, &id);
    api.artifact_publish(PublishReq {
        id: id.clone(),
        kind: ArtifactKind::StatementReview,
        name: "statement-review.json".into(),
        content: approve_body(),
        depends: vec![],
        contract: None,
        execution: None,
        category: DataCategory::Artifact,
        verdict: None,
    })
    .unwrap();
    assert_eq!(
        api.task_get(&id).unwrap().state,
        TaskState::Analysis,
        "the verdict, not the start, opens the analysis"
    );
}

#[test]
fn questions_hold_the_task_and_read_like_a_list_for_the_human() {
    let (_d, api, store) = home();
    let id = fresh_task(&api, "тонкая постановка");
    start(&api, &id);
    api.artifact_publish(PublishReq {
        id: id.clone(),
        kind: ArtifactKind::StatementReview,
        name: "statement-review.json".into(),
        content: questions_body(),
        depends: vec![],
        contract: None,
        execution: None,
        category: DataCategory::Artifact,
        verdict: None,
    })
    .unwrap();

    let task = api.task_get(&id).unwrap();
    assert_eq!(task.state, TaskState::WaitingForInput, "the human owes answers");
    assert_eq!(task.return_state, Some(TaskState::Statement));

    let q = store.list_questions(task.id).unwrap();
    assert_eq!(q.len(), 1, "one question row per hold");
    let text = &q[0].text;
    assert!(text.contains("Кто именно пользуется"), "{text}");
    assert!(text.contains("Где заканчивается работа"), "{text}");
    assert!(text.contains("Сколько работы ожидается"), "{text}");
    assert!(
        text.contains("1. ") && text.contains("3. "),
        "numbered list, not a blob: {text}"
    );
}

#[test]
fn a_thin_statement_cannot_reach_the_analysis_without_a_paragraph() {
    let (_d, api, store) = home();
    let id = fresh_task(&api, "постановка без абзаца");
    start(&api, &id);
    let err = api
        .artifact_publish(PublishReq {
            id: id.clone(),
            kind: ArtifactKind::StatementReview,
            name: "statement-review.json".into(),
            content: serde_json::to_vec(&json!({
                "verdict": "APPROVED",
                "checklist": []
            }))
            .unwrap(),
            depends: vec![],
            // The role's reply always carries the contract it answers to; that
            // is where the schema gate lives.
            contract: Some("statement-review@1".to_string()),
            execution: None,
            category: DataCategory::Artifact,
            verdict: None,
        })
        .unwrap_err();
    assert!(
        err.to_string().contains("human_paragraph"),
        "the contract names the missing field: {err}"
    );
    assert_eq!(api.task_get(&id).unwrap().state, TaskState::Statement);
}

#[test]
fn answering_the_checker_returns_the_task_to_him_not_to_the_analysis() {
    let (_d, api, store) = home();
    let id = fresh_task(&api, "ответ постановщику");
    start(&api, &id);
    api.artifact_publish(PublishReq {
        id: id.clone(),
        kind: ArtifactKind::StatementReview,
        name: "statement-review.json".into(),
        content: questions_body(),
        depends: vec![],
        contract: None,
        execution: None,
        category: DataCategory::Artifact,
        verdict: None,
    })
    .unwrap();
    let task = api.task_get(&id).unwrap();
    let q = store.list_questions(task.id).unwrap();
    let answered = api
        .question_answer(&q[0].id.to_string(), "Пользуется дежурный оператор; видно по списку.")
        .unwrap();
    assert_eq!(answered.to_state, TaskState::Statement);
}
