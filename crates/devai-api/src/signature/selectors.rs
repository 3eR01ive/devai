//! The two artifact selectors of the signature read-plane. Both are pure
//! functions of stored rows, so they are tested without a store.
//!
//! The CURRENT spec selector is NOT here: it lives once in
//! `inbox::pick_spec_artifact` and every consumer — this read plane, the wait
//! row and the approve/reject refs — goes through it (rework R6, REQ-SIGN-07
//! AC3: no second selector).

use devai_core::types::{Artifact, ArtifactKind, ArtifactStatus};

/// Which REPORT the reviewer block presents: CURRENT when the task has one,
/// else the LAST by (seq, created_at) marked `current: false` — the same
/// ratified OQ-GATE-3 rule as the spec selector.
pub fn last_review_artifact(arts: &[Artifact]) -> Option<(&Artifact, bool)> {
    let reviews: Vec<&Artifact> = arts
        .iter()
        .filter(|a| a.kind == ArtifactKind::ReviewReport)
        .collect();
    if let Some(current) = reviews
        .iter()
        .find(|a| a.status == ArtifactStatus::Current)
        .copied()
    {
        return Some((current, true));
    }
    reviews
        .into_iter()
        .max_by_key(|a| (a.seq, a.created_at))
        .map(|a| (a, false))
}

/// The previous spec of the same task: the highest seq BELOW the presented
/// one (REQ-SIGN-07 AC7). No previous → `None` and the caller says so.
pub fn pick_previous_spec<'a>(arts: &'a [Artifact], seq: u32) -> Option<&'a Artifact> {
    arts.iter()
        .filter(|a| a.kind == ArtifactKind::RequirementsSpec && a.seq < seq)
        .max_by_key(|a| (a.seq, a.created_at))
}
