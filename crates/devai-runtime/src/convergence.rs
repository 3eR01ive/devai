//! The convergence slice (task 01a1b5be, REQ-CV-01..16): a COMPUTED read
//! view over data the store already holds — published QUALITY_REPORT /
//! REVIEW_REPORT carriers, the role executions they name and the blobs
//! behind them. Nothing here writes: no rows, no tables, no events, no
//! cache (REQ-CV-01), and no score is re-derived from weights — the slice
//! quotes the stored `evaluation.score` / `evaluation.theta` of the body.
//!
//! One round of a stage is one published carrier artifact. `artifacts.seq`
//! is NOT a round number (it is a per-kind version counter), so the round
//! is the 1-based index in the full `(seq, created_at, id)` order computed
//! HERE, never stored (REQ-CV-02).
//!
//! Bodies are parsed as RAW JSON, not through `EvaluationBlock`: both
//! `evaluation.score` and `evaluation.theta` default in the typed shape, so
//! an absent key there would read as `0.0` (REQ-CV-03). The body reader is
//! memoised per artifact id for one call (REQ-CV-12 AC7/AC8): a spec bound
//! to N review rounds is read once, and a snapshot that asks for five tasks
//! reads each artifact once, not once per row.
//!
//! QA R2 (rel/error-context): a STORE read failure is NOT a corrupt body.
//! The seam keeps the typed `StoreError` code and message on the point
//! (`read_error`) and marks its applicable fields with the `read_error`
//! token; bytes that were read but are not a JSON object stay
//! `Body::Unreadable` and keep the report/body tokens. Swallowing both as
//! `Err(_)` would erase the only evidence of a real storage failure.

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use devai_core::types::{Artifact, ArtifactKind, Id, RoleExecution};
use devai_store::StoreError;
use serde::Serialize;

/// The step that counts as progress (REQ-CV-07): 0.01, compared in whole
/// thousandths, a platform constant — NOT a config knob.
pub const GAP_MILLI: i64 = 10;

pub const STAGE_SPEC_REVIEW: &str = "spec_review";
pub const STAGE_QUALITY: &str = "quality";

/// The closed mode vocabulary (REQ-CV-09): five tokens, one per point.
pub mod mode {
    pub const CONVERGING: &str = "CONVERGING";
    pub const PLATEAU: &str = "PLATEAU";
    pub const DIVERGING: &str = "DIVERGING";
    pub const STUCK: &str = "STUCK";
    pub const UNKNOWN: &str = "UNKNOWN";
}

/// `mode_basis.rule` tokens (REQ-CV-09): the predicate that fired, lowercase.
pub mod rule {
    pub const CONVERGING: &str = "converging";
    pub const PLATEAU: &str = "plateau";
    pub const DIVERGING: &str = "diverging";
    pub const STUCK: &str = "stuck";
    pub const UNKNOWN: &str = "unknown";
}

/// `mode_basis.reason` tokens — only ever present on UNKNOWN (REQ-CV-09).
pub mod reason {
    pub const INSUFFICIENT_DATA: &str = "insufficient_data";
    pub const NO_PREDICATE: &str = "no_predicate";
}

/// The computed volume direction of a spec round (REQ-CV-06).
pub mod volume {
    pub const GROWING: &str = "growing";
    pub const SHRINKING: &str = "shrinking";
    pub const FLAT: &str = "flat";
}

/// The closed `missing[]` vocabulary (REQ-CV-05 AC3). QA R2 ratified ONE
/// addition, `read_error`: a store read failure must not be named with the
/// corrupt-body token, so the vocabulary is 13 + `read_error`.
pub mod token {
    pub const QUALITY_REPORT: &str = "quality_report";
    pub const QUALITY_THETA: &str = "quality_theta";
    pub const REVIEW_VERDICT: &str = "review_verdict";
    pub const SPEC_BINDING: &str = "spec_binding";
    pub const SPEC_BODY: &str = "spec_body";
    pub const SPEC_COUNTS: &str = "spec_counts";
    pub const COUNTS_UNAVAILABLE: &str = "counts_unavailable";
    pub const TIME: &str = "time";
    pub const STAGE_TIME: &str = "stage_time";
    pub const PROFILE: &str = "profile";
    pub const CURRENT_UNMEASURED: &str = "current_unmeasured";
    pub const PREDECESSOR_UNMEASURED: &str = "predecessor_unmeasured";
    pub const SEGMENT_BOUNDARY: &str = "segment_boundary";
    /// QA R2: the store could not PRODUCE the body — distinct from a body
    /// that was produced but is corrupt (`quality_report` / `spec_body`).
    pub const READ_ERROR: &str = "read_error";
}

/// Every token of the closed vocabulary, in one place for the tests that
/// prove nothing else can be serialised.
pub const MISSING_TOKENS: &[&str] = &[
    token::QUALITY_REPORT,
    token::QUALITY_THETA,
    token::REVIEW_VERDICT,
    token::SPEC_BINDING,
    token::SPEC_BODY,
    token::SPEC_COUNTS,
    token::COUNTS_UNAVAILABLE,
    token::TIME,
    token::STAGE_TIME,
    token::PROFILE,
    token::CURRENT_UNMEASURED,
    token::PREDECESSOR_UNMEASURED,
    token::SEGMENT_BOUNDARY,
    token::READ_ERROR,
];

/// Quality fields that can be null with a token, in the printed order.
const QUALITY_FIELDS: &[&str] = &[
    "score_milli",
    "theta_milli",
    "low_count",
    "unscored_count",
    "invalid_count",
    "pending_count",
    "closed_count",
    "delta_score_milli",
    "profile",
];

