//! The newest stored signature fact (REQ-SIGN-06): among decision rows of the
//! two kinds and the two event kinds, by `created_at`; on a tie the decision
//! row wins (it carries the refs). `spec_seq` only from a resolvable
//! decision.refs → artifact REQUIREMENTS_SPEC → seq — an unresolvable ref is
//! `null`, never a guess (a backfilled row cites an event id).
//!
//! Selection is FAIL-CLOSED over the kind vocabulary (rework R4). A string
//! `match` with a bare `_ => None` would silently drop a signature act the
//! day a new decision/event kind appears. Here the non-signature kinds are an
//! EXPLICIT list (decisions) or the complete persisted vocabulary
//! (`event_kind::ALL`); anything outside both is an error naming the kind, so
//! an unknown kind is visible, never silence.

use devai_core::types::{
    Artifact, ArtifactKind, Decision, WorkflowEvent, decision_kind, event_kind,
};
use devai_store::Store;

use crate::{ApiError, ApiResult};

use super::types::{SignatureFact, SignatureKind, SignatureSource};

/// Decision kinds the platform stores that are NOT signature acts — spelled
/// out, so a decision kind that is neither listed here nor a signature kind
/// is a loud error instead of a silent skip.
const NON_SIGNATURE_DECISION_KINDS: &[&str] = &[
    decision_kind::REWORK_MERGE,
    decision_kind::PROFILE_RATIFICATION,
    decision_kind::REWORK_NOOP_RATIFIED,
    decision_kind::DEPENDENCY_OVERRIDE,
    decision_kind::QUALITY_RETURN,
    // Ответ на вопрос предела попыток: слово человека о круге роли, а не акт
    // подписи спеки. Без этой строки ВЕСЬ отбор подписи падал (`signature_unknown_kind`)
    // и гейт APPROVAL показывал «спеки нет — подписывать нечего» (прецедент
    // 01a1df87: задача стояла на подписи с целой спекой).
    decision_kind::ATTEMPT_LIMIT_ANSWER,
    // Ответ на вопрос ТОКЕН-БЮДЖЕТА («поднять до N» / «остановиться»): слово
    // человека о деньгах прогонов, а не акт подписи спеки. Без этой строки
    // отбор подписи падал бы `signature_unknown_kind` и гейт APPROVAL
    // показывал «спеки нет» — тот же прецедент 01a1df87, что и выше.
    decision_kind::USAGE_BUDGET,
];

/// Каждый вид решения обязан быть либо актом подписи, либо ЯВНО перечислен
/// как не-подпись: полнота проверяется тестом, поэтому новый вид не может
/// тихо ни потерять подпись, ни уронить весь отбор.
#[cfg(test)]
fn non_signature_is_complete() -> Vec<&'static str> {
    let signature = [
        decision_kind::REQUIREMENTS_APPROVAL,
        decision_kind::REQUIREMENTS_REJECTION,
    ];
    decision_kind::ALL
        .iter()
        .copied()
        .filter(|k| !signature.contains(k))
        .filter(|k| !NON_SIGNATURE_DECISION_KINDS.contains(k))
        .collect()
}

fn unknown_kind(what: &str, kind: &str) -> ApiError {
    ApiError::refused(
        "signature_unknown_kind",
        format!(
            "неизвестный вид {what} `{kind}`: отбор подписи остановлен, чтобы подпись не потерялась молча"
        ),
    )
}

pub fn newest_signature_fact(
    store: &Store,
    task_id: devai_core::types::Id,
    arts: &[Artifact],
) -> ApiResult<Option<SignatureFact>> {
    let decisions = store.list_decisions(task_id)?;
    let events = store.events(task_id, None)?;
    let mut best: Option<SignatureFact> = None;
    for decision in &decisions {
        let Some(fact) = decision_fact(decision, arts)? else {
            continue;
        };
        // `max_by` keeps the LAST of equal maxima (the newest row wins).
        if best.as_ref().is_none_or(|b| fact.at >= b.at) {
            best = Some(fact);
        }
    }
    for event in &events {
        let Some(fact) = event_fact(event)? else {
            continue;
        };
        if best.as_ref().is_none_or(|b| fact.at > b.at) {
            best = Some(fact);
        }
    }
    Ok(best)
}

fn decision_fact(decision: &Decision, arts: &[Artifact]) -> ApiResult<Option<SignatureFact>> {
    let kind = match decision.kind.as_str() {
        decision_kind::REQUIREMENTS_APPROVAL => SignatureKind::Approval,
        decision_kind::REQUIREMENTS_REJECTION => SignatureKind::Rejection,
        other if NON_SIGNATURE_DECISION_KINDS.contains(&other) => return Ok(None),
        other => return Err(unknown_kind("решения", other)),
    };
    Ok(Some(SignatureFact {
        kind,
        actor: decision.actor.clone(),
        at: decision.created_at,
        rationale: decision.rationale.clone(),
        spec_seq: seq_of_refs(&decision.refs, arts),
        source: SignatureSource::Decision,
    }))
}

fn event_fact(event: &WorkflowEvent) -> ApiResult<Option<SignatureFact>> {
    let kind = match event.kind.as_str() {
        event_kind::REQUIREMENTS_APPROVED => SignatureKind::Approval,
        event_kind::REQUIREMENTS_REJECTED => SignatureKind::Rejection,
        // Every other PERSISTED kind is a known non-signature event; the list
        // is the domain's own complete vocabulary, so it cannot drift.
        other if event_kind::ALL.contains(&other) => return Ok(None),
        other => return Err(unknown_kind("события", other)),
    };
    Ok(Some(SignatureFact {
        kind,
        actor: event.actor.clone(),
        at: event.created_at,
        rationale: event
            .payload
            .get("rationale")
            .and_then(|v| v.as_str())
            .map(str::to_string),
        // An event cites no artifact: its version is not recovered from the
        // payload (REQ-SIGN-06 AC3).
        spec_seq: None,
        source: SignatureSource::Event,
    }))
}

/// The spec version a decision ref resolves to, when it resolves at all.
fn seq_of_refs(refs: &[String], arts: &[Artifact]) -> Option<u32> {
    refs.iter().find_map(|r| {
        arts.iter()
            .find(|a| a.kind == ArtifactKind::RequirementsSpec && a.id.to_string() == *r)
            .map(|a| a.seq)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Новый вид решения не может ни потерять подпись молча, ни уронить весь
    /// отбор: он обязан быть актом подписи или явно перечислен как не-подпись.
    #[test]
    fn every_decision_kind_is_classified() {
        let unclassified = super::non_signature_is_complete();
        assert!(
            unclassified.is_empty(),
            "виды решений без классификации (добавь в NON_SIGNATURE_DECISION_KINDS): {unclassified:?}"
        );
    }

    /// Ответ на вопрос предела попыток — не подпись: на задаче 01a1df87 он
    /// ронял весь отбор, и гейт APPROVAL показывал «спеки нет».
    #[test]
    fn attempt_limit_answer_is_not_a_signature_act() {
        assert!(NON_SIGNATURE_DECISION_KINDS.contains(&decision_kind::ATTEMPT_LIMIT_ANSWER));
    }

    /// Ответ на вопрос токен-бюджета — тоже не подпись: иначе отбор подписи
    /// падал бы на каждой задаче, где человек поднимал бюджет (тот же
    /// прецедент, что у предела попыток).
    #[test]
    fn usage_budget_answer_is_not_a_signature_act() {
        assert!(NON_SIGNATURE_DECISION_KINDS.contains(&decision_kind::USAGE_BUDGET));
    }
}
