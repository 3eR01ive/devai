//! Assembling the whole `task.signature` answer from stored facts
//! (REQ-SIGN-07) — split by responsibility so no step hides another
//! (rework R4/R5): one spec selection, one parse per body, one reviewer
//! block, one previous block, one signature fact. No writes, no per-item
//! `artifact_content`, no second selector.

use chrono::Utc;
use devai_core::types::{Artifact, Task};
use devai_store::Store;

use crate::ApiResult;
use crate::inbox::pick_spec_artifact;

use super::body::{body_of, body_text};
use super::denial::{approve_denied, reject_denied};
use super::fact::newest_signature_fact;
use super::review::parse_review_body;
use super::selectors::{last_review_artifact, pick_previous_spec};
use super::spec::{ParsedSpec, parse_spec_body};
use super::types::{PreviousView, ReviewView, SignatureView, SpecView};

/// The spec the card presents, with the flag and the ONE parse of its body.
struct PresentedSpec {
    artifact: Artifact,
    current: bool,
    parsed: ParsedSpec,
}

impl PresentedSpec {
    fn view(&self) -> SpecView {
        self.parsed.view(&self.artifact, self.current)
    }
}

/// The whole answer, one read (REQ-SIGN-07).
pub fn signature_view(store: &Store, task: Task) -> ApiResult<SignatureView> {
    let arts = store.artifacts(task.id)?;
    let spec = presented_spec(store, &arts, &task);
    let spec_view = spec.as_ref().map(PresentedSpec::view);
    let (review, review_note) = review_block(store, &arts);
    let (previous, previous_note) = previous_block(store, &arts, &task, spec.as_ref());
    let signature = newest_signature_fact(store, task.id, &arts)?;

    let approval_denied = approve_denied(task.state, spec_view.as_ref(), task.requirements_seq);
    let rejection_denied = reject_denied(task.state);
    Ok(SignatureView {
        task_id: task.id.to_string(),
        state: task.state,
        snapshot_at: Utc::now(),
        may_approve: approval_denied.is_none(),
        may_approve_denied: approval_denied.map(str::to_string),
        may_reject: rejection_denied.is_none(),
        may_reject_denied: rejection_denied.map(str::to_string),
        spec: spec_view,
        review,
        review_note,
        previous,
        previous_note,
        signature,
    })
}

/// CURRENT spec, else the last one with `current: false` — through the ONE
/// selector (`inbox::pick_spec_artifact`), never a private copy.
fn presented_spec(store: &Store, arts: &[Artifact], task: &Task) -> Option<PresentedSpec> {
    let (artifact, current) = pick_spec_artifact(arts)?;
    let body = body_of(store, artifact);
    let parsed = parse_spec_body(body_text(&body), task);
    Some(PresentedSpec {
        artifact: artifact.clone(),
        current,
        parsed,
    })
}

/// The reviewer block — never absent from the answer: `review_note` stands in
/// when no report exists (REQ-SIGN-11 branch 1).
fn review_block(store: &Store, arts: &[Artifact]) -> (Option<ReviewView>, Option<String>) {
    let Some((artifact, current)) = last_review_artifact(arts) else {
        return (None, Some("отчёта ревьювера нет".to_string()));
    };
    let body = body_of(store, artifact);
    let parsed = parse_review_body(body_text(&body));
    (
        Some(ReviewView {
            artifact: artifact.clone(),
            current,
            verdict: parsed.verdict,
            human_paragraph: parsed.human_paragraph,
            paragraph_source: parsed.paragraph_source,
            note: parsed.note,
        }),
        None,
    )
}

/// The previous version and its field diff; a refusal travels inside the
/// diff (empty lines + note/refusal), never as an invented comparison.
fn previous_block(
    store: &Store,
    arts: &[Artifact],
    task: &Task,
    spec: Option<&PresentedSpec>,
) -> (Option<PreviousView>, Option<String>) {
    let Some(spec) = spec else {
        return (None, Some("прошлой версии нет".to_string()));
    };
    let Some(prev) = pick_previous_spec(arts, spec.artifact.seq) else {
        return (None, Some("прошлой версии нет".to_string()));
    };
    let body = body_of(store, prev);
    let parsed = parse_spec_body(body_text(&body), task);
    (
        Some(PreviousView {
            artifact: prev.clone(),
            diff: super::diff::diff_specs(&parsed, &spec.parsed),
        }),
        None,
    )
}