/// Spec fields that can be null with a token, in the printed order.
const SPEC_FIELDS: &[&str] = &[
    "verdict",
    "requirements_count",
    "oq_count",
    "delta_requirements",
    "delta_oq",
    "volume_direction",
];

const COMMON_FIELDS: &[&str] = &["time_seconds"];

/// One `missing[]` record: the field that is null and the reason why.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct MissingEntry {
    pub field: &'static str,
    pub token: &'static str,
}

/// A store read failure kept on the point (QA R2): the typed machine code
/// and the store's own message, so a real storage fault never looks like a
/// corrupt report body.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ReadError {
    pub code: &'static str,
    pub message: String,
}

/// The basis of a point's mode (REQ-CV-09): the fired rule, an honest
/// reason only for UNKNOWN, the rounds the rule read, and the check id
/// only for STUCK.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct ModeBasis {
    pub rule: &'static str,
    pub reason: Option<&'static str>,
    pub rounds: Vec<u32>,
    pub check_id: Option<String>,
}

impl ModeBasis {
    fn unknown(reason: &'static str) -> ModeBasis {
        ModeBasis {
            rule: rule::UNKNOWN,
            reason: Some(reason),
            rounds: Vec::new(),
            check_id: None,
        }
    }

    fn fired(rule: &'static str, rounds: Vec<u32>) -> ModeBasis {
        ModeBasis {
            rule,
            reason: None,
            rounds,
            check_id: None,
        }
    }
}

/// One round of a stage — ONE shape for both stages, every key always
/// present (REQ-CV-05). Fields that do not apply to the stage are null and
/// carry no `missing[]` record. QA R2 adds `read_error`: null unless the
/// store itself failed to produce this round's body.
#[derive(Serialize, Clone, Debug)]
pub struct ConvergencePoint {
    pub round: u32,
    pub artifact_id: String,
    pub created_at: DateTime<Utc>,
    pub stage: &'static str,
    pub status: String,
    pub measured: bool,
    pub comparable: bool,
    pub missing: Vec<MissingEntry>,
    pub score_milli: Option<i64>,
    pub theta_milli: Option<i64>,
    pub low_count: Option<usize>,
    pub unscored_count: Option<usize>,
    pub invalid_count: Option<usize>,
    pub pending_count: Option<usize>,
    pub closed_count: Option<usize>,
    pub delta_score_milli: Option<i64>,
    pub verdict: Option<String>,
    pub requirements_count: Option<usize>,
    pub oq_count: Option<usize>,
    pub delta_requirements: Option<i64>,
    pub delta_oq: Option<i64>,
    pub volume_direction: Option<&'static str>,
    pub time_seconds: Option<i64>,
    pub profile: Option<String>,
    pub mode: &'static str,
    pub mode_basis: ModeBasis,
    pub read_error: Option<ReadError>,
}

/// Stage counters (REQ-CV-12): all four are counts over the series, never a
/// derived volume.
#[derive(Serialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ConvergenceCounters {
    pub rounds_total: usize,
    pub rounds_unmeasured: usize,
    pub rounds_without_time: usize,
    pub spec_versions_without_review: usize,
}

/// One stage of the slice: its rounds, counters, accumulated time and the
/// stage-level gaps. `stage_current_seconds` is the profile segment the
/// stage is currently in (REQ-CV-11): the LAST segment for quality, the
/// whole series for spec_review (no profile exists there).
#[derive(Serialize, Clone, Debug)]
pub struct StageConvergence {
    pub stage: &'static str,
    pub series: Vec<ConvergencePoint>,
    pub counters: ConvergenceCounters,
    pub stage_total_seconds: Option<i64>,
    pub stage_current_seconds: Option<i64>,
    pub missing: Vec<MissingEntry>,
}

/// The whole response of the read: `gap_milli` and `stages[]`. No own
/// snapshot time — that lives on the reading envelopes (REQ-CV-12).
#[derive(Serialize, Clone, Debug)]
pub struct ConvergenceView {
    pub gap_milli: i64,
    pub stages: Vec<StageConvergence>,
}

impl ConvergenceView {
    pub fn stage(&self, name: &str) -> Option<&StageConvergence> {
        self.stages.iter().find(|s| s.stage == name)
    }
}

/// One body outcome: absent blob, body bytes that are not a JSON object,
/// a store read FAILURE (keeps its code/message), or a usable JSON object.
/// All four are distinct; none becomes `{}`.
#[derive(Clone, Debug)]
pub enum Body {
    Absent,
    Unreadable,
    Failed(ReadError),
    Json(serde_json::Map<String, serde_json::Value>),
}

/// Parse bytes that WERE produced: only a JSON object is usable; anything
/// else is a corrupt body, never a read failure (QA R2 keeps them apart).
fn classify_bytes(bytes: Vec<u8>) -> Body {
    match std::str::from_utf8(&bytes)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
    {
        Some(serde_json::Value::Object(map)) => Body::Json(map),
        _ => Body::Unreadable,
    }
}

/// The body reader seam with one-read-per-artifact-id memoisation. The API
/// hands the store's ONE read policy (`Store::read_artifact_body`); tests
/// hand fixed bodies.
pub struct BodyCache<'a> {
    source: Box<dyn Fn(&Artifact) -> Body + 'a>,
    cache: RefCell<HashMap<Id, Body>>,
    reads: Cell<usize>,
}

impl<'a> BodyCache<'a> {
    pub fn from_reader(read: &'a dyn Fn(&Artifact) -> Result<Vec<u8>, StoreError>) -> Self {
        BodyCache {
            source: Box::new(move |a: &Artifact| match read(a) {
                Ok(bytes) => classify_bytes(bytes),
                Err(StoreError::NotFound(..)) => Body::Absent,
                // QA R2: keep the typed category AND the store's message.
                Err(e) => Body::Failed(ReadError {
                    code: e.code(),
                    message: e.to_string(),
                }),
            }),
            cache: RefCell::new(HashMap::new()),
            reads: Cell::new(0),
        }
    }

