//! The signature read-plane (REQ-SIGN-01..13): everything the operator's
//! APPROVAL card needs to sign exactly what is stored, in ONE read.
//!
//! The parse rules used to live in the WINDOW (`devai-ui/src/commands.rs`:
//! `SpecPresentation`, `ParagraphSource`, `spec_presentation` and friends),
//! which made the window the only place able to present a spec body. The
//! `task.signature` operation is a `devai-api` read, so the rules move HERE
//! (REQ-SIGN-12): ONE function parses a stored spec body into ONE
//! intermediate value ([`ParsedSpec`]), from which two projections are
//! built — the existing [`SpecPresentation`] for the human-wait row (shape
//! unchanged, REQ-SIGN-10 AC7) and [`SpecView`] for the signature card
//! (adds requirements/requirements_note).
//!
//! Nothing here rewrites stored data: the body is read, parsed, projected.
//! `artifact.diff` is not introduced (registry.rs:386-405) — the human diff
//! is a projection over the SAME parsed fields (REQ-SIGN-13).

use chrono::{DateTime, Utc};
use devai_core::types::{
    Artifact, ArtifactKind, ArtifactStatus, Task, TaskState, decision_kind, event_kind,
};
use devai_store::Store;
use serde::Serialize;

use crate::{ApiResult, inbox::pick_spec_artifact};

/// Which spec / review artifact a gate presents. CURRENT when the task has
/// one, else the LAST row marked `current: false` — the ratified OQ-GATE-3
/// rule, reused by both projections (one selector, `pick_spec_artifact`).
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
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ParagraphSource {
    /// The spec's own top-level `human_paragraph`.
    Spec,
    /// Ratified OQ-GATE-4: the spec predates the paragraph protocol (or its
    /// body is unreadable) — the stored task business description stands
    /// in, labelled as such.
    BusinessDescription,
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

/// Origin of the reviewer paragraph (REQ-SIGN-11 AC3): exactly three values,
/// `none` only for a missing/unreadable report.
#[derive(Serialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "snake_case")]
pub enum ReviewParagraphSource {
    ReviewReport,
    Summary,
    None,
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

/// The diff of the spec against its previous version, compared by PARSED
/// FIELDS (never by raw JSON lines — reformatting is not a spec change).
#[derive(Serialize, Clone, Debug)]
pub struct SpecDiff {
    pub paragraph: Vec<DiffLine>,
    pub requirements: Vec<DiffLine>,
    /// Honest reason when the comparison could not be made.
    pub note: Option<String>,
}

/// The previous version of the spec plus its field diff.
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

/// Why the approve is impossible, in the ratified order (REQ-SIGN-07 AC1):
/// the FIRST failed conjunct names the cause.
pub fn approve_denied(
    state: TaskState,
    spec: Option<&SpecView>,
    requirements_seq: u32,
) -> Option<&'static str> {
    if state != TaskState::Approval {
        return Some("not_approval");
    }
    let Some(spec) = spec else {
        return Some("no_spec");
    };
    if !spec.current {
        return Some("spec_not_current");
    }
    if requirements_seq == 0 {
        return Some("no_requirements_version");
    }
    None
}

/// The reject right is the state and nothing else (workflow.rs:452-457).
pub fn reject_denied(state: TaskState) -> Option<&'static str> {
    if state != TaskState::Approval {
        return Some("not_approval");
    }
    None
}

