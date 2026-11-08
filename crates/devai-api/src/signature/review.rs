//! Parsing a stored REVIEW_REPORT body (REQ-SIGN-11). The verdict is the
//! STORED token — `approved`/`needs_revision`/`rejected` in the report's own
//! spelling — never derived from the task state; the paragraph is the
//! report's own `human_paragraph` (@2), else its stored `summary` (@1),
//! labelled. Every branch yields text or an honest note (never an empty
//! block, never a red error box).

use serde::Serialize;

use super::body::BodyError;

/// Origin of the reviewer paragraph (REQ-SIGN-11 AC3): exactly three values,
/// `none` only for a missing/unreadable report.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ReviewParagraphSource {
    ReviewReport,
    Summary,
    None,
}

/// One parsed reviewer report — the fields the card prints, nothing derived.
#[derive(Clone, Debug)]
pub struct ReviewParse {
    pub verdict: Option<String>,
    pub paragraph_source: ReviewParagraphSource,
    pub human_paragraph: Option<String>,
    pub note: Option<String>,
}

impl ReviewParse {
    /// Every failure branch: no verdict, no paragraph, one visible reason.
    fn unreadable(note: String) -> Self {
        ReviewParse {
            verdict: None,
            paragraph_source: ReviewParagraphSource::None,
            human_paragraph: None,
            note: Some(note),
        }
    }
}

/// Parse the reviewer report body (REQ-SIGN-11), in the five branches the
/// requirement enumerates: @2 paragraph, @1 summary, empty paragraph =
/// absent, unreadable body, readable body without a verdict.
pub fn parse_review_body(body: Result<&str, &BodyError>) -> ReviewParse {
    let value = match review_document(body) {
        Ok(value) => value,
        Err(note) => return ReviewParse::unreadable(note),
    };
    let verdict = value
        .get("verdict")
        .and_then(serde_json::Value::as_str)
        .map(str::to_string);
    let missing_verdict = verdict
        .is_none()
        .then(|| "в отчёте ревьювера нет вердикта".to_string());
    let (paragraph_source, human_paragraph, paragraph_note) = review_paragraph(&value);
    ReviewParse {
        verdict,
        paragraph_source,
        human_paragraph,
        note: paragraph_note.or(missing_verdict),
    }
}

/// Text → JSON value; the refusal keeps the read cause when there is one.
fn review_document(body: Result<&str, &BodyError>) -> Result<serde_json::Value, String> {
    let text = body.map_err(|e| format!("тело отчёта не читается: {}", e.reason()))?;
    serde_json::from_str(text)
        .map_err(|_| "тело отчёта ревьювера не разбирается как JSON".to_string())
}

/// The paragraph branch: the report's own paragraph, else the stored summary
/// (labelled), else nothing with no note (the caller adds the verdict note).
fn review_paragraph(
    value: &serde_json::Value,
) -> (ReviewParagraphSource, Option<String>, Option<String>) {
    if let Some(paragraph) = field_text(value, "human_paragraph") {
        return (ReviewParagraphSource::ReviewReport, Some(paragraph), None);
    }
    match field_text(value, "summary") {
        Some(summary) => (
            ReviewParagraphSource::Summary,
            Some(summary),
            Some("абзаца ревьювера в отчёте нет".to_string()),
        ),
        None => (ReviewParagraphSource::None, None, None),
    }
}

/// A stored non-blank string field, trimmed.
fn field_text(value: &serde_json::Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}