    /// Fixed bodies for the tests: a missing id is an ABSENT blob, never an
    /// empty object.
    pub fn from_bodies(bodies: HashMap<Id, Body>) -> Self {
        let fixed = RefCell::new(bodies);
        BodyCache {
            source: Box::new(move |a: &Artifact| {
                fixed.borrow().get(&a.id).cloned().unwrap_or(Body::Absent)
            }),
            cache: RefCell::new(HashMap::new()),
            reads: Cell::new(0),
        }
    }

    /// The body of this artifact, read at most once per id for the whole
    /// life of the cache.
    pub fn body(&self, a: &Artifact) -> Body {
        if let Some(hit) = self.cache.borrow().get(&a.id) {
            return hit.clone();
        }
        self.reads.set(self.reads.get() + 1);
        let body = (self.source)(a);
        self.cache.borrow_mut().insert(a.id, body.clone());
        body
    }

    /// Distinct artifacts actually read (REQ-CV-12 AC7: the cost witness).
    pub fn reads(&self) -> usize {
        self.reads.get()
    }
}

/// A point mid-flight: measurement first, modes last (they need neighbours).
#[derive(Clone, Debug)]
struct Draft {
    round: u32,
    artifact: Artifact,
    measured: bool,
    profile: Option<String>,
    score_milli: Option<i64>,
    theta_milli: Option<i64>,
    /// Ids with a score below 1.0 — `low_count`.
    low: HashSet<String>,
    unscored: usize,
    /// Every check id present in the body, whatever its score (STUCK reads
    /// identity only, never the score — REQ-CV-07).
    checks_seen: HashSet<String>,
    /// Whether `evaluation.checks` was a readable array: an empty array is
    /// readable (no shared id), an absent array is not (STUCK breaks).
    checks_readable: bool,
    /// Id → numeric score of THIS round (`None` when the check has no
    /// number); used to tell a clean item (exactly 1.0) from an unclean one.
    scores: HashMap<String, Option<f64>>,
    /// Every check id that is present and NOT exactly 1.0 (below one, no
    /// score, a non-numeric score, a waiting signature) — REQ-CV-06 closure.
    unclean: HashSet<String>,
    /// A score key that is present but is not a number — REQ-CV-03.
    invalid: usize,
    /// A check waiting for the human signature — REQ-CV-03.
    pending: usize,
    verdict: Option<String>,
    requirements: Option<usize>,
    oq: Option<usize>,
    time_seconds: Option<i64>,
    /// field → token for every applicable null field.
    tokens: HashMap<&'static str, &'static str>,
    comparable: bool,
    closed: Option<usize>,
    delta_score: Option<i64>,
    delta_req: Option<i64>,
    delta_oq: Option<i64>,
    volume: Option<&'static str>,
    mode: &'static str,
    basis: ModeBasis,
    /// QA R2: the store fault behind an unmeasured round, if any.
    read_error: Option<ReadError>,
}

impl Draft {
    fn new(round: u32, artifact: Artifact) -> Draft {
        Draft {
            round,
            artifact,
            measured: false,
            profile: None,
            score_milli: None,
            theta_milli: None,
            low: HashSet::new(),
            unscored: 0,
            checks_seen: HashSet::new(),
            checks_readable: false,
            scores: HashMap::new(),
            unclean: HashSet::new(),
            invalid: 0,
            pending: 0,
            verdict: None,
            requirements: None,
            oq: None,
            time_seconds: None,
            tokens: HashMap::new(),
            comparable: false,
            closed: None,
            delta_score: None,
            delta_req: None,
            delta_oq: None,
            volume: None,
            mode: mode::UNKNOWN,
            basis: ModeBasis::unknown(reason::INSUFFICIENT_DATA),
            read_error: None,
        }
    }

    fn token(&mut self, field: &'static str, token: &'static str) {
        self.tokens.insert(field, token);
    }
}

/// `round(x * 1000)`; scores are non-negative, so `f64::round` is the
/// half-away-from-zero comparison agreed for thousandths. Applied ONCE —
/// the stored score is already rounded to three decimals.
fn milli(x: f64) -> i64 {
    (x * 1000.0).round() as i64
}

fn exec_time(a: &Artifact, execs: &HashMap<String, RoleExecution>) -> Option<i64> {
    let exec = execs.get(&a.execution_id?.to_string())?;
    let finished = exec.finished_at?;
    let secs = (finished - exec.started_at).num_seconds();
    (secs >= 0).then_some(secs)
}

fn sort_rounds(rows: &mut [Artifact]) {
    // REQ-CV-02: seq, then created_at, then id — the slice sorts itself;
    // the store's ORDER BY created_at is not the round order.
    rows.sort_by(|a, b| {
        a.seq
            .cmp(&b.seq)
            .then(a.created_at.cmp(&b.created_at))
            .then(a.id.cmp(&b.id))
    });
}

fn carriers(artifacts: &[Artifact], kind: ArtifactKind) -> Vec<Artifact> {
    let mut rows: Vec<Artifact> = artifacts
        .iter()
        .filter(|a| a.kind == kind)
        .cloned()
        .collect();
    sort_rounds(&mut rows);
    rows
}

fn profile_of(map: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    // REQ-CV-10: `project@version` from the top-level `profile` object of
    // the body; anything unreadable is null, never a guess.
    let p = map.get("profile")?.as_object()?;
    let project = p.get("project")?.as_str()?;
    let version = p.get("version")?.as_u64()?;
    Some(format!("{project}@{version}"))
}

