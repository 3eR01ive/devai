//! The wire shapes of the signature read-plane (REQ-SIGN-07/11/13). Plain
//! data: every field is a stored fact or a projection over one, never a
//! re-derivation (store-is-cache). Mirrored by `frontend/src/types.ts`.

use chrono::{DateTime, Utc};
use devai_core::types::{Artifact, TaskState};
use serde::Serialize;

use super::body::BodyProblem;
use super::review::ReviewParagraphSource;
use super::spec::{ParagraphSource, RequirementView};

/// The APPROVAL signature card's spec projection (REQ-SIGN-07).
#[derive(Serialize, Clone, Debug)]
pub struct SpecView {
    pub artifact: Artifact,
    pub current: bool,
    pub human_paragraph: Option<String>,
    pub paragraph_source: ParagraphSource,
    pub requirements: Vec<RequirementView>,
    pub requirements_note: Option<String>,
    pub open_questions: Vec<String>,
    pub unavailable_note: Option<String>,
}

/// The reviewer block of the signature card (REQ-SIGN-11): the verdict is
/// the token AS STORED IN THE REPORT (never derived from the task state),
/// the paragraph is the report's own human paragraph or its stored summary.
#[derive(Serialize, Clone, Debug)]
pub struct ReviewView {
    pub artifact: Artifact,
    pub current: bool,
    pub verdict: Option<String>,
    pub human_paragraph: Option<String>,
    pub paragraph_source: ReviewParagraphSource,
    pub note: Option<String>,
}

/// The result of a signature act (REQ-SIGN-06): the newest stored fact among
/// the decision rows and the events of the act.
#[derive(Serialize, Clone, Debug)]
pub struct SignatureFact {
    /// "approval" | "rejection"
    pub kind: String,
    pub actor: String,
    pub at: DateTime<Utc>,
    pub rationale: Option<String>,
    /// Only from decision.refs → artifact REQUIREMENTS_SPEC → seq
    /// (REQ-SIGN-06 AC3); an unresolvable ref is `null`, never a guess.
    pub spec_seq: Option<u32>,
    /// "decision" | "event"
    pub source: String,
}

/// The human diff of one field line (REQ-SIGN-13).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct DiffLine {
    /// "+" added, "-" removed, " " unchanged context.
    pub mark: String,
    /// The readable field the line belongs to («абзац», «REQ-1», «AC»…).
    pub label: String,
    pub text: String,
}

/// Why a field diff could not be computed: which side(s) failed and how.
/// Typed so a consumer branches instead of parsing the sentence (rework R8).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct DiffRefusal {
    /// The refusal of the previous version, when it was that side.
    pub previous: Option<BodyProblem>,
    /// The refusal of the presented version, when it was that side.
    pub current: Option<BodyProblem>,
}

/// The diff of the spec against its previous version, compared by PARSED
/// FIELDS (never by raw JSON lines — reformatting is not a spec change).
#[derive(Serialize, Clone, Debug)]
pub struct SpecDiff {
    pub paragraph: Vec<DiffLine>,
    pub requirements: Vec<DiffLine>,
    /// Set when the comparison was refused: an unreadable body is never
    /// shown as an empty version (rework R3).
    pub refusal: Option<DiffRefusal>,
    /// The human sentence of `refusal` — the block header prints THIS.
    pub note: Option<String>,
}

/// The previous version of the spec plus its field diff. The reason a
/// comparison could not be made travels inside `diff.note` / `diff.refusal`
/// (rework R8: the cause reaches the answer, so no invented diff appears).
#[derive(Serialize, Clone, Debug)]
pub struct PreviousView {
    pub artifact: Artifact,
    pub diff: SpecDiff,
}

/// The whole `task.signature` answer (REQ-SIGN-07). One read, no per-item
/// `artifact_content`, no second selector.
#[derive(Serialize, Clone, Debug)]
pub struct SignatureView {
    pub task_id: String,
    pub state: TaskState,
    pub snapshot_at: DateTime<Utc>,
    pub may_approve: bool,
    pub may_approve_denied: Option<String>,
    pub may_reject: bool,
    pub may_reject_denied: Option<String>,
    pub spec: Option<SpecView>,
    pub review: Option<ReviewView>,
    pub review_note: Option<String>,
    pub previous: Option<PreviousView>,
    pub previous_note: Option<String>,
    pub signature: Option<SignatureFact>,
}
