//! The ONE vocabulary of the quality-evaluation pipeline (rework R3/R5/R8).
//!
//! The check states the evaluator emits and the item kinds the stored human
//! form carries used to be written out as string literals in four places —
//! the producer (`evaluation.rs`), the parser (`human_view.rs`), the stored
//! JSON-schema enum/description (`seed.rs`) and the frontend contract
//! (`types.ts`) — with no machine check tying them together, so a state the
//! producer emitted could silently fall into a catch-all (MECHANICAL_PASS did).
//! Now every surface reads THESE constants, and parity tests fail loudly when
//! one of them drifts:
//!
//! * add a `CheckState` variant → `human_view`'s exhaustive match stops
//!   compiling, and the schema-enum test in `devai-store` fails;
//! * add an `ItemKind` variant → the schema-description and `types.ts` parity
//!   tests fail.
//!
//! Unknown input is NEVER guessed into a known branch: [`CheckState::parse`]
//! returns `None` and the reader presents it as an explicitly named unknown.

/// States the evaluator writes into `CheckOutcome::state`. One variant per
/// string the producer emits; nothing else exists.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CheckState {
    /// Mechanical probes all passed — the platform's own 1.0.
    MechanicalPass,
    /// A mechanical probe failed: blocking, never a footnote.
    MechanicalFail,
    /// Rubric-reviewed and scored by the auditor.
    Scored,
    /// No score: probe did not run, evidence missing, or no auditor rating.
    Blocked,
    /// Left unsigned on purpose — the human's signature is the verdict.
    PendingHuman,
}

impl CheckState {
    /// Every state, in the order the wire vocabulary lists them.
    pub const ALL: [CheckState; 5] = [
        CheckState::MechanicalPass,
        CheckState::MechanicalFail,
        CheckState::Scored,
        CheckState::Blocked,
        CheckState::PendingHuman,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            CheckState::MechanicalPass => "MECHANICAL_PASS",
            CheckState::MechanicalFail => "MECHANICAL_FAIL",
            CheckState::Scored => "SCORED",
            CheckState::Blocked => "BLOCKED",
            CheckState::PendingHuman => "PENDING_HUMAN",
        }
    }

    /// Fail-closed parse: an unrecognised token is `None`, never a default.
    pub fn parse(raw: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|s| s.as_str() == raw)
    }
}

/// Why an item of the stored human form stands where it stands. This is the
/// `kind` token of `HumanItem`, read by the schema and (as a union) by the
/// UI; it is never re-derived from text at read time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ItemKind {
    /// A gate probe failed — the verdict goes back.
    Gate,
    /// A gate probe could not be performed.
    GateUnperformed,
    /// Unperformed weight above the cap: the verdict cannot be complete.
    Cap,
    /// Waiting for the human's signature.
    Pending,
    /// A floor was breached.
    Floor,
    /// A mechanical check failed.
    Mechanical,
    /// A scored point in the nuances band (ratified OQ-QH-1/2).
    Scored,
    /// Not performed — no score.
    Blocked,
    /// Kept in force by a named ratified decision (names only).
    Anchor,
    /// Scored 1.0 without a decision link (presented, importance 0).
    Passed,
    /// Scored state without a score — nothing to present but the fact.
    NoScore,
    /// A state this build does not recognise, shown with its raw token.
    Unknown,
}

impl ItemKind {
    pub const ALL: [ItemKind; 12] = [
        ItemKind::Gate,
        ItemKind::GateUnperformed,
        ItemKind::Cap,
        ItemKind::Pending,
        ItemKind::Floor,
        ItemKind::Mechanical,
        ItemKind::Scored,
        ItemKind::Blocked,
        ItemKind::Anchor,
        ItemKind::Passed,
        ItemKind::NoScore,
        ItemKind::Unknown,
    ];

    pub const fn as_str(self) -> &'static str {
        match self {
            ItemKind::Gate => "gate",
            ItemKind::GateUnperformed => "gate-unperformed",
            ItemKind::Cap => "cap",
            ItemKind::Pending => "pending",
            ItemKind::Floor => "floor",
            ItemKind::Mechanical => "mechanical",
            ItemKind::Scored => "scored",
            ItemKind::Blocked => "blocked",
            ItemKind::Anchor => "anchor",
            ItemKind::Passed => "passed",
            ItemKind::NoScore => "no-score",
            ItemKind::Unknown => "unknown",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_two_vocabularies_are_total_and_round_trip() {
        for s in CheckState::ALL {
            assert_eq!(CheckState::parse(s.as_str()), Some(s));
        }
        assert_eq!(CheckState::parse("SOMETHING_NEW"), None);
        let mut kinds: Vec<&str> = ItemKind::ALL.iter().map(|k| k.as_str()).collect();
        kinds.sort_unstable();
        kinds.dedup();
        assert_eq!(kinds.len(), ItemKind::ALL.len(), "kinds are distinct");
    }
}