/// The token an unreadable quality body carries: a store failure is named
/// `read_error`, a produced-but-corrupt body keeps `quality_report`.
fn quality_gap_token(body: &Body) -> &'static str {
    match body {
        Body::Failed(_) => token::READ_ERROR,
        _ => token::QUALITY_REPORT,
    }
}

/// The token an unreadable spec body carries (REQ-CV-04).
fn spec_gap_token(body: &Body) -> &'static str {
    match body {
        Body::Failed(_) => token::READ_ERROR,
        _ => token::SPEC_BODY,
    }
}

fn set_time(d: &mut Draft, execs: &HashMap<String, RoleExecution>) {
    d.time_seconds = exec_time(&d.artifact, execs);
    if d.time_seconds.is_none() {
        d.token("time_seconds", token::TIME);
    }
}

fn quality_counters_missing(d: &mut Draft, tok: &'static str) {
    for field in [
        "score_milli",
        "low_count",
        "unscored_count",
        "invalid_count",
        "pending_count",
    ] {
        d.token(field, tok);
    }
}

fn mark_quality_unmeasured(d: &mut Draft) {
    d.measured = false;
    d.score_milli = None;
    // The CHECK LIST is deliberately NOT cleared (REQ-CV-03 AC7 /
    // REQ-CV-07): a round without a score still offers its ids to STUCK.
    // The metric counters are not readable without the score, so their
    // output stays null, but identity survives.
    d.token("closed_count", token::CURRENT_UNMEASURED);
    d.token("delta_score_milli", token::CURRENT_UNMEASURED);
}

fn quality_draft(
    round: u32,
    a: Artifact,
    bodies: &BodyCache<'_>,
    execs: &HashMap<String, RoleExecution>,
) -> Draft {
    let mut d = Draft::new(round, a);
    set_time(&mut d, execs);
    match bodies.body(&d.artifact) {
        Body::Json(map) => fill_quality_body(&mut d, &map),
        gap => fill_quality_gap(&mut d, gap),
    }
    d
}

/// A quality body that could not be used at all: the metric counters share
/// ONE reason record each, the threshold and profile have their own
/// (REQ-CV-03 AC5). A store failure additionally keeps its code/message.
fn fill_quality_gap(d: &mut Draft, body: Body) {
    let tok = quality_gap_token(&body);
    if let Body::Failed(err) = body {
        d.read_error = Some(err);
    }
    quality_counters_missing(d, tok);
    d.token("theta_milli", token::QUALITY_THETA);
    d.token("profile", token::PROFILE);
    mark_quality_unmeasured(d);
}

fn fill_quality_body(d: &mut Draft, map: &serde_json::Map<String, serde_json::Value>) {
    d.profile = profile_of(map);
    if d.profile.is_none() {
        d.token("profile", token::PROFILE);
    }
    let evaluation = map.get("evaluation").and_then(|v| v.as_object());
    fill_theta(d, evaluation);
    let score = evaluation
        .and_then(|e| e.get("score"))
        .and_then(|v| v.as_f64());
    let checks = evaluation
        .and_then(|e| e.get("checks"))
        .and_then(|v| v.as_array());
    if let Some(checks) = checks {
        d.checks_readable = true;
        scan_checks(d, checks);
    }
    // Measured means BOTH a numeric score and a readable check array; the
    // counters of a body without them are a gap, not zeroes (REQ-CV-03).
    d.measured = score.is_some() && checks.is_some();
    if d.measured {
        d.score_milli = score.map(milli);
    } else {
        quality_counters_missing(d, token::QUALITY_REPORT);
        mark_quality_unmeasured(d);
    }
}

fn fill_theta(d: &mut Draft, evaluation: Option<&serde_json::Map<String, serde_json::Value>>) {
    match evaluation
        .and_then(|e| e.get("theta"))
        .and_then(|v| v.as_f64())
    {
        Some(theta) => d.theta_milli = Some(milli(theta)),
        None => d.token("theta_milli", token::QUALITY_THETA),
    }
}

fn scan_checks(d: &mut Draft, checks: &[serde_json::Value]) {
    for c in checks {
        let Some(obj) = c.as_object() else { continue };
        let Some(id) = obj.get("id").and_then(|v| v.as_str()) else {
            continue;
        };
        record_check(d, id, obj.get("score"), obj.get("state"));
    }
}

/// One check block: identity is always recorded (STUCK reads it without a
/// score); the counters tell a clean item (exactly 1.0) from the defects.
fn record_check(
    d: &mut Draft,
    id: &str,
    raw: Option<&serde_json::Value>,
    state: Option<&serde_json::Value>,
) {
    let numeric = raw.and_then(|v| v.as_f64());
    d.checks_seen.insert(id.to_string());
    d.scores.insert(id.to_string(), numeric);
    if state.and_then(|v| v.as_str()) == Some("PENDING_HUMAN") {
        // REQ-CV-03: the signature is a separate counter, never a score.
        d.pending += 1;
        d.unclean.insert(id.to_string());
        return;
    }
    match (raw, numeric) {
        (None, _) => {
            d.unscored += 1;
            d.unclean.insert(id.to_string());
        }
        (Some(_), Some(x)) if x < 1.0 => {
            d.low.insert(id.to_string());
            d.unclean.insert(id.to_string());
        }
        // A present but non-numeric score is a defect, not a missing
        // measurement — REQ-CV-03 AC1 («0.7»).
        (Some(_), None) => {
            d.invalid += 1;
            d.unclean.insert(id.to_string());
        }
        _ => {}
    }
}

