//! The newest stored signature fact (REQ-SIGN-06): among decision rows of the
//! two kinds and the two event kinds, by `created_at`; on a tie the decision
//! row wins (it carries the refs). `spec_seq` only from a resolvable
//! decision.refs → artifact REQUIREMENTS_SPEC → seq — an unresolvable ref is
//! `null`, never a guess (a backfilled row cites an event id).

use devai_core::types::{
    Artifact, ArtifactKind, Decision, WorkflowEvent, decision_kind, event_kind,
};
use devai_store::Store;

use crate::ApiResult;

use super::types::SignatureFact;

pub fn newest_signature_fact(
    store: &Store,
    task_id: devai_core::types::Id,
    arts: &[Artifact],
) -> ApiResult<Option<SignatureFact>> {
    let decisions = store.list_decisions(task_id)?;
    let events = store.events(task_id, None)?;
    let mut best = decisions
        .iter()
        .filter_map(|d| decision_fact(d, arts))
        .max_by(|a, b| a.at.cmp(&b.at));
    for fact in events.iter().filter_map(event_fact) {
        if best.as_ref().is_none_or(|b| fact.at > b.at) {
            best = Some(fact);
        }
    }
    Ok(best)
}

fn decision_fact(decision: &Decision, arts: &[Artifact]) -> Option<SignatureFact> {
    let kind = match decision.kind.as_str() {
        decision_kind::REQUIREMENTS_APPROVAL => "approval",
        decision_kind::REQUIREMENTS_REJECTION => "rejection",
        _ => return None,
    };
    Some(SignatureFact {
        kind: kind.to_string(),
        actor: decision.actor.clone(),
        at: decision.created_at,
        rationale: decision.rationale.clone(),
        spec_seq: seq_of_refs(&decision.refs, arts),
        source: "decision".to_string(),
    })
}

fn event_fact(event: &WorkflowEvent) -> Option<SignatureFact> {
    let kind = match event.kind.as_str() {
        event_kind::REQUIREMENTS_APPROVED => "approval",
        event_kind::REQUIREMENTS_REJECTED => "rejection",
        _ => return None,
    };
    Some(SignatureFact {
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
    })
}

/// The spec version a decision ref resolves to, when it resolves at all.
fn seq_of_refs(refs: &[String], arts: &[Artifact]) -> Option<u32> {
    refs.iter().find_map(|r| {
        arts.iter()
            .find(|a| a.kind == ArtifactKind::RequirementsSpec && a.id.to_string() == *r)
            .map(|a| a.seq)
    })
}
