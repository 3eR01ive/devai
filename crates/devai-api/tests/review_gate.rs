//! Отказ ревью уходит ЧЕЛОВЕКУ (процесс v5, решение оператора 12.11): каждый
//! новый круг анализа начинается со слова оператора, а не сам по себе.
//!
//! Что здесь закреплено:
//!   · отказ ревью паркует задачу с ОДНИМ блокирующим вопросом, запомненная
//!     стадия — ANALYSIS (проверять заново нечего, пока нет новой спеки);
//!   · отчёт проверки называет ПРОВЕРЕННУЮ версию спеки (review-report@4:
//!     spec_seq / spec_artifact ставит платформа при публикации);
//!   · «продолжить» возвращает спеку аналитику, «остановить» оставляет дело
//!     у человека.

use devai_api::{Api, PublishReq, TaskCreateReq};
use devai_core::types::*;
use serde_json::{json, Value};
use tempfile::TempDir;

fn home() -> (TempDir, Api, devai_store::Store) {
    let dir = TempDir::new().unwrap();
    let api = Api::open(dir.path(), "review-gate-tester").unwrap();
    api.seed_config(None).unwrap();
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

fn publish(api: &Api, id: &str, kind: ArtifactKind, body: Value, verdict: Option<&str>) -> String {
    api.artifact_publish(PublishReq {
        id: id.to_string(),
        kind,
        name: match kind {
            ArtifactKind::StatementReview => "statement-review.json".into(),
            ArtifactKind::RequirementsSpec => "requirements-specification.json".into(),
            ArtifactKind::ReviewReport => "review-report.json".into(),
            _ => "artifact.json".into(),
        },
        content: serde_json::to_vec(&body).unwrap(),
        depends: vec![],
        contract: None,
        execution: None,
        category: DataCategory::Artifact,
        verdict: verdict.map(|v| v.to_string()),
    })
    .unwrap()
    .publish
    .artifact
    .id
    .to_string()
}

/// Задача в REVIEW с одной опубликованной спекой.
fn to_review(api: &Api, title: &str) -> String {
    let id = fresh_task(api, title);
    api.task_start(&id).unwrap();
    publish(
        api,
        &id,
        ArtifactKind::StatementReview,
        json!({
            "verdict": "APPROVED",
            "human_paragraph": "Постановка проверена: пользователь, наблюдаемый результат, границы и объём названы, поэтому анализу есть с чем работать.",
            "checklist": [
                {"item": "user", "ok": true},
                {"item": "observable", "ok": true},
                {"item": "boundaries", "ok": true},
                {"item": "volume", "ok": true},
                {"item": "acceptance", "ok": true},
                {"item": "single_milestone", "ok": true}
            ]
        }),
        None,
    );
    publish(
        api,
        &id,
        ArtifactKind::RequirementsSpec,
        json!({
            "requirements": [{"id": "R-1", "statement": "первое требование", "acceptance_criteria": ["видно на экране"]}],
            "open_questions": []
        }),
        None,
    );
    assert_eq!(api.task_get(&id).unwrap().state, TaskState::Review);
    id
}

fn rejection() -> Value {
    json!({
        "verdict": "NEEDS_REVISION",
        "summary": "Требование не проверяемо: нет наблюдаемого признака готовности.",
        "human_paragraph": "Проверка вернула требования: один пункт нельзя проверить по написанному, поэтому спека не идёт в реализацию, пока аналитик его не перепишет.",
        "findings": [
            {"severity": "major", "description": "у требования нет наблюдаемого признака готовности", "requirement_ref": "R-1"}
        ]
    })
}

#[test]
fn rejection_hands_control_to_the_human_and_names_the_reviewed_spec() {
    let (_d, api, store) = home();
    let id = to_review(&api, "отказ ревью зовёт человека");
    let spec = store
        .artifacts(id.parse().unwrap())
        .unwrap()
        .into_iter()
        .find(|a| a.kind == ArtifactKind::RequirementsSpec)
        .unwrap();

    let review_id = publish(&api, &id, ArtifactKind::ReviewReport, rejection(), Some("needs_revision"));

    // 1. Задача паркуется у человека, а не уезжает в следующий круг сама.
    let task = api.task_get(&id).unwrap();
    assert_eq!(task.state, TaskState::WaitingForInput);
    assert_eq!(task.return_state, Some(TaskState::Analysis));

    // 2. Вопрос один, блокирующий, с двумя stored-вариантами.
    let qs = store.list_questions(task.id).unwrap();
    assert_eq!(qs.len(), 1, "one question row per hold");
    assert!(qs[0].blocking);
    assert_eq!(
        qs[0].options,
        vec![
            attempt_limit_answer::CONTINUE.to_string(),
            attempt_limit_answer::STOP.to_string()
        ]
    );
    assert!(qs[0].text.contains("не приняла"), "{}", qs[0].text);

    // 3. Отчёт называет проверенную версию спеки — по stored body.
    let review = store.get_artifact(&review_id).unwrap();
    let body: Value = serde_json::from_slice(&store.read_blob(&review.sha256).unwrap()).unwrap();
    assert_eq!(body["spec_seq"], json!(spec.seq));
    assert_eq!(body["spec_artifact"], json!(spec.id.to_string()));
}

#[test]
fn continue_opens_a_new_round_and_stop_keeps_the_task_with_the_human() {
    let (_d, api, store) = home();
    let id = to_review(&api, "ответ на вопрос о новом круге");
    publish(&api, &id, ArtifactKind::ReviewReport, rejection(), Some("needs_revision"));
    let q = store
        .list_questions(api.task_get(&id).unwrap().id)
        .unwrap()
        .remove(0);

    let out = api.question_answer(&q.id.to_string(), attempt_limit_answer::CONTINUE).unwrap();
    assert_eq!(out.to_state, TaskState::Analysis, "«продолжить» opens the round");
    assert_eq!(api.task_get(&id).unwrap().return_state, None);

    // Второй круг: новая спека → снова отказ → «остановить».
    publish(
        &api,
        &id,
        ArtifactKind::RequirementsSpec,
        json!({
            "requirements": [{"id": "R-1", "statement": "первое требование переписано", "acceptance_criteria": ["видно на экране"]}],
            "open_questions": []
        }),
        None,
    );
    publish(&api, &id, ArtifactKind::ReviewReport, rejection(), Some("needs_revision"));
    let q2 = store
        .list_questions(api.task_get(&id).unwrap().id)
        .unwrap()
        .into_iter()
        .find(|q| q.status == QuestionStatus::Open)
        .expect("second rejection asks again");
    let out = api.question_answer(&q2.id.to_string(), attempt_limit_answer::STOP).unwrap();
    assert_eq!(out.to_state, TaskState::Blocked, "«остановить» keeps it with the human");
    let task = api.task_get(&id).unwrap();
    assert_eq!(task.state, TaskState::Blocked);
    assert_eq!(task.return_state, Some(TaskState::Analysis));
}