/// The single spec version a review execution names as its input
/// (REQ-CV-04): exactly one resolvable REQUIREMENTS_SPEC, else no binding.
fn bound_spec(
    a: &Artifact,
    execs: &HashMap<String, RoleExecution>,
    by_id: &HashMap<String, Artifact>,
) -> Option<Artifact> {
    let exec = execs.get(&a.execution_id.as_ref()?.to_string())?;
    let mut found: Vec<&Artifact> = exec
        .inputs
        .iter()
        .filter_map(|i| by_id.get(i))
        .filter(|s| s.kind == ArtifactKind::RequirementsSpec)
        .collect();
    found.dedup_by_key(|s| s.id);
    (found.len() == 1).then(|| found[0].clone())
}

fn count_gap(d: &mut Draft, tok: &'static str) {
    d.token("requirements_count", tok);
    d.token("oq_count", tok);
}

/// The two counts come from the BOUND spec body, independent of whether the
/// review body itself is readable (REQ-CV-04 AC7).
fn fill_spec_counts(
    d: &mut Draft,
    bodies: &BodyCache<'_>,
    execs: &HashMap<String, RoleExecution>,
    by_id: &HashMap<String, Artifact>,
) {
    match bound_spec(&d.artifact, execs, by_id).map(|s| bodies.body(&s)) {
        None => count_gap(d, token::SPEC_BINDING),
        Some(Body::Json(map)) => read_counts(d, &map),
        Some(gap) => {
            let tok = spec_gap_token(&gap);
            if let Body::Failed(err) = gap {
                d.read_error = Some(err);
            }
            count_gap(d, tok);
        }
    }
}

fn read_counts(d: &mut Draft, map: &serde_json::Map<String, serde_json::Value>) {
    if let Some(reqs) = map.get("requirements").and_then(|v| v.as_array()) {
        d.requirements = Some(reqs.len());
    } else {
        d.token("requirements_count", token::SPEC_COUNTS);
    }
    if let Some(oq) = map.get("open_questions").and_then(|v| v.as_array()) {
        d.oq = Some(oq.len());
    } else {
        d.token("oq_count", token::SPEC_COUNTS);
    }
}

/// The verdict comes from the REVIEW body alone; an unreadable review makes
/// the point unmeasured but keeps the counts above (REQ-CV-04 AC7).
fn fill_review_verdict(d: &mut Draft, bodies: &BodyCache<'_>) {
    match bodies.body(&d.artifact) {
        Body::Json(map) => {
            d.measured = true;
            d.verdict = read_verdict(&map);
            if d.verdict.is_none() {
                d.token("verdict", token::REVIEW_VERDICT);
            }
        }
        Body::Failed(err) => {
            d.read_error = Some(err);
            d.token("verdict", token::READ_ERROR);
        }
        Body::Absent | Body::Unreadable => d.token("verdict", token::REVIEW_VERDICT),
    }
}

fn read_verdict(map: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    map.get("verdict")
        .and_then(|v| v.as_str())
        .filter(|v| matches!(*v, "APPROVED" | "NEEDS_REVISION" | "REJECTED"))
        .map(|v| v.to_string())
}

fn spec_draft(
    round: u32,
    a: Artifact,
    bodies: &BodyCache<'_>,
    execs: &HashMap<String, RoleExecution>,
    by_id: &HashMap<String, Artifact>,
) -> Draft {
    let mut d = Draft::new(round, a);
    set_time(&mut d, execs);
    fill_spec_counts(&mut d, bodies, execs, by_id);
    fill_review_verdict(&mut d, bodies);
    d
}

fn finish_quality(out: &mut [Draft]) {
    for i in 0..out.len() {
        if !out[i].measured {
            continue;
        }
        let Some(j) = i.checked_sub(1) else {
            out[i].token("closed_count", token::PREDECESSOR_UNMEASURED);
            out[i].token("delta_score_milli", token::PREDECESSOR_UNMEASURED);
            continue;
        };
        if !out[j].measured {
            out[i].token("closed_count", token::PREDECESSOR_UNMEASURED);
            out[i].token("delta_score_milli", token::PREDECESSOR_UNMEASURED);
            continue;
        }
        // A segment is proven only when BOTH profiles are known and equal
        // (REQ-CV-10): an unknown profile breaks the series like a change.
        let same_segment = match (&out[j].profile, &out[i].profile) {
            (Some(p), Some(c)) => p == c,
            _ => false,
        };
        if !same_segment {
            out[i].token("closed_count", token::SEGMENT_BOUNDARY);
            out[i].token("delta_score_milli", token::SEGMENT_BOUNDARY);
            continue;
        }
        out[i].comparable = true;
        out[i].delta_score = out[i]
            .score_milli
            .zip(out[j].score_milli)
            .map(|(c, p)| c - p);
        out[i].closed = Some(count_closed(&out[j], &out[i]));
    }
}

/// REQ-CV-06: a point is closed when the same check id was present and NOT
/// clean in the previous round (below 1.0, no score, an invalid score or a
/// waiting signature) and is exactly 1.0 now. A check that disappeared is
/// not a closure.
fn count_closed(prev: &Draft, cur: &Draft) -> usize {
    cur.scores
        .iter()
        .filter(|(id, s)| {
            let clean_now = s.map(|x| x == 1.0).unwrap_or(false);
            clean_now && prev.unclean.contains(*id)
        })
        .count()
}

fn finish_spec(out: &mut [Draft]) {
    for i in 0..out.len() {
        finish_spec_round(out, i);
    }
}

fn spec_round_missing(d: &mut Draft, tok: &'static str) {
    for field in ["delta_requirements", "delta_oq", "volume_direction"] {
        d.token(field, tok);
    }
}

