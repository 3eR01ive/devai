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
    decision_kind, event_kind, Artifact, ArtifactKind, Decision, WorkflowEvent,
};
use devai_store::Store;

use crate::{ApiError, ApiResult};

use super::types::SignatureFact;

/// Decision kinds the platform stores that are NOT signature acts — spelled
/// out, so a decision kind that is neither listed here nor a signature kind
/// is a loud error instead of a silent skip.
const NON_SIGNATURE_DECISION_KINDS: &[&str] = &[
    decision_kind::REWORK_MERGE,
    decision_kind::PROFILE_RATIFICATION,
    decision_kind::REWORK_NOOP_RATIFIED,
    decision_kind::DEPENDENCY_OVERRIDE,
    decision_kind::QUALITY_RETURN,
];

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
        decision_kind::REQUIREMENTS_APPROVAL => "approval",
        decision_kind::REQUIREMENTS_REJECTION => "rejection",
        other if NON_SIGNATURE_DECISION_KINDS.contains(&other) => return Ok(None),
        other => return Err(unknown_kind("решения", other)),
    };
    Ok(Some(SignatureFact {
        kind: kind.to_string(),
        actor: decision.actor.clone(),
        at: decision.created_at,
        rationale: decision.rationale.clone(),
        spec_seq: seq_of_refs(&decision.refs, arts),
        source: "decision".to_string(),
    }))
}

fn event_fact(event: &WorkflowEvent) -> ApiResult<Option<SignatureFact>> {
    let kind = match event.kind.as_str() {
        event_kind::REQUIREMENTS_APPROVED => "approval",
        event_kind::REQUIREMENTS_REJECTED => "rejection",
        // Every other PERSISTED kind is a known non-signature event; the list
        // is the domain's own complete vocabulary, so it cannot drift.
        other if event_kind::ALL.contains(&other) => return Ok(None),
        other => return Err(unknown_kind("события", other)),
    };
    Ok(Some(SignatureFact {
        kind: kind.to_string(),
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
        source: "event".to_string(),
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
