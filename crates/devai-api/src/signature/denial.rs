//! The refusal codes of the two INDEPENDENT signature rights (REQ-SIGN-07
//! AC1/AC2). The codes are DATA — `signature/labels.json`, the same file the
//! section's words come from (rework R8) — so a code cannot be spelled at a
//! return site without a word beside it. This module names only the MEANING:
//! the first failed conjunct decides which dictionary entry is returned.
//!
//! The UI reads the same data through the generated
//! `frontend/src/generated/signature-labels.ts`; the render-staleness test in
//! `frontend/tests/signature-denial-parity.test.ts` fails when that module
//! lags the data file, so the two halves cannot drift.

use devai_core::types::TaskState;

use super::types::SpecView;
use super::vocabulary::{denial_code, tokens};

/// The act is not signable from this state at all.
pub fn not_approval() -> &'static str {
    denial_code("not_approval")
}

/// No REQUIREMENTS_SPEC is published — there is nothing to approve.
pub fn no_spec() -> &'static str {
    denial_code("no_spec")
}

/// The presented spec is the last one, but no longer CURRENT.
pub fn spec_not_current() -> &'static str {
    denial_code("spec_not_current")
}

/// The task carries no approved requirements version yet.
pub fn no_requirements_version() -> &'static str {
    denial_code("no_requirements_version")
}

/// Every code the two functions below may return, in the ratified order —
/// read from the dictionary, so the answer and the UI share one list.
pub fn denial_codes() -> Vec<&'static str> {
    tokens("denial")
}

/// Why the approve is impossible, in the ratified order (REQ-SIGN-07 AC1):
/// the FIRST failed conjunct names the cause.
pub fn approve_denied(
    state: TaskState,
    spec: Option<&SpecView>,
    requirements_seq: u32,
) -> Option<&'static str> {
    if state != TaskState::Approval {
        return Some(not_approval());
    }
    let Some(spec) = spec else {
        return Some(no_spec());
    };
    if !spec.current {
        return Some(spec_not_current());
    }
    if requirements_seq == 0 {
        return Some(no_requirements_version());
    }
    None
}

/// The reject right is the state and nothing else (workflow.rs:452-457).
pub fn reject_denied(state: TaskState) -> Option<&'static str> {
    if state != TaskState::Approval {
        return Some(not_approval());
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every code the functions can return comes from the dictionary; the
    /// dictionary's own contents are pinned in `vocabulary::tests` against
    /// the ratified contract.
    #[test]
    fn every_returned_code_is_in_the_single_dictionary() {
        let codes = denial_codes();
        let states = [
            TaskState::Draft,
            TaskState::Analysis,
            TaskState::Review,
            TaskState::Approval,
            TaskState::Implementation,
            TaskState::Qa,
            TaskState::Rework,
            TaskState::WaitingForInput,
            TaskState::Paused,
            TaskState::Blocked,
            TaskState::Failed,
            TaskState::Accepted,
            TaskState::Cancelled,
        ];
        for state in states {
            if let Some(code) = reject_denied(state) {
                assert!(
                    codes.contains(&code),
                    "reject code {code} is not in the data dictionary"
                );
            }
            // spec=None covers `no_spec`; the two spec branches are covered
            // by the unit test of the read plane.
            if let Some(code) = approve_denied(state, None, 0) {
                assert!(
                    codes.contains(&code),
                    "approve code {code} is not in the data dictionary"
                );
            }
        }
        assert_eq!(codes.len(), 4, "the dictionary is the closed set");
    }
}