fn finish_spec_round(out: &mut [Draft], i: usize) {
    if !out[i].measured {
        spec_round_missing(&mut out[i], token::CURRENT_UNMEASURED);
        return;
    }
    let Some(j) = i.checked_sub(1) else {
        spec_round_missing(&mut out[i], token::PREDECESSOR_UNMEASURED);
        return;
    };
    if !out[j].measured {
        spec_round_missing(&mut out[i], token::PREDECESSOR_UNMEASURED);
        return;
    }
    out[i].comparable = true;
    let (head, tail) = out.split_at_mut(i);
    let cur = &mut tail[0];
    let prev = &head[j];
    // A counter absent from either round is NOT zero: the delta and the
    // direction stay null with `counts_unavailable` (REQ-CV-06 AC6).
    cur.delta_req = cur
        .requirements
        .zip(prev.requirements)
        .map(|(c, p)| c as i64 - p as i64);
    cur.delta_oq = cur.oq.zip(prev.oq).map(|(c, p)| c as i64 - p as i64);
    if cur.delta_req.is_none() {
        cur.token("delta_requirements", token::COUNTS_UNAVAILABLE);
    }
    if cur.delta_oq.is_none() {
        cur.token("delta_oq", token::COUNTS_UNAVAILABLE);
    }
    cur.volume = volume_of(cur.delta_req, cur.delta_oq);
    if cur.volume.is_none() {
        cur.token("volume_direction", token::COUNTS_UNAVAILABLE);
    }
}

fn volume_of(delta_req: Option<i64>, delta_oq: Option<i64>) -> Option<&'static str> {
    match (delta_req, delta_oq) {
        (Some(dr), Some(dq)) if dr > 0 || dq > 0 => Some(volume::GROWING),
        (Some(dr), Some(dq)) if dr <= 0 && dq <= 0 && (dr < 0 || dq < 0) => Some(volume::SHRINKING),
        (Some(0), Some(0)) => Some(volume::FLAT),
        _ => None,
    }
}

/// STUCK (REQ-CV-07, ratified 07.11): the same check id in three adjacent
/// rounds of one proven profile segment whose check list is READABLE — the
/// score is never required and never read, identity is the whole predicate.
/// A round whose body has no readable `evaluation.checks` array, a round
/// without that id, and a profile change/unknown all break the chain. Ties
/// resolve to the lexicographically smallest id (REQ-CV-09 AC5).
fn stuck_at(out: &[Draft], i: usize) -> Option<(String, Vec<u32>)> {
    if i < 2 {
        return None;
    }
    let (a, b, c) = (&out[i - 2], &out[i - 1], &out[i]);
    if !(a.checks_readable && b.checks_readable && c.checks_readable) {
        return None;
    }
    let Some(p) = &c.profile else { return None };
    if a.profile.as_ref() != Some(p) || b.profile.as_ref() != Some(p) {
        return None;
    }
    let mut common: Vec<&String> = a
        .checks_seen
        .iter()
        .filter(|id| b.checks_seen.contains(*id) && c.checks_seen.contains(*id))
        .collect();
    common.sort();
    common
        .first()
        .map(|id| ((*id).clone(), vec![a.round, b.round, c.round]))
}

/// STUCK is older than every other rule and independent of whether the
/// score was measured (REQ-CV-09 AC2). A chain of three marks ALL THREE
/// members (REQ-CV-07 AC2); an unscored round with a readable list does NOT
/// fall out of the series. The earliest chain owns a shared point
/// (deterministic by ascending index).
fn stuck_bases(out: &[Draft]) -> Vec<Option<ModeBasis>> {
    let mut bases: Vec<Option<ModeBasis>> = vec![None; out.len()];
    for i in 0..out.len() {
        let Some((check_id, rounds)) = stuck_at(out, i) else {
            continue;
        };
        let basis = ModeBasis {
            rule: rule::STUCK,
            reason: None,
            rounds,
            check_id: Some(check_id),
        };
        for slot in &mut bases[i.saturating_sub(2)..=i] {
            if slot.is_none() {
                *slot = Some(basis.clone());
            }
        }
    }
    bases
}

fn quality_modes(out: &mut [Draft]) {
    let stuck = stuck_bases(out);
    for i in 0..out.len() {
        grade_quality(out, i, &stuck[i]);
    }
}

fn grade_quality(out: &mut [Draft], i: usize, stuck: &Option<ModeBasis>) {
    if let Some(basis) = stuck {
        out[i].mode = mode::STUCK;
        out[i].basis = basis.clone();
        return;
    }
    if !out[i].measured || !out[i].comparable {
        out[i].mode = mode::UNKNOWN;
        out[i].basis = ModeBasis::unknown(reason::INSUFFICIENT_DATA);
        return;
    }
    let Some(delta) = out[i].delta_score else {
        // Comparable without a delta cannot happen; fail closed rather than
        // panic on live data.
        out[i].mode = mode::UNKNOWN;
        out[i].basis = ModeBasis::unknown(reason::INSUFFICIENT_DATA);
        return;
    };
    let rounds = vec![out[i - 1].round, out[i].round];
    // REQ-CV-07 / ratified 08.11: +10 or more converges with NO closed item
    // required; a negative step diverges; anything in [0, gap) is a plateau
    // FROM THE FIRST such round (no two-round warm-up).
    let (fired_mode, fired_rule) = if delta >= GAP_MILLI {
        (mode::CONVERGING, rule::CONVERGING)
    } else if delta < 0 {
        (mode::DIVERGING, rule::DIVERGING)
    } else {
        (mode::PLATEAU, rule::PLATEAU)
    };
    out[i].mode = fired_mode;
    out[i].basis = ModeBasis::fired(fired_rule, rounds);
}

