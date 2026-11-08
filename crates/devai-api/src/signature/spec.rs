//! Parsing a stored REQUIREMENTS_SPEC body — the ONE rule set behind both
//! projections (REQ-SIGN-12 AC5): the human-wait row (`SpecPresentation`,
//! shape unchanged) and the signature card (`SpecView`).
//!
//! Nothing here rewrites stored data: the body is read, parsed, projected.
//! Fail-closed rules (ratified OQ-GATE-4, carried into the API): an absent
//! paragraph falls back to the stored business description WITH a label; an
//! absent structure keeps the paragraph and says so; a non-string array item
//! is printed as the anomaly it is, never stringified into plausible text
//! (rework R10).

use devai_core::types::{Artifact, Task};
use serde::Serialize;

use super::body::{BodyError, BodyProblem};
use super::types::SpecView;

/// A REQUIREMENTS_SPEC with no `requirements` array at all.
const NO_REQUIREMENTS_ARRAY: &str =
    "в теле спеки нет массива требований — формальной структуры нет";
/// A `requirements` array that parsed to nothing.
const EMPTY_REQUIREMENTS: &str = "массив требований пуст — формальной структуры нет";
/// Requirements present, but no `acceptance_criteria` array.
const NO_CRITERIA: &str = "в требованиях нет acceptance criteria — структура неполная";
/// The ratified OQ-GATE-4 wording for a pre-protocol body.
const PRE_PROTOCOL: &str =
    "аналитик абзац не писал — спека до протокола; подставляю бизнес-описание задачи";

/// The REQUIREMENTS_SPEC presentation the human-wait row renders — the
/// shape the window published before the moves (REQ-SIGN-10 AC7: unchanged
/// on the wire, `ipc_shapes.json` passes as-is).
#[derive(Serialize, Clone, Debug)]
pub struct SpecPresentation {
    pub artifact: Artifact,
    /// false when there is no CURRENT REQUIREMENTS_SPEC and this is the
    /// LAST spec artifact shown with an honest label (ratified OQ-GATE-3).
    pub current: bool,
    pub human_paragraph: Option<String>,
    /// Which text stands in the paragraph slot (ratified OQ-GATE-4).
    pub paragraph_source: ParagraphSource,
    /// The REQUIREMENTS_SPEC `open_questions` array — questions of the
    /// document, NOT registry questions.
    pub open_questions: Vec<String>,
    /// Why the paragraph could not come from the spec body.
    pub unavailable_note: Option<String>,
}

/// Origin of the paragraph text shown for an APPROVAL row.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum ParagraphSource {
    /// The spec's own top-level `human_paragraph`.
    #[default]
    Spec,
    /// Ratified OQ-GATE-4: the spec predates the paragraph protocol (or its
    /// body is unreadable) — the stored task business description stands
    /// in, labelled as such.
    BusinessDescription,
}

impl ParagraphSource {
    /// The closed set, so the label parity test (rework R8) compares the
    /// data file against the type instead of a hand-kept list.
    pub const ALL: &'static [ParagraphSource] = &[Self::Spec, Self::BusinessDescription];
}

/// One requirement of the spec body as stored (REQ-SIGN-02): formulation and
/// criteria are printed verbatim, never translated or rewritten.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct RequirementView {
    pub id: String,
    pub statement: String,
    pub acceptance_criteria: Vec<String>,
    pub constraints: Vec<String>,
}

/// The ONE parsed value every projection is built from (REQ-SIGN-12 AC5):
/// failing to read/parse the body only changes `unavailable_note` and
/// `body_problem`, never the shape, so the card and the wait row cannot
/// disagree.
#[derive(Clone, Debug, Default)]
pub struct ParsedSpec {
    pub human_paragraph: Option<String>,
    pub paragraph_source: ParagraphSource,
    pub unavailable_note: Option<String>,
    pub open_questions: Vec<String>,
    pub requirements: Vec<RequirementView>,
    pub requirements_note: Option<String>,
    /// Set only when the body never became a document (unreadable or not
    /// JSON). A readable pre-protocol body leaves it `None`: its structure
    /// still diffs honestly (rework R3/R8).
    pub body_problem: Option<BodyProblem>,
}

