//! Producer-side assembly of the stored facts the empty-rework gate compares
//! (REQ-RG-01). This module reads NOTHING but persisted values — artifact
//! bodies and the decision/event history — never touches git (the stamping of
//! `commit` already happened platform-side at publication,
//! `Engine::capture_writing_result`), and performs NO decision: the verdict
//! lives in `devai_core::workflow::implementation_reported`, the single
//! serialization point every publication path passes through. The engine and
//! the CLI `artifact publish` call the same collector, so neither producer can
//! hand the gate a different view of the facts.

use devai_core::types::{decision_kind, event_kind, Artifact, ArtifactKind, Decision, Id};
use devai_core::workflow::{ImplementationReportFacts, PriorImplementationReport};
use devai_store::Store;

use crate::engine::RuntimeResult;

/// The stamped `commit` a stored report body names, or None when the body
/// carries none (read-only producer, pre-@2 contract shape, manual publication
/// without the field) — a distinct case the gate records, never guesses
/// (REQ-RG-04). A body that is not readable/parseable JSON resolves to the
/// same honest None rather than to a crash mid-publication.
fn stamped_commit(store: &Store, a: &Artifact) -> Option<String> {
    let body = store.read_blob(&a.sha256).ok()?;
    let v: serde_json::Value = serde_json::from_slice(&body).ok()?;
    v.get("commit").and_then(|c| c.as_str()).map(|s| s.to_string())
}

/// Assemble the gate facts for publishing `current` as a task's
/// IMPLEMENTATION_REPORT, from `artifacts` (the snapshot INCLUDING the
/// already-published current row) and the stored history.
pub fn collect_implementation_report_facts(
    store: &Store,
    task_id: Id,
    current: &Artifact,
    artifacts: &[Artifact],
    summary: Option<String>,
) -> RuntimeResult<ImplementationReportFacts> {
    let priors: Vec<PriorImplementationReport> = artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::ImplementationReport && a.id != current.id)
        .map(|a| PriorImplementationReport { artifact_id: a.id, seq: a.seq, commit: stamped_commit(store, a) })
        .collect();
    let ratifications: Vec<Decision> = store
        .list_decisions(task_id)?
        .into_iter()
        .filter(|d| d.kind == decision_kind::REWORK_NOOP_RATIFIED)
        .collect();
    // The QA return that opened the CURRENT rework cycle: the most recent
    // stored `quality.returned` rationale (its payload key was written by
    // `workflow::return_to_rework`).
    let qa_return_rationale = store
        .all_events_for_task(task_id)?
        .into_iter()
        .rev()
        .find(|e| e.kind == event_kind::QUALITY_RETURNED)
        .and_then(|e| e.payload.get("rationale").and_then(|r| r.as_str()).map(|s| s.to_string()));
    Ok(ImplementationReportFacts {
        summary,
        current_report: current.id,
        current_commit: stamped_commit(store, current),
        priors,
        ratifications,
        qa_return_rationale,
    })
}