fn spec_modes(out: &mut [Draft]) {
    for i in 0..out.len() {
        if !out[i].measured || !out[i].comparable {
            out[i].mode = mode::UNKNOWN;
            out[i].basis = ModeBasis::unknown(reason::INSUFFICIENT_DATA);
            continue;
        }
        let approved = out[i].verdict.as_deref() == Some("APPROVED");
        let rounds = vec![out[i - 1].round, out[i].round];
        match out[i].volume {
            Some(volume::SHRINKING) => {
                out[i].mode = mode::CONVERGING;
                out[i].basis = ModeBasis::fired(rule::CONVERGING, rounds);
            }
            Some(volume::GROWING) if !approved => {
                out[i].mode = mode::DIVERGING;
                out[i].basis = ModeBasis::fired(rule::DIVERGING, rounds);
            }
            // REQ-CV-08 / ratified 08.11: a flat volume is a plateau FROM
            // THE FIRST flat round, whatever the verdict (APPROVED included).
            Some(volume::FLAT) => {
                out[i].mode = mode::PLATEAU;
                out[i].basis = ModeBasis::fired(rule::PLATEAU, rounds);
            }
            _ => {
                // Null volume (counts unavailable) and "approved yet growing"
                // both land here: no predicate fired, honestly reported.
                out[i].mode = mode::UNKNOWN;
                out[i].basis = ModeBasis::unknown(reason::NO_PREDICATE);
            }
        }
    }
}

#[derive(Default)]
struct QualityCounts {
    low: Option<usize>,
    unscored: Option<usize>,
    invalid: Option<usize>,
    pending: Option<usize>,
}

fn quality_counts(d: &Draft, stage: &'static str) -> QualityCounts {
    if stage == STAGE_QUALITY && d.measured {
        QualityCounts {
            low: Some(d.low.len()),
            unscored: Some(d.unscored),
            invalid: Some(d.invalid),
            pending: Some(d.pending),
        }
    } else {
        QualityCounts::default()
    }
}

/// Applicability table: a field is null-with-a-token only when its stage
/// can carry it. Inapplicable fields stay null WITHOUT a record (REQ-CV-05).
fn is_null(d: &Draft, stage: &'static str, field: &str) -> bool {
    match field {
        "score_milli" => d.score_milli.is_none(),
        "theta_milli" => d.theta_milli.is_none(),
        "low_count" | "unscored_count" | "invalid_count" | "pending_count" => {
            stage == STAGE_QUALITY && !d.measured
        }
        "closed_count" => d.closed.is_none(),
        "delta_score_milli" => d.delta_score.is_none(),
        "profile" => d.profile.is_none(),
        "verdict" => d.verdict.is_none(),
        "requirements_count" => d.requirements.is_none(),
        "oq_count" => d.oq.is_none(),
        "delta_requirements" => d.delta_req.is_none(),
        "delta_oq" => d.delta_oq.is_none(),
        "volume_direction" => d.volume.is_none(),
        "time_seconds" => d.time_seconds.is_none(),
        // invariant: every entry of QUALITY_FIELDS/SPEC_FIELDS/COMMON_FIELDS
        // is matched below; a new table entry without an arm must fail loudly
        // here instead of silently mis-tokenising a null field.
        other => unreachable!("field {other} is not in the applicability table"),
    }
}

