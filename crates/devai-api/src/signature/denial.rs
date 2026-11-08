//! The refusal codes of the two INDEPENDENT signature rights (REQ-SIGN-07
//! AC1/AC2) — spelled ONCE here (rework R7). `approve_denied` and
//! `reject_denied` return these constants, so a code cannot be added at a
//! return site without existing in this dictionary, and
//! frontend/tests/signature-denial-parity.test.ts pins the UI `DENIAL_LABELS`
//! keys against `DENIAL_CODES`: a new code lands on both sides or the test
//! fails.

use devai_core::types::TaskState;

use super::types::SpecView;

/// The act is not signable from this state at all.
pub const NOT_APPROVAL: &str = "not_approval";
/// No REQUIREMENTS_SPEC is published — there is nothing to approve.
pub const NO_SPEC: &str = "no_spec";
/// The presented spec is the last one, but no longer CURRENT.
pub const SPEC_NOT_CURRENT: &str = "spec_not_current";
/// The task carries no approved requirements version yet.
pub const NO_REQUIREMENTS_VERSION: &str = "no_requirements_version";

/// Every code the two functions below may return, in the ratified order.
/// The frontend parity test reads THIS array.
pub const DENIAL_CODES: &[&str] = &[
    NOT_APPROVAL,
    NO_SPEC,
    SPEC_NOT_CURRENT,
    NO_REQUIREMENTS_VERSION,
];

/// Why the approve is impossible, in the ratified order (REQ-SIGN-07 AC1):
/// the FIRST failed conjunct names the cause.
pub fn approve_denied(
    state: TaskState,
    spec: Option<&SpecView>,
    requirements_seq: u32,
) -> Option<&'static str> {
    if state != TaskState::Approval {
        return Some(NOT_APPROVAL);
    }
    let Some(spec) = spec else {
        return Some(NO_SPEC);
    };
    if !spec.current {
        return Some(SPEC_NOT_CURRENT);
    }
    if requirements_seq == 0 {
        return Some(NO_REQUIREMENTS_VERSION);
    }
    None
}

/// The reject right is the state and nothing else (workflow.rs:452-457).
pub fn reject_denied(state: TaskState) -> Option<&'static str> {
    if state != TaskState::Approval {
        return Some(NOT_APPROVAL);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R7: every code the functions can return is in the ONE dictionary, and
    /// the dictionary carries no code nothing can return.
    #[test]
    fn every_returned_code_is_in_the_single_dictionary() {
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
                    DENIAL_CODES.contains(&code),
                    "reject code {code} is not in DENIAL_CODES"
                );
            }
            // spec=None covers `no_spec`; the two spec branches are covered
            // by the unit test of the read plane.
            if let Some(code) = approve_denied(state, None, 0) {
                assert!(
                    DENIAL_CODES.contains(&code),
                    "approve code {code} is not in DENIAL_CODES"
                );
            }
        }
        assert_eq!(DENIAL_CODES.len(), 4, "the dictionary is the closed set");
    }
}
