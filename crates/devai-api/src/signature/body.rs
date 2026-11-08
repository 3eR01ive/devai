//! Reading ONE stored artifact body as text — with a TYPED refusal
//! (rework R8: the old `Result<&str, &str>` boundary dropped the cause, so an
//! unreadable previous spec silently turned into a plausible empty one).
//!
//! The failure never travels as a bare string: [`BodyError`] is the read
//! boundary, [`BodyProblem`] is the same fact on the wire (`kind` + `reason`),
//! so a consumer can BRANCH on it instead of printing it.

use devai_core::types::Artifact;
use devai_store::Store;
use serde::Serialize;

/// Why the stored bytes are not text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum BodyError {
    /// The blob is not UTF-8; raw bytes are never rendered (REQ-SIGN-13).
    NotText { size: u64 },
    /// The store refused to read the blob; the message is the store's own.
    Unreadable { message: String },
}

impl BodyError {
    /// The human sentence for this failure — one wording, used by every
    /// projection (the card, the CLI and the diff note cannot drift).
    pub fn reason(&self) -> String {
        match self {
            BodyError::NotText { size } => format!("артефакт бинарный (не UTF-8, {size} байт)"),
            BodyError::Unreadable { message } => {
                format!("артефакт не читается из стора: {message}")
            }
        }
    }

    /// The wire shape of this failure.
    pub fn problem(&self) -> BodyProblem {
        let kind = match self {
            BodyError::NotText { .. } => BodyProblemKind::NotText,
            BodyError::Unreadable { .. } => BodyProblemKind::Unreadable,
        };
        BodyProblem {
            kind,
            reason: self.reason(),
        }
    }
}

/// Machine class of a refused body — the token a consumer branches on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum BodyProblemKind {
    NotText,
    Unreadable,
    NotJson,
}

/// A body that never became a parsed document: unreadable bytes or text that
/// is not JSON. `reason` is the sentence shown to the operator.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct BodyProblem {
    pub kind: BodyProblemKind,
    pub reason: String,
}

impl BodyProblem {
    pub fn not_json() -> Self {
        BodyProblem {
            kind: BodyProblemKind::NotJson,
            reason: "тело артефакта не разбирается как JSON".to_string(),
        }
    }

    pub fn reason(&self) -> &str {
        &self.reason
    }
}

/// The stored body of one artifact as text, or the typed reason it is not.
pub fn body_of(store: &Store, artifact: &Artifact) -> Result<String, BodyError> {
    let bytes = store
        .read_artifact_body(artifact)
        .map_err(|e| BodyError::Unreadable {
            message: e.to_string(),
        })?;
    String::from_utf8(bytes).map_err(|_| BodyError::NotText {
        size: artifact.size,
    })
}

/// Borrow a read body as the `Result<&str, &BodyError>` the parse functions
/// take — the bytes are held once, no second copy and no string error.
pub fn body_text(body: &Result<String, BodyError>) -> Result<&str, &BodyError> {
    body.as_ref().map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// R8: the refusal is typed AND carries the reason — the diff names the
    /// failing side from this value, never from a stringified error.
    #[test]
    fn a_failed_read_is_typed_and_carries_its_reason() {
        let not_text = BodyError::NotText { size: 12 };
        assert_eq!(not_text.problem().kind, BodyProblemKind::NotText);
        assert!(not_text.reason().contains("12"));
        let unreadable = BodyError::Unreadable {
            message: "диск".into(),
        };
        assert_eq!(unreadable.problem().kind, BodyProblemKind::Unreadable);
        assert!(unreadable.reason().contains("диск"));
        assert_eq!(BodyProblem::not_json().kind, BodyProblemKind::NotJson);
    }
}