fn token_of(d: &Draft, field: &'static str) -> &'static str {
    d.tokens.get(field).copied().unwrap_or_else(|| {
        // invariant: every null field writes its token where it became null;
        // the fallback only degrades a future table edit to a named gap in
        // release builds instead of panicking on live data.
        debug_assert!(false, "null field {field} has no token");
        token::QUALITY_REPORT
    })
}

fn missing_entries(d: &Draft, stage: &'static str) -> Vec<MissingEntry> {
    let fields = if stage == STAGE_QUALITY {
        QUALITY_FIELDS
    } else {
        SPEC_FIELDS
    };
    fields
        .iter()
        .chain(COMMON_FIELDS.iter())
        .copied()
        .filter(|field| is_null(d, stage, field))
        .map(|field| MissingEntry {
            field,
            token: token_of(d, field),
        })
        .collect()
}

/// Build the point: every key always present, `missing[]` carrying exactly
/// one record for each APPLICABLE null field (REQ-CV-05).
fn into_point(d: Draft, stage: &'static str) -> ConvergencePoint {
    let missing = missing_entries(&d, stage);
    let counts = quality_counts(&d, stage);
    ConvergencePoint {
        round: d.round,
        artifact_id: d.artifact.id.to_string(),
        created_at: d.artifact.created_at,
        stage,
        status: d.artifact.status.as_str().to_string(),
        measured: d.measured,
        comparable: d.comparable,
        missing,
        score_milli: d.score_milli,
        theta_milli: d.theta_milli,
        low_count: counts.low,
        unscored_count: counts.unscored,
        invalid_count: counts.invalid,
        pending_count: counts.pending,
        closed_count: d.closed,
        delta_score_milli: d.delta_score,
        verdict: d.verdict,
        requirements_count: d.requirements,
        oq_count: d.oq,
        delta_requirements: d.delta_req,
        delta_oq: d.delta_oq,
        volume_direction: d.volume,
        time_seconds: d.time_seconds,
        profile: d.profile,
        mode: d.mode,
        mode_basis: d.basis,
        read_error: d.read_error,
    }
}

struct StageTimes {
    total: Option<i64>,
    current: Option<i64>,
}

fn counters_of(series: &[ConvergencePoint], specs_without_review: usize) -> ConvergenceCounters {
    let mut counters = ConvergenceCounters {
        rounds_total: series.len(),
        spec_versions_without_review: specs_without_review,
        ..Default::default()
    };
    for p in series {
        if !p.measured {
            counters.rounds_unmeasured += 1;
        }
        if p.time_seconds.is_none() {
            counters.rounds_without_time += 1;
        }
    }
    counters
}

/// REQ-CV-11: the CURRENT time is the profile segment the stage stands in —
/// the last contiguous segment with one known profile for quality, the whole
/// series for spec (a spec point has no profile). An unknown segment has no
/// "current" span to add up.
fn segment_start(name: &'static str, series: &[ConvergencePoint]) -> usize {
    if name != STAGE_QUALITY {
        return 0;
    }
    let Some(tail) = series.last().and_then(|p| p.profile.as_deref()) else {
        return series.len();
    };
    let mut start = series.len().saturating_sub(1);
    while start > 0 && series[start - 1].profile.as_deref() == Some(tail) {
        start -= 1;
    }
    start
}

fn stage_times(name: &'static str, series: &[ConvergencePoint]) -> StageTimes {
    // An unknown total is null, never 0 — even when every round lacks time.
    let known = series.iter().filter(|p| p.time_seconds.is_some()).count();
    let total = (known > 0).then(|| series.iter().filter_map(|p| p.time_seconds).sum());
    let start = segment_start(name, series);
    let current: Option<i64> = {
        let seconds: Vec<i64> = series[start..]
            .iter()
            .filter_map(|p| p.time_seconds)
            .collect();
        (!seconds.is_empty()).then(|| seconds.iter().sum())
    };
    StageTimes { total, current }
}

/// One `missing[]` record per applicable null stage field (REQ-CV-12).
fn stage_missing(total: Option<i64>, current_known: bool) -> Vec<MissingEntry> {
    let mut missing = Vec::new();
    if total.is_none() {
        missing.push(MissingEntry {
            field: "stage_total_seconds",
            token: token::STAGE_TIME,
        });
    }
    if !current_known {
        missing.push(MissingEntry {
            field: "stage_current_seconds",
            token: token::STAGE_TIME,
        });
    }
    missing
}

fn stage(
    name: &'static str,
    drafts: Vec<Draft>,
    spec_versions_without_review: usize,
) -> StageConvergence {
    let series: Vec<ConvergencePoint> = drafts.into_iter().map(|d| into_point(d, name)).collect();
    let counters = counters_of(&series, spec_versions_without_review);
    let times = stage_times(name, &series);
    StageConvergence {
        stage: name,
        series,
        counters,
        stage_total_seconds: times.total,
        stage_current_seconds: times.current,
        missing: stage_missing(times.total, times.current.is_some()),
    }
}

fn index_execs(executions: &[RoleExecution]) -> HashMap<String, RoleExecution> {
    executions
        .iter()
        .cloned()
        .map(|e| (e.id.to_string(), e))
        .collect()
}

fn index_artifacts(artifacts: &[Artifact]) -> HashMap<String, Artifact> {
    artifacts
        .iter()
        .cloned()
        .map(|a| (a.id.to_string(), a))
        .collect()
}

/// Versions without a review are a COUNTER, not rounds (REQ-CV-12 AC2):
/// count spec rows no review execution names as its single spec input.
fn specs_without_review(
    artifacts: &[Artifact],
    review_rows: &[Artifact],
    execs: &HashMap<String, RoleExecution>,
    by_id: &HashMap<String, Artifact>,
) -> usize {
    let mut reviewed: HashSet<Id> = HashSet::new();
    for a in review_rows {
        if let Some(spec) = bound_spec(a, execs, by_id) {
            reviewed.insert(spec.id);
        }
    }
    let total = artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::RequirementsSpec)
        .count();
    total.saturating_sub(reviewed.len())
}

fn spec_stage(
    rows: Vec<Artifact>,
    bodies: &BodyCache<'_>,
    execs: &HashMap<String, RoleExecution>,
    by_id: &HashMap<String, Artifact>,
    specs_without_review: usize,
) -> StageConvergence {
    let mut drafts: Vec<Draft> = rows
        .into_iter()
        .enumerate()
        .map(|(i, a)| spec_draft(i as u32 + 1, a, bodies, execs, by_id))
        .collect();
    finish_spec(&mut drafts);
    spec_modes(&mut drafts);
    stage(STAGE_SPEC_REVIEW, drafts, specs_without_review)
}

fn quality_stage(
    rows: Vec<Artifact>,
    bodies: &BodyCache<'_>,
    execs: &HashMap<String, RoleExecution>,
) -> StageConvergence {
    let mut drafts: Vec<Draft> = rows
        .into_iter()
        .enumerate()
        .map(|(i, a)| quality_draft(i as u32 + 1, a, bodies, execs))
        .collect();
    finish_quality(&mut drafts);
    quality_modes(&mut drafts);
    stage(STAGE_QUALITY, drafts, 0)
}

/// The whole slice of one task. Read-only by construction: the only store
/// capability it asks for is the memoised body reader.
pub fn compute(
    artifacts: &[Artifact],
    executions: &[RoleExecution],
    bodies: &BodyCache<'_>,
) -> ConvergenceView {
    let execs = index_execs(executions);
    let by_id = index_artifacts(artifacts);
    let q_rows = carriers(artifacts, ArtifactKind::QualityReport);
    let s_rows = carriers(artifacts, ArtifactKind::ReviewReport);
    let specs_without_review = specs_without_review(artifacts, &s_rows, &execs, &by_id);

    let mut stages = Vec::new();
    if !s_rows.is_empty() {
        stages.push(spec_stage(
            s_rows,
            bodies,
            &execs,
            &by_id,
            specs_without_review,
        ));
    }
    if !q_rows.is_empty() {
        stages.push(quality_stage(q_rows, bodies, &execs));
    }

    ConvergenceView {
        gap_milli: GAP_MILLI,
        stages,
    }
}
