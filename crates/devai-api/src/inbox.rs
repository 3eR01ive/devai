//! The human-wait section's data plane (task 01a1b0ea, REQ-GATE-01..10).
//!
//! The section is ONE batch read, not a fan-out: `Api::inbox_snapshot`
//! answers with the whole home in one operation — the declarative demand
//! table (`config/workflow/inbox-waits.yaml`) plus, per task, its questions
//! and the click-presentation of its gate (spec body for APPROVAL, quality
//! report for QA). R2 of the QA return: the UI used to issue
//! `question.list` per active task, `task.status + artifact.list +
//! artifact.cat_text` per APPROVAL and `task.report` per QA — the number of
//! IPC calls grew with the row count. Now the UI makes exactly one call per
//! load whatever the number of rows (O(1) per row); inside the API the store
//! is read in bulk snapshots (`list_tasks`, `questions_all`,
//! `artifacts_all`) instead of per-task queries.
//!
//! Cost is linear and bounded by the home itself, with no declared task
//! cap needed: three store snapshots plus one grouping pass over questions
//! and artifacts (O(T+Q+A)) and one hash lookup per task — the per-task
//! loop never rescans either table (R2 of the QA return).
//!
//! Membership is NOT decided here: the section groups rows by the table's
//! `group` and by exact stored tokens on the UI side (one place,
//! `devai-ui::commands::wait_group_of`). This module only carries data.
//!
//! R5 of the QA return — SPEC-FIDELITY, RATIFIED: the spec named
//! `task.list`-composition + `artifact.cat_text` as the source operations;
//! this batch read replaces them with `inbox.snapshot`. The operator
//! ratified the replacement in words ("ратифицирую", 07.11, R5 of the QA
//! return) as an improvement — one call instead of N+1. The old operations
//! are NOT to be restored; this note records the ratification in code.

use chrono::{DateTime, Utc};
use devai_core::inbox::InboxWaitPolicy;
use devai_core::types::{Artifact, Question, Task};
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