/// The ONE parsed value every projection is built from (REQ-SIGN-12 AC5):
/// failing to read/parse the body only changes `unavailable_note`, never the
/// shape, so the card and the wait row cannot disagree.
#[derive(Clone, Debug)]
pub struct ParsedSpec {
    pub human_paragraph: Option<String>,
    pub paragraph_source: ParagraphSource,
    pub unavailable_note: Option<String>,
    pub open_questions: Vec<String>,
    pub requirements: Vec<RequirementView>,
    pub requirements_note: Option<String>,
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

/// Parse the spec artifact body. The JSON is read, never rewritten; the
/// array shapes are the seeded contract's (`open_questions` is an array of
/// strings, seed.rs:312). Ratified OQ-GATE-4: absent paragraph → the stored
/// task business description with an honest source + note.
pub fn parse_spec_body(body: Result<&str, &str>, task: &Task) -> ParsedSpec {
    let text = match body {
        Ok(text) => text,
        Err(reason) => {
            return fallback(
                task,
                Vec::new(),
                Vec::new(),
                None,
                format!("{reason}; показываю бизнес-описание задачи"),
            );
        }
    };
    let value = match serde_json::from_str::<serde_json::Value>(text) {
        Ok(value) => value,
        Err(_) => {
            return fallback(
                task,
                Vec::new(),
                Vec::new(),
                None,
                "тело артефакта специки не разбирается как JSON; показываю бизнес-описание задачи"
                    .to_string(),
            );
        }
    };
    let open_questions = spec_open_questions(&value);
    let (requirements, requirements_note) = spec_requirements(&value);
    match spec_paragraph(&value) {
        Some(paragraph) => ParsedSpec {
            human_paragraph: Some(paragraph),
            paragraph_source: ParagraphSource::Spec,
            unavailable_note: None,
            open_questions,
            requirements,
            requirements_note,
        },
        // Ratified OQ-GATE-4, verbatim wording of the decision.
        None => fallback(
            task,
            open_questions,
            requirements,
            requirements_note,
            "аналитик абзац не писал — спека до протокола; подставляю бизнес-описание задачи"
                .to_string(),
        ),
    }
}

/// The ONE constructor of the fail-closed shape: the presentation is never
/// empty, and the three fail-closed branches differ only in `note`.
fn fallback(
    task: &Task,
    open_questions: Vec<String>,
    requirements: Vec<RequirementView>,
    requirements_note: Option<String>,
    note: String,
) -> ParsedSpec {
    ParsedSpec {
        human_paragraph: non_empty(task.business_description.as_str()).map(str::to_string),
        paragraph_source: ParagraphSource::BusinessDescription,
        unavailable_note: Some(note),
        open_questions,
        requirements,
        requirements_note,
    }
}

/// The spec-card convenience wrapper over [`parse_spec_body`] — kept for the
/// call sites that only need the wait-row projection.
pub fn spec_presentation(
    artifact: &Artifact,
    body: Result<&str, &str>,
    task: &Task,
    current: bool,
) -> SpecPresentation {
    parse_spec_body(body, task).presentation(artifact, current)
}

/// The document's own top-level `human_paragraph`, trimmed.
fn spec_paragraph(value: &serde_json::Value) -> Option<String> {
    value
        .get("human_paragraph")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
}

/// The REQUIREMENTS_SPEC `open_questions` array verbatim (strings as-is,
/// other JSON shapes stringified): questions of the document, never
/// registry questions (REQ-GATE-05 AC2).
fn spec_open_questions(value: &serde_json::Value) -> Vec<String> {
    value
        .get("open_questions")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .map(|item| match item {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The formal structure (REQ-SIGN-02): printed verbatim, in array order. A
/// body without a requirements array keeps the paragraph and says honestly
/// that the structure is absent — no requirement is invented.
fn spec_requirements(value: &serde_json::Value) -> (Vec<RequirementView>, Option<String>) {
    let Some(items) = value.get("requirements").and_then(|v| v.as_array()) else {
        return (
            Vec::new(),
            Some("в теле спеки нет массива требований — формальной структуры нет".to_string()),
        );
    };
    let mut out = Vec::new();
    let mut missing_criteria = false;
    for item in items {
        let Some(obj) = item.as_object() else {
            continue;
        };
        let id = obj
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let statement = obj
            .get("statement")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let acceptance_criteria = string_array(obj.get("acceptance_criteria"));
        if obj
            .get("acceptance_criteria")
            .and_then(|v| v.as_array())
            .is_none()
        {
            missing_criteria = true;
        }
        let constraints = string_array(obj.get("constraints"));
        out.push(RequirementView {
            id,
            statement,
            acceptance_criteria,
            constraints,
        });
    }
    let note = if out.is_empty() {
        Some("массив требований пуст — формальной структуры нет".to_string())
    } else if missing_criteria {
        Some("в требованиях нет acceptance criteria — структура неполная".to_string())
    } else {
        None
    };
    (out, note)
}

fn string_array(v: Option<&serde_json::Value>) -> Vec<String> {
    v.and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .map(|item| match item {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}

fn non_empty(s: &str) -> Option<&str> {
    let t = s.trim();
    if t.is_empty() { None } else { Some(t) }
}

/// Parse the reviewer report body (REQ-SIGN-11). The verdict is the STORED
/// token — `approved`/`needs_revision`/`rejected` in the report's own
/// spelling — never derived from the task state.
pub fn parse_review_body(body: Result<&str, &str>) -> (Option<String>, ReviewParagraphSource, Option<String>, Option<String>) {
    let text = match body {
        Ok(text) => text,
        Err(reason) => {
            return (
                None,
                ReviewParagraphSource::None,
                None,
                Some(format!("тело отчёта не читается: {reason}")),
            );
        }
    };
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(_) => {
            return (
                None,
                ReviewParagraphSource::None,
                None,
                Some("тело отчёта ревьювера не разбирается как JSON".to_string()),
            );
        }
    };
    let verdict = value
        .get("verdict")
        .and_then(|v| v.as_str())
        .map(str::to_string);
    let note_verdict = if verdict.is_none() {
        Some("в отчёте ревьювера нет вердикта".to_string())
    } else {
        None
    };
    let paragraph = value
        .get("human_paragraph")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    if let Some(paragraph) = paragraph {
        return (
            verdict,
            ReviewParagraphSource::ReviewReport,
            Some(paragraph),
            note_verdict,
        );
    }
    let summary = value
        .get("summary")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string);
    match summary {
        Some(summary) => (
            verdict,
            ReviewParagraphSource::Summary,
            Some(summary),
            Some("абзаца ревьювера в отчёте нет".to_string()),
        ),
        None => (verdict, ReviewParagraphSource::None, None, note_verdict),
    }
}

/// The previous spec of the same task: the highest seq BELOW the presented
/// one (REQ-SIGN-07 AC7). No previous → `None` and the caller says so.
pub fn pick_previous_spec<'a>(arts: &'a [Artifact], seq: u32) -> Option<&'a Artifact> {
    arts.iter()
        .filter(|a| a.kind == ArtifactKind::RequirementsSpec && a.seq < seq)
        .max_by_key(|a| (a.seq, a.created_at))
}

/// The field diff of two parsed specs (REQ-SIGN-13). Comparison is made on
/// parsed values, so JSON reformatting never reads as a change.
pub fn diff_specs(previous: &ParsedSpec, current: &ParsedSpec) -> SpecDiff {
    let paragraph = diff_paragraph(
        previous.human_paragraph.as_deref().unwrap_or(""),
        current.human_paragraph.as_deref().unwrap_or(""),
    );
    let mut requirements = Vec::new();
    for req in &current.requirements {
        match previous.requirements.iter().find(|p| p.id == req.id) {
            None => {
                requirements.push(line("+", &label_of(&req.id), &req.statement));
                for ac in &req.acceptance_criteria {
                    requirements.push(line("+", &format!("{} · AC", label_of(&req.id)), ac));
                }
                for c in &req.constraints {
                    requirements.push(line("+", &format!("{} · ограничение", label_of(&req.id)), c));
                }
            }
            Some(old) if old == req => {
                requirements.push(line(" ", &label_of(&req.id), &req.statement));
            }
            Some(old) => {
                if old.statement != req.statement {
                    requirements.push(line("-", &label_of(&req.id), &old.statement));
                    requirements.push(line("+", &label_of(&req.id), &req.statement));
                }
                for ac in &old.acceptance_criteria {
                    if !req.acceptance_criteria.contains(ac) {
                        requirements.push(line("-", &format!("{} · AC", label_of(&req.id)), ac));
                    }
                }
                for ac in &req.acceptance_criteria {
                    if !old.acceptance_criteria.contains(ac) {
                        requirements.push(line("+", &format!("{} · AC", label_of(&req.id)), ac));
                    }
                }
                for c in &old.constraints {
                    if !req.constraints.contains(c) {
                        requirements.push(line(
                            "-",
                            &format!("{} · ограничение", label_of(&req.id)),
                            c,
                        ));
                    }
                }
                for c in &req.constraints {
                    if !old.constraints.contains(c) {
                        requirements.push(line(
                            "+",
                            &format!("{} · ограничение", label_of(&req.id)),
                            c,
                        ));
                    }
                }
            }
        }
    }
    for old in &previous.requirements {
        if !current.requirements.iter().any(|r| r.id == old.id) {
            requirements.push(line("-", &label_of(&old.id), &old.statement));
        }
    }
    SpecDiff {
        paragraph,
        requirements,
        note: None,
    }
}

fn label_of(id: &str) -> String {
    if id.is_empty() {
        "требование".to_string()
    } else {
        id.to_string()
    }
}

fn line(mark: &str, label: &str, text: &str) -> DiffLine {
    DiffLine {
        mark: mark.to_string(),
        label: label.to_string(),
        text: text.to_string(),
    }
}

/// Line diff with unchanged context (small LCS over lines — the paragraph
/// is prose, line granularity is the human unit).
fn diff_paragraph(old: &str, new: &str) -> Vec<DiffLine> {
    let a: Vec<&str> = if old.is_empty() {
        Vec::new()
    } else {
        old.lines().collect()
    };
    let b: Vec<&str> = if new.is_empty() {
        Vec::new()
    } else {
        new.lines().collect()
    };
    let mut lcs = vec![vec![0usize; b.len() + 1]; a.len() + 1];
    for i in (0..a.len()).rev() {
        for j in (0..b.len()).rev() {
            lcs[i][j] = if a[i] == b[j] {
                lcs[i + 1][j + 1] + 1
            } else {
                lcs[i + 1][j].max(lcs[i][j + 1])
            };
        }
    }
    let mut out = Vec::new();
    let (mut i, mut j) = (0usize, 0usize);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            out.push(line(" ", "абзац", a[i]));
            i += 1;
            j += 1;
        } else if lcs[i + 1][j] >= lcs[i][j + 1] {
            out.push(line("-", "абзац", a[i]));
            i += 1;
        } else {
            out.push(line("+", "абзац", b[j]));
            j += 1;
        }
    }
    while i < a.len() {
        out.push(line("-", "абзац", a[i]));
        i += 1;
    }
    while j < b.len() {
        out.push(line("+", "абзац", b[j]));
        j += 1;
    }
    out
}

