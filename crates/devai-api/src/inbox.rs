//! The human-wait section's data plane (task 01a1b0ea, REQ-GATE-01..10).
//!
//! The section is ONE batch read, not a fan-out: `Api::inbox_snapshot`
//! answers with the whole home in one operation — the declarative demand
//! table (`config/workflow/inbox-waits.yaml`) plus, per task, its questions
//! and the click-presentation of its gate (spec body for APPROVAL, quality
//! report for QA). The fan-out this replaces, and why it had to go: the UI
//! used to issue `question.list` per active task, `task.status +
//! artifact.list + artifact.cat_text` per APPROVAL and `task.report` per QA
//! — the number of IPC calls grew with the row count. Now the UI makes
//! exactly one call per load whatever the number of rows; inside the API the
//! store is read in bulk snapshots (`list_tasks`, `questions_all`,
//! `artifacts_all`) instead of per-task queries.
//!
//! Cost is linear and bounded by the home itself, with no declared task
//! cap needed: three store snapshots plus one grouping pass over questions
//! and artifacts (O(T+Q+A)) and one hash lookup per task — the per-task
//! loop never rescans either table.
//!
//! Membership is NOT decided here: the section groups rows by the table's
//! `group` and by exact stored tokens on the UI side (one place,
//! `devai-ui::commands::wait_group_of`). This module only carries data.
//!
//! Source-of-operations note (SPEC-FIDELITY, RATIFIED). The spec named
//! `task.list`-composition + `artifact.cat_text` as the section's source
//! reads; `Api::inbox_snapshot` replaces them with this one batch read —
//! one call instead of N+1. The operator ratified the replacement in words
//! («Ратифицирую», 07.11); that word is stored in this task's QA-return
//! rationale record (it appears as R5 in the 20:25 return and as R6 in the
//! 20:48 one — the platform has no separate decision row for it), so the
//! citation here points at the QA-return rationale, not at a decision id.
//! The old operations are NOT to be restored.

use chrono::{DateTime, Utc};
use devai_core::inbox::InboxWaitPolicy;
use devai_core::types::{Artifact, ArtifactKind, ArtifactStatus, Question, Task};
use serde::Serialize;

use crate::{ApiResult, QualityReportView};

/// The whole section input in one snapshot: policy + per-task facts.
#[derive(Serialize, Clone, Debug)]
pub struct InboxSnapshotView {
    pub snapshot_at: DateTime<Utc>,
    /// The declarative demand table as loaded for THIS home (user-editable
    /// `config/workflow/inbox-waits.yaml`, factory bytes when absent). The
    /// renderer is generic over it — no wording lives in code.
    pub policy: InboxWaitPolicy,
    /// Every task of the home (the section picks its rows from these), each
    /// with its stored questions and, for a gate state, its presentation.
    pub tasks: Vec<InboxTaskFacts>,
}

/// Per-task facts of the section. Questions are the stored rows as-is (the
/// UI filters the OPEN token — the single filtering site); `spec`/`report`
/// are present only for the stored gate states that present them.
#[derive(Serialize, Clone, Debug)]
pub struct InboxTaskFacts {
    pub task: Task,
    pub questions: Vec<Question>,
    /// APPROVAL click-presentation (REQ-GATE-04/05 + ratified OQ-GATE-3/4).
    pub spec: Option<SpecFacts>,
    /// QA click-presentation: the CURRENT quality report.
    pub report: Option<QualityReportView>,
    /// Honest platform text when a QA row has no signable report.
    pub report_note: Option<String>,
}

/// The spec artifact the APPROVAL gate presents, plus its body. `current`
/// is false when there is no CURRENT REQUIREMENTS_SPEC and this is the LAST
/// spec artifact — the row says so and approves are impossible (ratified
/// OQ-GATE-3: not a silent hole, not a fake).
#[derive(Serialize, Clone, Debug)]
pub struct SpecFacts {
    pub artifact: Artifact,
    pub current: bool,
    pub body: SpecBodyView,
}

/// The spec body as read from the blob store: text, an honest binary fact
/// with the stored size (raw bytes are never rendered), or the platform's
/// error text — the presentation always has something honest to show.
#[derive(Serialize, Clone, Debug)]
#[serde(tag = "shape", rename_all = "lowercase")]
pub enum SpecBodyView {
    Text { text: String },
    Binary { size: u64 },
    Error { message: String },
}

