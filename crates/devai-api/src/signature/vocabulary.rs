//! The token→word dictionaries of the signature section as DATA (rework R8):
//! `labels.json` beside this file is the ONE source of every word the section
//! prints, and it is shipped inside the binary (`include_str!`). There is no
//! const table in code any more — a new closed-set token lands in the data
//! file, and the tests below tie the data to the CLOSED TYPES (the Rust enums)
//! instead of to a hand-kept mirror list.
//!
//! The UI reads the SAME file: `frontend/scripts/gen-signature-labels.mjs`
//! renders it into `frontend/src/generated/signature-labels.ts`, and a stale
//! render fails the frontend parity test (rework R5). So one file feeds both
//! languages, and neither side can add a word the other never sees.
//!
//! Token sets that are produced by functions rather than by an enum
//! (`denial`, `signature_kind`, `signature_source`) are pinned to their
//! producers here as well: the denial functions return tokens READ FROM THIS
//! DATA (a missing entry panics at the call site, and the test below fails
//! first), the fact projector's two literals are asserted against the data.

use std::collections::BTreeMap;
use std::sync::LazyLock;

use serde::Deserialize;

/// One `token → word` pair of a closed set, in the order the section prints.
#[derive(Deserialize, Clone, Debug)]
pub struct Label {
    pub token: String,
    pub word: String,
}

type Sets = BTreeMap<String, Vec<Label>>;

/// The dictionaries, parsed once. A malformed shipped file is an invariant
/// violation, not a user error: it fails loudly at first use and the tests
/// below fail earlier.
static VOCABULARY: LazyLock<Sets> = LazyLock::new(|| {
    serde_json::from_str(include_str!("labels.json"))
        .expect("crates/devai-api/src/signature/labels.json must be a map of token/word arrays")
});

/// Every pair of one closed set; an unknown set name is a programmer error.
pub fn labels(set: &str) -> &'static [Label] {
    VOCABULARY
        .get(set)
        .map(Vec::as_slice)
        .unwrap_or_else(|| panic!("no `{set}` set in signature labels.json"))
}

/// The token a semantic name resolves to. The denial rules pass their own
/// meaning (`"no_spec"`), never a copy of the wire token, so the concrete
/// spelling has exactly one home — the data file.
pub fn denial_code(name: &str) -> &'static str {
    labels("denial")
        .iter()
        .find(|l| l.token == name)
        .map(|l| l.token.as_str())
        .unwrap_or_else(|| panic!("denial code `{name}` is missing from labels.json"))
}

/// The tokens of one set, in file order — what a parity test compares.
pub fn tokens(set: &str) -> Vec<&'static str> {
    labels(set).iter().map(|l| l.token.as_str()).collect()
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use devai_core::types::{ArtifactStatus, Verdict};

    use super::super::review::ReviewParagraphSource;
    use super::super::spec::ParagraphSource;
    use super::*;

    fn set_of(set: &str) -> BTreeSet<&'static str> {
        tokens(set).into_iter().collect()
    }

    fn serde_token<T: serde::Serialize>(value: T) -> String {
        serde_json::to_value(value)
            .expect("every vocabulary enum is serializable")
            .as_str()
            .expect("every vocabulary enum is a string on the wire")
            .to_string()
    }

    /// REQ-SIGN-02 AC4: the status words cover EXACTLY the closed enum —
    /// adding an `ArtifactStatus` variant fails here until the data file
    /// gains its word.
    #[test]
    fn artifact_status_words_are_exactly_the_closed_enum() {
        let expected: BTreeSet<&str> = ArtifactStatus::ALL.iter().map(|s| s.as_str()).collect();
        assert_eq!(set_of("artifact_status"), expected);
    }

    /// The paragraph-source tokens are the serde spelling of the enum, so the
    /// data can never drift from what the answer actually serializes.
    #[test]
    fn paragraph_source_words_follow_their_enums() {
        let spec: BTreeSet<String> = ParagraphSource::ALL
            .iter()
            .map(|v| serde_token(*v))
            .collect();
        assert_eq!(
            set_of("paragraph_source"),
            spec.iter().map(String::as_str).collect()
        );
        let review: BTreeSet<String> = ReviewParagraphSource::ALL
            .iter()
            .map(|v| serde_token(*v))
            .collect();
        assert_eq!(
            set_of("review_paragraph_source"),
            review.iter().map(String::as_str).collect()
        );
    }

    /// A review report carries a lowercase verdict (REQ-SIGN-11 AC2). Every
    /// word here must be a token `Verdict::from_token` understands, and every
    /// verdict a report may carry — `returned` is a workflow verdict, never a
    /// report verdict — must have a word.
    #[test]
    fn review_verdict_words_are_the_report_vocabulary() {
        let expected: BTreeSet<String> = Verdict::ALL
            .iter()
            .map(|v| v.as_str().to_ascii_lowercase())
            .filter(|v| v != "returned")
            .collect();
        assert_eq!(
            set_of("review_verdict"),
            expected.iter().map(String::as_str).collect()
        );
        for token in tokens("review_verdict") {
            assert!(
                Verdict::from_token(token).is_some(),
                "the verdict token `{token}` is not in the domain vocabulary"
            );
        }
    }

    /// The denial rules return tokens from the data; the set itself is the
    /// ratified four (REQ-SIGN-07 AC1/AC2), pinned as the CONTRACT here.
    #[test]
    fn denial_words_are_exactly_the_ratified_codes() {
        let ratified: BTreeSet<&str> = [
            "not_approval",
            "no_spec",
            "spec_not_current",
            "no_requirements_version",
        ]
        .into_iter()
        .collect();
        assert_eq!(set_of("denial"), ratified);
        for name in &ratified {
            assert_eq!(denial_code(name), *name, "the data spells the code");
        }
    }

    /// The two sets the fact projector produces (§ REQ-SIGN-06) must each
    /// have a word, or the result panel would print a raw token.
    #[test]
    fn produced_signature_tokens_have_words() {
        assert_eq!(
            set_of("signature_kind"),
            ["approval", "rejection"].into_iter().collect()
        );
        assert_eq!(
            set_of("signature_source"),
            ["decision", "event"].into_iter().collect()
        );
    }

    /// A word is never blank — a blank label would leave the operator with
    /// nothing where the section prints a fact.
    #[test]
    fn no_word_is_blank() {
        for (set, pairs) in VOCABULARY.iter() {
            assert!(!pairs.is_empty(), "set `{set}` is empty");
            for pair in pairs {
                assert!(
                    !pair.token.trim().is_empty() && !pair.word.trim().is_empty(),
                    "set `{set}` carries a blank token or word: {:?}",
                    pair
                );
            }
        }
    }
}