impl ParsedSpec {
    /// The wait-row projection — the pre-move shape, unchanged.
    pub fn presentation(&self, artifact: &Artifact, current: bool) -> SpecPresentation {
        SpecPresentation {
            artifact: artifact.clone(),
            current,
            human_paragraph: self.human_paragraph.clone(),
            paragraph_source: self.paragraph_source,
            open_questions: self.open_questions.clone(),
            unavailable_note: self.unavailable_note.clone(),
        }
    }

    /// The signature-card projection — adds the formal structure.
    pub fn view(&self, artifact: &Artifact, current: bool) -> SpecView {
        SpecView {
            artifact: artifact.clone(),
            current,
            human_paragraph: self.human_paragraph.clone(),
            paragraph_source: self.paragraph_source,
            requirements: self.requirements.clone(),
            requirements_note: self.requirements_note.clone(),
            open_questions: self.open_questions.clone(),
            unavailable_note: self.unavailable_note.clone(),
        }
    }
}

/// The fields a READABLE body contributes, before the fallback rule applies.
#[derive(Default)]
struct SpecFields {
    human_paragraph: Option<String>,
    open_questions: Vec<String>,
    requirements: Vec<RequirementView>,
    requirements_note: Option<String>,
}

/// Parse the spec artifact body. The JSON is read, never rewritten; the
/// array shapes are the seeded contract's (`open_questions` is an array of
/// strings, crates/devai-store/src/seed.rs:312). Ratified OQ-GATE-4: absent
/// paragraph → the stored
/// task business description with an honest source + note.
pub fn parse_spec_body(body: Result<&str, &BodyError>, task: &Task) -> ParsedSpec {
    let fields = match spec_document(body) {
        Ok(fields) => fields,
        Err(problem) => {
            let note = format!("{}; показываю бизнес-описание задачи", problem.reason());
            return fallback(task, SpecFields::default(), Some(problem), note);
        }
    };
    if fields.human_paragraph.is_none() {
        return fallback(task, fields, None, PRE_PROTOCOL.to_string());
    }
    ParsedSpec {
        human_paragraph: fields.human_paragraph,
        paragraph_source: ParagraphSource::Spec,
        unavailable_note: None,
        open_questions: fields.open_questions,
        requirements: fields.requirements,
        requirements_note: fields.requirements_note,
        body_problem: None,
    }
}

/// The spec-card convenience wrapper over [`parse_spec_body`] — kept for the
/// call sites that only need the wait-row projection.
pub fn spec_presentation(
    artifact: &Artifact,
    body: Result<&str, &BodyError>,
    task: &Task,
    current: bool,
) -> SpecPresentation {
    parse_spec_body(body, task).presentation(artifact, current)
}

/// Text → JSON value → the spec fields; a failure is typed, never silent.
fn spec_document(body: Result<&str, &BodyError>) -> Result<SpecFields, BodyProblem> {
    let text = body.map_err(BodyError::problem)?;
    let value =
        serde_json::from_str::<serde_json::Value>(text).map_err(|_| BodyProblem::not_json())?;
    Ok(spec_fields(&value))
}

fn spec_fields(value: &serde_json::Value) -> SpecFields {
    SpecFields {
        human_paragraph: spec_paragraph(value),
        open_questions: text_array(value.get("open_questions")).items,
        ..spec_requirements_of(value)
    }
}

/// The ONE constructor of the fail-closed shape: the presentation is never
/// empty, and the branches differ only in `note` and `body_problem`.
fn fallback(
    task: &Task,
    fields: SpecFields,
    problem: Option<BodyProblem>,
    note: String,
) -> ParsedSpec {
    ParsedSpec {
        human_paragraph: non_empty(task.business_description.as_str()).map(str::to_string),
        paragraph_source: ParagraphSource::BusinessDescription,
        unavailable_note: Some(note),
        open_questions: fields.open_questions,
        requirements: fields.requirements,
        requirements_note: fields.requirements_note,
        body_problem: problem,
    }
}

