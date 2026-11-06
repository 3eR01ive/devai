//! Producer-side assembly of the stored facts the empty-rework gate compares
//! (REQ-RG-01). This module reads NOTHING but persisted values — artifact
//! bodies and the decision/event history — never touches git (the stamping of
//! `commit` already happened platform-side at publication,
//! `Engine::capture_writing_result`), and performs NO decision: the verdict
//! lives in `devai_core::workflow::implementation_reported`, the single
//! serialization point every publication path passes through. The engine and
//! the CLI `artifact publish` call the same collector, so neither producer can
//! hand the gate a different view of the facts.

use devai_core::types::{Artifact, ArtifactKind, Decision, Id, decision_kind, event_kind};
use devai_core::workflow::{ImplementationReportFacts, PriorImplementationReport, ReportBodyRead};
use devai_store::{Store, StoreError};

use crate::engine::RuntimeResult;

/// R5 QA (rl/error-handling/rel/error-context): the stamped `commit` a
/// stored report body names, WITH the typed read outcome that explains an
/// absent commit — readable-but-unnamed (read-only producer, pre-@2 contract
/// shape, manual publication without the field), a missing blob, or a body
/// that is not parseable JSON (corruption). Each collapse into one silent
/// `None` was indistinguishable to the gate before; the decision stays the
/// same (REQ-RG-04: never guess, proceed incomparable), the RECORD becomes
/// honest.
///
/// The blob read goes through `Store::read_artifact_body`, whose one policy
/// separates absence (`NotFound`) from a body that exists but cannot be read
/// (`Corrupt`) — the second is reported as corruption here, not folded into
/// «missing» as it was when this site read the blob itself.
fn stamped_commit(store: &Store, a: &Artifact) -> (Option<String>, ReportBodyRead) {
    let body = match store.read_artifact_body(a) {
        Ok(body) => body,
        Err(StoreError::NotFound(..)) => return (None, ReportBodyRead::Missing),
        Err(_) => return (None, ReportBodyRead::Corrupt),
    };
    match serde_json::from_slice::<serde_json::Value>(&body) {
        Ok(v) => (
            v.get("commit")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string()),
            ReportBodyRead::Readable,
        ),
        Err(_) => (None, ReportBodyRead::Corrupt),
    }
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
        .map(|a| {
            let (commit, body) = stamped_commit(store, a);
            PriorImplementationReport {
                artifact_id: a.id,
                seq: a.seq,
                commit,
                body,
            }
        })
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
        .and_then(|e| {
            e.payload
                .get("rationale")
                .and_then(|r| r.as_str())
                .map(|s| s.to_string())
        });
    let (current_commit, current_body) = stamped_commit(store, current);
    Ok(ImplementationReportFacts {
        summary,
        current_report: current.id,
        current_commit,
        current_body,
        priors,
        ratifications,
        qa_return_rationale,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use devai_core::types::{DataCategory, Task};
    use devai_store::ArtifactDraft;

    /// R5 QA (rl/error-handling/rel/error-context): the collector must tell
    /// the three ways a `commit` can be ABSENT apart — readable-but-unnamed,
    /// corrupt bytes, missing blob — on a real store, not only in core
    /// fixtures. Each scenario flips the SAME report row; the typed outcome
    /// is what the gate then records (see the core workflow tests for the
    /// distinct phrases this feeds).
    #[test]
    fn body_read_classifies_readable_corrupt_and_missing_apart() {
        let dir = tempfile::TempDir::new().unwrap();
        let store = Store::open(dir.path()).unwrap();
        let task = Task::new("gate-read", "fixture", None, chrono::Utc::now());
        store.create_task(&task).unwrap();

        let publish_with = |body: &str| {
            store
                .publish_artifact(ArtifactDraft {
                    task_id: task.id,
                    kind: ArtifactKind::ImplementationReport,
                    name: "report.json".into(),
                    category: DataCategory::Artifact,
                    content: body.as_bytes().to_vec(),
                    media_type: "application/json".into(),
                    contract: None,
                    created_by: "impl".into(),
                    execution_id: None,
                    depends_on: vec![],
                })
                .unwrap()
        };

        // (1) READABLE with a commit — the normal @2 body.
        let art = publish_with(r#"{"commit":"c0ffee1","notes":"ok"}"#);
        let facts =
            collect_implementation_report_facts(&store, task.id, &art, &[art.clone()], None)
                .unwrap();
        assert_eq!(facts.current_commit.as_deref(), Some("c0ffee1"));
        assert_eq!(facts.current_body, ReportBodyRead::Readable);

        // (2) READABLE without a commit (read-only producer shape).
        let art2 = publish_with(r#"{"notes":"no commit field"}"#);
        let facts = collect_implementation_report_facts(
            &store,
            task.id,
            &art2,
            &[art.clone(), art2.clone()],
            None,
        )
        .unwrap();
        assert_eq!(facts.current_commit, None);
        assert_eq!(facts.current_body, ReportBodyRead::Readable);
        // The prior (art1, highest among others) keeps its readable commit.
        assert_eq!(facts.priors.len(), 1);
        assert_eq!(facts.priors[0].commit.as_deref(), Some("c0ffee1"));
        assert_eq!(facts.priors[0].body, ReportBodyRead::Readable);

        // (3) CORRUPT: the blob bytes exist but are not parseable JSON —
        // overwritten out-of-band, exactly the damage the gate must NAME.
        std::fs::write(store.blob_path(&art.sha256), b"{ truncated mid-wri").unwrap();
        let facts =
            collect_implementation_report_facts(&store, task.id, &art2, &[art.clone()], None)
                .unwrap();
        assert_eq!(facts.priors[0].commit, None, "corrupt names no commit");
        assert_eq!(
            facts.priors[0].body,
            ReportBodyRead::Corrupt,
            "corruption must NOT collapse into Missing or Readable"
        );

        // (4) MISSING: the blob file is gone from disk.
        std::fs::remove_file(store.blob_path(&art.sha256)).unwrap();
        let facts =
            collect_implementation_report_facts(&store, task.id, &art2, &[art.clone()], None)
                .unwrap();
        assert_eq!(facts.priors[0].commit, None);
        assert_eq!(
            facts.priors[0].body,
            ReportBodyRead::Missing,
            "an absent blob must NOT masquerade as corruption"
        );
    }
}