impl SpecBodyView {
    /// The body as the presentation parser wants it: `Ok(text)` or
    /// `Err(honest reason)` for the business-description fallback.
    pub fn as_text(&self) -> Result<&str, String> {
        match self {
            SpecBodyView::Text { text } => Ok(text),
            SpecBodyView::Binary { size } => {
                Err(format!("артефакт бинарный (не UTF-8, {size} байт)"))
            }
            SpecBodyView::Error { message } => Err(message.clone()),
        }
    }
}

/// The factory demand table (`config/workflow/inbox-waits.yaml` embedded),
/// exposed without opening a home: a fresh home renders exactly these
/// rows, and the section's composer tests pin their behavior against the
/// same data the window uses (no test-only copy of the wording).
pub fn factory_inbox_wait_policy() -> ApiResult<InboxWaitPolicy> {
    Ok(devai_store::config::factory_inbox_wait_policy()?)
}

/// Which REQUIREMENTS_SPEC an APPROVAL row presents, and whether it is the
/// CURRENT one: CURRENT when the task has one, else the LAST spec artifact
/// (`current: false` — ratified OQ-GATE-3: the row says «not current, the
/// approve is impossible» instead of showing nothing). Selection is a pure
/// function of stored artifact rows, so it is tested without a store.
pub fn pick_spec_artifact(arts: &[Artifact]) -> Option<(&Artifact, bool)> {
    let specs: Vec<&Artifact> = arts
        .iter()
        .filter(|a| a.kind == ArtifactKind::RequirementsSpec)
        .collect();
    if let Some(current) = specs
        .iter()
        .find(|a| a.status == ArtifactStatus::Current)
        .copied()
    {
        return Some((current, true));
    }
    specs
        .into_iter()
        .max_by_key(|a| (a.seq, a.created_at))
        .map(|a| (a, false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use devai_core::types::{ArtifactStatus, DataCategory, new_id};

    fn spec(seq: u32, status: ArtifactStatus) -> Artifact {
        Artifact {
            id: new_id(),
            task_id: new_id(),
            kind: ArtifactKind::RequirementsSpec,
            name: format!("spec-v{seq}.json"),
            seq,
            category: DataCategory::Artifact,
            status,
            sha256: "0".repeat(64),
            size: 2,
            media_type: "application/json".into(),
            contract: None,
            created_by: "user".into(),
            execution_id: None,
            depends_on: vec![],
            created_at: "2026-11-06T12:00:00Z"
                .parse::<chrono::DateTime<chrono::Utc>>()
                .unwrap(),
        }
    }

    /// Ratified OQ-GATE-3: CURRENT wins; with no CURRENT the LAST spec rides
    /// with `current: false`; the row is never empty when a spec exists.
    #[test]
    fn spec_selection_prefers_current_then_the_last() {
        let current = spec(3, ArtifactStatus::Current);
        let stale = spec(2, ArtifactStatus::Stale);
        let arts = vec![stale.clone(), current.clone()];
        let (picked, is_current) = pick_spec_artifact(&arts).unwrap();
        assert_eq!(picked.id, current.id);
        assert!(is_current);

        // No CURRENT row: the highest seq is the one presented, labelled.
        let arts = vec![
            spec(1, ArtifactStatus::Superseded),
            spec(2, ArtifactStatus::Stale),
        ];
        let (picked, is_current) = pick_spec_artifact(&arts).unwrap();
        assert_eq!(picked.seq, 2);
        assert!(!is_current, "the not-current label rides on the same view");

        // No spec at all: nothing to present (the caller keeps the empty
        // presentation honest rather than inventing one).
        assert!(pick_spec_artifact(&[]).is_none());
    }

    /// A non-spec artifact never becomes the APPROVAL presentation.
    #[test]
    fn spec_selection_ignores_other_kinds() {
        let mut report = spec(1, ArtifactStatus::Current);
        report.kind = ArtifactKind::QualityReport;
        assert!(pick_spec_artifact(&[report]).is_none());
    }
}