/// The body of one artifact as text or as the honest reason it is not text.
fn body_of(store: &Store, artifact: &Artifact) -> Result<String, String> {
    match store.read_artifact_body(artifact) {
        Ok(bytes) => match String::from_utf8(bytes) {
            Ok(text) => Ok(text),
            Err(_) => Err(format!(
                "артефакт бинарный (не UTF-8, {} байт)",
                artifact.size
            )),
        },
        Err(e) => Err(e.to_string()),
    }
}

/// Build the whole signature answer from stored facts (REQ-SIGN-07). One
/// spec selector (`pick_spec_artifact`), one parse per body, no writes.
pub fn signature_view(store: &Store, task: Task) -> ApiResult<SignatureView> {
    let arts = store.artifacts(task.id)?;

    let spec = match pick_spec_artifact(&arts) {
        Some((artifact, current)) => {
            let body = body_of(store, artifact);
            let parsed = parse_spec_body(body.as_deref().map_err(|e| e.as_str()), &task);
            Some((artifact.clone(), current, parsed))
        }
        None => None,
    };

    let spec_view = spec
        .as_ref()
        .map(|(artifact, current, parsed)| parsed.view(artifact, *current));

    let (review, review_note) = match last_review_artifact(&arts) {
        Some((artifact, current)) => {
            let body = body_of(store, artifact);
            let (verdict, paragraph_source, human_paragraph, note) =
                parse_review_body(body.as_deref().map_err(|e| e.as_str()));
            (
                Some(ReviewView {
                    artifact: artifact.clone(),
                    current,
                    verdict,
                    human_paragraph,
                    paragraph_source,
                    note,
                }),
                None,
            )
        }
        None => (None, Some("отчёта ревьювера нет".to_string())),
    };

    let (previous, previous_note) = match &spec {
        Some((artifact, _, _)) => match pick_previous_spec(&arts, artifact.seq) {
            Some(prev) => {
                let prev_parsed = parse_spec_body(
                    body_of(store, prev).as_deref().map_err(|e| e.as_str()),
                    &task,
                );
                let current_parsed = spec.as_ref().map(|(_, _, p)| p.clone()).expect("spec");
                (
                    Some(PreviousView {
                        artifact: prev.clone(),
                        diff: diff_specs(&prev_parsed, &current_parsed),
                    }),
                    None,
                )
            }
            None => (None, Some("прошлой версии нет".to_string())),
        },
        None => (None, Some("прошлой версии нет".to_string())),
    };

    let signature = newest_signature_fact(store, &task, &arts)?;

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

/// The newest signature fact (REQ-SIGN-06): among decision rows of the two
/// kinds and the two event kinds, by `created_at`; on a tie the decision row
/// wins (it carries the refs). `spec_seq` only from a resolvable ref.
fn newest_signature_fact(
    store: &Store,
    task: &Task,
    arts: &[Artifact],
) -> ApiResult<Option<SignatureFact>> {
    let decisions = store.list_decisions(task.id)?;
    let events = store.events(task.id, None)?;
    let mut best: Option<SignatureFact> = None;
    for d in decisions {
        let kind = match d.kind.as_str() {
            decision_kind::REQUIREMENTS_APPROVAL => "approval",
            decision_kind::REQUIREMENTS_REJECTION => "rejection",
            _ => continue,
        };
        let spec_seq = d.refs.iter().find_map(|r| {
            arts.iter()
                .find(|a| a.kind == ArtifactKind::RequirementsSpec && a.id.to_string() == *r)
                .map(|a| a.seq)
        });
        let fact = SignatureFact {
            kind: kind.to_string(),
            actor: d.actor,
            at: d.created_at,
            rationale: d.rationale,
            spec_seq,
            source: "decision".to_string(),
        };
        if best.as_ref().is_none_or(|b| fact.at >= b.at) {
            best = Some(fact);
        }
    }
    for e in events {
        let kind = match e.kind.as_str() {
            event_kind::REQUIREMENTS_APPROVED => "approval",
            event_kind::REQUIREMENTS_REJECTED => "rejection",
            _ => continue,
        };
        let fact = SignatureFact {
            kind: kind.to_string(),
            actor: e.actor,
            at: e.created_at,
            rationale: e
                .payload
                .get("rationale")
                .and_then(|v| v.as_str())
                .map(str::to_string),
            spec_seq: None,
            source: "event".to_string(),
        };
        if best.as_ref().is_none_or(|b| fact.at > b.at) {
            best = Some(fact);
        }
    }
    Ok(best)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use devai_core::types::{DataCategory, Task};

    fn task() -> Task {
        Task {
            id: devai_core::types::new_id(),
            project_id: None,
            title: "дело".into(),
            business_description: "бизнес-описание задачи".into(),
            human_name: None,
            state: TaskState::Approval,
            return_state: None,
            requirements_seq: 2,
            lock_version: 1,
            created_at: Utc::now(),
            updated_at: Utc::now(),
        }
    }

    fn artifact(seq: u32, status: ArtifactStatus) -> Artifact {
        Artifact {
            id: devai_core::types::new_id(),
            task_id: super::tests::task().id,
            kind: ArtifactKind::RequirementsSpec,
            name: format!("spec-v{seq}.json"),
            seq,
            category: DataCategory::Artifact,
            status,
            sha256: "0".repeat(64),
            size: 2,
            media_type: "application/json".into(),
            contract: None,
            created_by: "analyst".into(),
            execution_id: None,
            depends_on: vec![],
            created_at: Utc::now(),
        }
    }

    /// REQ-SIGN-02 AC1/AC2: the paragraph is printed verbatim (with its line
    /// breaks), the requirements keep array order and their criteria; a body
    /// without structure keeps the paragraph and says so.
    #[test]
    fn spec_parsing_prints_stored_text_and_structure_verbatim() {
        let task = task();
        let body = r#"{
          "human_paragraph": "Строка один.\nСтрока два.",
          "requirements": [
            {"id": "REQ-1", "statement": "Сделать X", "acceptance_criteria": ["AC1", "AC2"], "constraints": ["C1"]},
            {"id": "REQ-2", "statement": "Сделать Y", "acceptance_criteria": []}
          ],
          "open_questions": ["OQ-1: вопрос?"]
        }"#;
        let parsed = parse_spec_body(Ok(body), &task);
        assert_eq!(parsed.paragraph_source, ParagraphSource::Spec);
        assert_eq!(
            parsed.human_paragraph.as_deref(),
            Some("Строка один.\nСтрока два.")
        );
        assert_eq!(parsed.requirements.len(), 2);
        assert_eq!(parsed.requirements[0].id, "REQ-1");
        assert_eq!(parsed.requirements[0].acceptance_criteria, vec!["AC1", "AC2"]);
        assert_eq!(parsed.requirements[1].id, "REQ-2");
        assert!(parsed.requirements_note.is_none());
        assert_eq!(parsed.open_questions, vec!["OQ-1: вопрос?"]);

        // No structure: paragraph survives, the note is honest, nothing invented.
        let no_structure = r#"{"human_paragraph": "Абзац."}"#;
        let parsed = parse_spec_body(Ok(no_structure), &task);
        assert!(parsed.requirements.is_empty());
        assert!(parsed.requirements_note.is_some());
        assert_eq!(parsed.human_paragraph.as_deref(), Some("Абзац."));
    }

    /// Ratified OQ-GATE-4 carried over: unreadable body / pre-protocol spec
    /// fall back to the business description, labelled — never empty.
    #[test]
    fn unreadable_bodies_fall_back_to_the_labelled_business_description() {
        let task = task();
        for parsed in [
            parse_spec_body(Err("артефакт бинарный (не UTF-8, 12 байт)"), &task),
            parse_spec_body(Ok("не json {{{"), &task),
            parse_spec_body(Ok(r#"{"version":2}"#), &task),
        ] {
            assert_eq!(
                parsed.paragraph_source,
                ParagraphSource::BusinessDescription
            );
            assert!(parsed.unavailable_note.is_some());
            assert_eq!(
                parsed.human_paragraph.as_deref(),
                Some("бизнес-описание задачи")
            );
        }
    }

    /// REQ-SIGN-07 AC1: the denial names the FIRST failed conjunct, in order.
    #[test]
    fn approve_denial_names_the_first_failed_conjunct() {
        assert_eq!(
            approve_denied(TaskState::Review, None, 0),
            Some("not_approval")
        );
        assert_eq!(approve_denied(TaskState::Approval, None, 0), Some("no_spec"));
        let mut view = parse_spec_body(Ok(r#"{"human_paragraph":"x"}"#), &task())
            .view(&artifact(1, ArtifactStatus::Stale), false);
        assert_eq!(
            approve_denied(TaskState::Approval, Some(&view), 1),
            Some("spec_not_current")
        );
        view.current = true;
        assert_eq!(
            approve_denied(TaskState::Approval, Some(&view), 0),
            Some("no_requirements_version")
        );
        assert_eq!(approve_denied(TaskState::Approval, Some(&view), 1), None);
        assert_eq!(reject_denied(TaskState::Approval), None);
        assert_eq!(reject_denied(TaskState::Analysis), Some("not_approval"));
    }

    /// REQ-SIGN-11: the five review branches, each with a visible text.
    #[test]
    fn review_branches_are_all_visible() {
        // (2) @1: no human_paragraph → summary as the paragraph, labelled.
        let (verdict, source, paragraph, note) =
            parse_review_body(Ok(r#"{"verdict":"approved","summary":"Коротко"}"#));
        assert_eq!(verdict.as_deref(), Some("approved"));
        assert_eq!(source, ReviewParagraphSource::Summary);
        assert_eq!(paragraph.as_deref(), Some("Коротко"));
        assert!(note.is_some());

        // (3) @2: human_paragraph as stored.
        let (_, source, paragraph, _) =
            parse_review_body(Ok(r#"{"verdict":"needs_revision","human_paragraph":"Абзац"}"#));
        assert_eq!(source, ReviewParagraphSource::ReviewReport);
        assert_eq!(paragraph.as_deref(), Some("Абзац"));

        // Empty paragraph = absent → summary branch.
        let (_, source, _, _) =
            parse_review_body(Ok(r#"{"verdict":"approved","human_paragraph":"  ","summary":"S"}"#));
        assert_eq!(source, ReviewParagraphSource::Summary);

        // (5) unreadable body: no verdict, a reason, no panic.
        for (verdict, source, _, note) in [
            parse_review_body(Err("boom")),
            parse_review_body(Ok("не json")),
        ] {
            assert_eq!(source, ReviewParagraphSource::None);
            assert!(note.is_some(), "every unreadable branch states its reason");
            assert!(verdict.is_none());
        }
        // A readable body WITHOUT a verdict: the paragraph branch still
        // shows the stored summary, the missing verdict is named in a note.
        let (verdict, source, paragraph, note) = parse_review_body(Ok(r#"{"summary":"S"}"#));
        assert!(verdict.is_none());
        assert_eq!(source, ReviewParagraphSource::Summary);
        assert_eq!(paragraph.as_deref(), Some("S"));
        assert!(note.is_some());
    }

    /// REQ-SIGN-13: the diff compares PARSED fields — reformatting is not a
    /// change; a changed statement shows as -/+; an added requirement as +.
    #[test]
    fn spec_diff_is_human_and_by_parsed_fields() {
        let task = task();
        let old = parse_spec_body(
            Ok(r#"{"requirements":[{"id":"REQ-1","statement":"Старое","acceptance_criteria":["A"]}]}"#),
            &task,
        );
        let reformatted = parse_spec_body(
            Ok("{\n  \"requirements\": [\n    {\"id\": \"REQ-1\", \"statement\": \"Старое\", \"acceptance_criteria\": [\"A\"]}\n  ]\n}\n"),
            &task,
        );
        let same = diff_specs(&old, &reformatted);
        assert!(
            same.requirements.iter().all(|l| l.mark == " "),
            "reformatting must not read as a spec change"
        );

        let new = parse_spec_body(
            Ok(r#"{"requirements":[
              {"id":"REQ-1","statement":"Новое","acceptance_criteria":["A"]},
              {"id":"REQ-2","statement":"Добавлено","acceptance_criteria":["B"]}
            ]}"#),
            &task,
        );
        let diff = diff_specs(&old, &new);
        assert!(diff.requirements.iter().any(|l| l.mark == "-" && l.text == "Старое"));
        assert!(diff.requirements.iter().any(|l| l.mark == "+" && l.text == "Новое"));
        assert!(diff
            .requirements
            .iter()
            .any(|l| l.mark == "+" && l.label == "REQ-2"));
    }
}