/// The document's own top-level `human_paragraph`, trimmed.
fn spec_paragraph(value: &serde_json::Value) -> Option<String> {
    value
        .get("human_paragraph")
        .and_then(serde_json::Value::as_str)
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The formal structure (REQ-SIGN-02): printed verbatim, in array order. A
/// body without a requirements array keeps the paragraph and says honestly
/// that the structure is absent — no requirement is invented.
fn spec_requirements_of(value: &serde_json::Value) -> SpecFields {
    let Some(items) = value.get("requirements").and_then(|v| v.as_array()) else {
        return SpecFields {
            requirements_note: Some(NO_REQUIREMENTS_ARRAY.to_string()),
            ..SpecFields::default()
        };
    };
    let mut views = Vec::new();
    let mut missing_criteria = false;
    let mut anomalies = 0usize;
    for item in items {
        match requirement_of(item) {
            Ok((view, item_anomalies, criteria_listed)) => {
                missing_criteria |= !criteria_listed;
                anomalies += item_anomalies;
                views.push(view);
            }
            // A non-object element is an ANOMALY, not a requirement: it is
            // counted and named in the note instead of being dropped.
            Err(()) => anomalies += 1,
        }
    }
    SpecFields {
        requirements_note: requirements_note(views.is_empty(), missing_criteria, anomalies),
        requirements: views,
        ..SpecFields::default()
    }
}

/// One stored requirement → its view. `Err(())` when the element is not an
/// object; the returned count is the non-string AC/constraint items seen.
fn requirement_of(item: &serde_json::Value) -> Result<(RequirementView, usize, bool), ()> {
    let obj = item.as_object().ok_or(())?;
    let criteria = text_array(obj.get("acceptance_criteria"));
    let constraints = text_array(obj.get("constraints"));
    let view = RequirementView {
        id: stored_text(obj.get("id")),
        statement: stored_text(obj.get("statement")),
        acceptance_criteria: criteria.items,
        constraints: constraints.items,
    };
    let criteria_listed = obj
        .get("acceptance_criteria")
        .and_then(|v| v.as_array())
        .is_some();
    Ok((
        view,
        criteria.anomalies + constraints.anomalies,
        criteria_listed,
    ))
}

/// The honest note of the structure, in priority order: anomalies first
/// (they hide data), then an absent/empty structure.
fn requirements_note(empty: bool, missing_criteria: bool, anomalies: usize) -> Option<String> {
    if anomalies > 0 {
        return Some(format!(
            "в структуре требований {anomalies} элемент(ов) нестрокового типа — показаны как аномалия, а не как текст"
        ));
    }
    if empty {
        return Some(EMPTY_REQUIREMENTS.to_string());
    }
    if missing_criteria {
        return Some(NO_CRITERIA.to_string());
    }
    None
}

/// A stored string array plus how many of its items were NOT strings.
#[derive(Default)]
struct Texts {
    items: Vec<String>,
    anomalies: usize,
}

/// Fail-closed strings (rework R10): a non-string item is kept as a VISIBLE
/// anomaly marker — the old `to_string()` made any JSON value look like a
/// legitimate stored line.
fn text_array(v: Option<&serde_json::Value>) -> Texts {
    let Some(items) = v.and_then(|v| v.as_array()) else {
        return Texts::default();
    };
    let mut out = Texts::default();
    for item in items {
        match item.as_str() {
            Some(s) => out.items.push(s.to_string()),
            None => {
                out.anomalies += 1;
                out.items.push(anomaly_text(item));
            }
        }
    }
    out
}

/// The visible marker for a non-string array item.
fn anomaly_text(item: &serde_json::Value) -> String {
    let kind = match item {
        serde_json::Value::Null => "null",
        serde_json::Value::Bool(_) => "bool",
        serde_json::Value::Number(_) => "число",
        serde_json::Value::Array(_) => "массив",
        serde_json::Value::Object(_) => "объект",
        serde_json::Value::String(_) => "строка",
    };
    format!("[аномальный элемент: {kind}] {item}")
}

/// A stored text field: a string verbatim, a non-string as its anomaly
/// marker, an absent field as the empty string (nothing invented).
fn stored_text(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(other) => anomaly_text(other),
        None => String::new(),
    }
}

fn non_empty(s: &str) -> Option<&str> {
    let t = s.trim();
    if t.is_empty() { None } else { Some(t) }
}
