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

/// The closed `missing[]` vocabulary: exactly thirteen tokens (REQ-CV-05 AC3).
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

/// One round of a stage — ONE shape for both stages, 26 keys always present
/// (REQ-CV-05). Fields that do not apply to the stage are null and carry no
/// `missing[]` record.
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

    /// The stage a wait row draws (REQ-CV-13): the gate's own stage when it
    /// has one, otherwise the LAST stage of the slice.
    pub fn stage_for_gate(&self, gate_stage: Option<&str>) -> Option<&StageConvergence> {
        gate_stage
            .and_then(|name| self.stage(name))
            .or_else(|| self.stages.last())
    }
}

/// One body outcome: absent blob, unreadable/not-an-object body, or a
/// usable JSON object. All three are distinct outcomes; none becomes `{}`.
#[derive(Clone, Debug)]
pub enum Body {
    Absent,
    Unreadable,
    Json(serde_json::Map<String, serde_json::Value>),
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
        let classify = |bytes: Vec<u8>| match std::str::from_utf8(&bytes)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        {
            Some(serde_json::Value::Object(map)) => Body::Json(map),
            _ => Body::Unreadable,
        };
        BodyCache {
            source: Box::new(move |a: &Artifact| match read(a) {
                Ok(bytes) => classify(bytes),
                Err(StoreError::NotFound(..)) => Body::Absent,
                Err(_) => Body::Unreadable,
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

fn quality_draft(
    round: u32,
    a: Artifact,
    bodies: &BodyCache<'_>,
    execs: &HashMap<String, RoleExecution>,
) -> Draft {
    let mut d = Draft::new(round, a);
    d.time_seconds = exec_time(&d.artifact, execs);
    if d.time_seconds.is_none() {
        d.token("time_seconds", token::TIME);
    }

    let map = match bodies.body(&d.artifact) {
        Body::Absent | Body::Unreadable => {
            // No usable report: the five metric counters share ONE reason
            // record each (REQ-CV-03 AC5), the threshold and profile have
            // their own.
            for field in [
                "score_milli",
                "low_count",
                "unscored_count",
                "invalid_count",
                "pending_count",
            ] {
                d.token(field, token::QUALITY_REPORT);
            }
            d.token("theta_milli", token::QUALITY_THETA);
            d.token("profile", token::PROFILE);
            mark_quality_unmeasured(&mut d);
            return d;
        }
        Body::Json(map) => map,
    };

    d.profile = profile_of(&map);
    if d.profile.is_none() {
        d.token("profile", token::PROFILE);
    }

    let evaluation = map.get("evaluation").and_then(|v| v.as_object());
    let score = evaluation
        .and_then(|e| e.get("score"))
        .and_then(|v| v.as_f64());
    let checks = evaluation
        .and_then(|e| e.get("checks"))
        .and_then(|v| v.as_array());
    if let Some(theta) = evaluation
        .and_then(|e| e.get("theta"))
        .and_then(|v| v.as_f64())
    {
        d.theta_milli = Some(milli(theta));
    } else {
        d.token("theta_milli", token::QUALITY_THETA);
    }

    if let Some(checks) = checks {
        d.checks_readable = true;
        for c in checks {
            let Some(obj) = c.as_object() else { continue };
            let Some(id) = obj.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let raw = obj.get("score");
            let numeric = raw.and_then(|v| v.as_f64());
            let state = obj.get("state").and_then(|v| v.as_str());
            d.checks_seen.insert(id.to_string());
            d.scores.insert(id.to_string(), numeric);
            if state == Some("PENDING_HUMAN") {
                // REQ-CV-03: the signature is a separate counter, never a
                // score of the round.
                d.pending += 1;
                d.unclean.insert(id.to_string());
                continue;
            }
            match (raw, numeric) {
                (None, _) => {
                    // A block with no score at all — REQ-CV-03 AC1.
                    d.unscored += 1;
                    d.unclean.insert(id.to_string());
                }
                (Some(_), Some(x)) => {
                    if x < 1.0 {
                        d.low.insert(id.to_string());
                        d.unclean.insert(id.to_string());
                    }
                }
                (Some(_), None) => {
                    // A present but non-numeric score is a defect, not a
                    // missing measurement — REQ-CV-03 AC1 («0.7»).
                    d.invalid += 1;
                    d.unclean.insert(id.to_string());
                }
            }
        }
    }

    d.measured = score.is_some() && checks.is_some();
    if !d.measured {
        for field in [
            "score_milli",
            "low_count",
            "unscored_count",
            "invalid_count",
            "pending_count",
        ] {
            d.token(field, token::QUALITY_REPORT);
        }
        mark_quality_unmeasured(&mut d);
        return d;
    }

    d.score_milli = score.map(milli);
    d
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

fn spec_draft(
    round: u32,
    a: Artifact,
    bodies: &BodyCache<'_>,
    execs: &HashMap<String, RoleExecution>,
    by_id: &HashMap<String, Artifact>,
) -> Draft {
    let mut d = Draft::new(round, a);
    d.time_seconds = exec_time(&d.artifact, execs);
    if d.time_seconds.is_none() {
        d.token("time_seconds", token::TIME);
    }

    // The spec version is bound by the stored execution row — exactly one
    // resolvable REQUIREMENTS_SPEC among its inputs (REQ-CV-04). Binding
    // and counters are independent of the review body's readability.
    let spec = d
        .artifact
        .execution_id
        .as_ref()
        .and_then(|id| execs.get(&id.to_string()))
        .and_then(|e| {
            let mut found: Vec<&Artifact> = e
                .inputs
                .iter()
                .filter_map(|i| by_id.get(i))
                .filter(|s| s.kind == ArtifactKind::RequirementsSpec)
                .collect();
            found.dedup_by_key(|s| s.id);
            (found.len() == 1).then(|| found[0].clone())
        });

    match spec.map(|s| bodies.body(&s)) {
        None => {
            for field in ["requirements_count", "oq_count"] {
                d.token(field, token::SPEC_BINDING);
            }
        }
        Some(Body::Absent) | Some(Body::Unreadable) => {
            for field in ["requirements_count", "oq_count"] {
                d.token(field, token::SPEC_BODY);
            }
        }
        Some(Body::Json(smap)) => {
            if let Some(reqs) = smap.get("requirements").and_then(|v| v.as_array()) {
                d.requirements = Some(reqs.len());
            } else {
                d.token("requirements_count", token::SPEC_COUNTS);
            }
            if let Some(oq) = smap.get("open_questions").and_then(|v| v.as_array()) {
                d.oq = Some(oq.len());
            } else {
                d.token("oq_count", token::SPEC_COUNTS);
            }
        }
    }

    match bodies.body(&d.artifact) {
        Body::Json(map) => {
            d.measured = true;
            d.verdict = map
                .get("verdict")
                .and_then(|v| v.as_str())
                .filter(|v| matches!(*v, "APPROVED" | "NEEDS_REVISION" | "REJECTED"))
                .map(|v| v.to_string());
            if d.verdict.is_none() {
                d.token("verdict", token::REVIEW_VERDICT);
            }
        }
        Body::Absent | Body::Unreadable => {
            // The ONE cause of measured=false on a spec point: the review
            // body could not be read (REQ-CV-04 AC7). The counters above
            // stay numbers when binding and spec body were readable.
            d.token("verdict", token::REVIEW_VERDICT);
        }
    }
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
        if !out[i].measured {
            for field in ["delta_requirements", "delta_oq", "volume_direction"] {
                out[i].token(field, token::CURRENT_UNMEASURED);
            }
            continue;
        }
        let Some(j) = i.checked_sub(1) else {
            for field in ["delta_requirements", "delta_oq", "volume_direction"] {
                out[i].token(field, token::PREDECESSOR_UNMEASURED);
            }
            continue;
        };
        if !out[j].measured {
            for field in ["delta_requirements", "delta_oq", "volume_direction"] {
                out[i].token(field, token::PREDECESSOR_UNMEASURED);
            }
            continue;
        }
        out[i].comparable = true;
        // A counter absent from either round is NOT zero: the delta and the
        // direction stay null with `counts_unavailable` (REQ-CV-06 AC6).
        out[i].delta_req = out[i]
            .requirements
            .zip(out[j].requirements)
            .map(|(c, p)| c as i64 - p as i64);
        out[i].delta_oq = out[i].oq.zip(out[j].oq).map(|(c, p)| c as i64 - p as i64);
        if out[i].delta_req.is_none() {
            out[i].token("delta_requirements", token::COUNTS_UNAVAILABLE);
        }
        if out[i].delta_oq.is_none() {
            out[i].token("delta_oq", token::COUNTS_UNAVAILABLE);
        }
        out[i].volume = match (out[i].delta_req, out[i].delta_oq) {
            (Some(dr), Some(dq)) if dr > 0 || dq > 0 => Some(volume::GROWING),
            (Some(dr), Some(dq)) if dr <= 0 && dq <= 0 && (dr < 0 || dq < 0) => {
                Some(volume::SHRINKING)
            }
            (Some(0), Some(0)) => Some(volume::FLAT),
            _ => {
                out[i].token("volume_direction", token::COUNTS_UNAVAILABLE);
                None
            }
        };
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

fn quality_modes(out: &mut [Draft]) {
    // STUCK is older than every other rule and independent of whether the
    // score was measured (REQ-CV-09 AC2). A chain of three marks ALL THREE
    // members (REQ-CV-07 AC2: «все три STUCK, средний круг не даёт
    // insufficient_data»); an unscored round with a readable list does NOT
    // fall out of the series. The earliest chain owns a shared point
    // (deterministic by ascending index).
    let mut stuck_basis: Vec<Option<ModeBasis>> = vec![None; out.len()];
    for i in 0..out.len() {
        if let Some((check_id, rounds)) = stuck_at(out, i) {
            let basis = ModeBasis {
                rule: rule::STUCK,
                reason: None,
                rounds,
                check_id: Some(check_id),
            };
            let from = i.saturating_sub(2);
            for slot in &mut stuck_basis[from..=i] {
                if slot.is_none() {
                    *slot = Some(basis.clone());
                }
            }
        }
    }
    for i in 0..out.len() {
        if let Some(basis) = &stuck_basis[i] {
            out[i].mode = mode::STUCK;
            out[i].basis = basis.clone();
            continue;
        }
        if !out[i].measured || !out[i].comparable {
            out[i].mode = mode::UNKNOWN;
            out[i].basis = ModeBasis::unknown(reason::INSUFFICIENT_DATA);
            continue;
        }
        let delta = out[i]
            .delta_score
            .expect("comparable quality round has a delta");
        // REQ-CV-07 / ratified 08.11: +10 or more converges with NO closed
        // item required; a negative step diverges; anything in [0, gap) is a
        // plateau FROM THE FIRST such round (no two-round warm-up).
        let rounds = vec![out[i - 1].round, out[i].round];
        if delta >= GAP_MILLI {
            out[i].mode = mode::CONVERGING;
            out[i].basis = ModeBasis::fired(rule::CONVERGING, rounds);
        } else if delta < 0 {
            out[i].mode = mode::DIVERGING;
            out[i].basis = ModeBasis::fired(rule::DIVERGING, rounds);
        } else {
            out[i].mode = mode::PLATEAU;
            out[i].basis = ModeBasis::fired(rule::PLATEAU, rounds);
        }
    }
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

/// Build the 26-field point: every key always present, `missing[]` carrying
/// exactly one record for each APPLICABLE null field (REQ-CV-05).
fn into_point(d: Draft, stage: &'static str) -> ConvergencePoint {
    let fields: Vec<&'static str> = if stage == STAGE_QUALITY {
        QUALITY_FIELDS
            .iter()
            .chain(COMMON_FIELDS.iter())
            .copied()
            .collect()
    } else {
        SPEC_FIELDS
            .iter()
            .chain(COMMON_FIELDS.iter())
            .copied()
            .collect()
    };
    let mut missing = Vec::new();
    for field in fields {
        let is_null = match field {
            "score_milli" => d.score_milli.is_none(),
            "theta_milli" => d.theta_milli.is_none(),
            "low_count" => stage == STAGE_QUALITY && !d.measured,
            "unscored_count" => stage == STAGE_QUALITY && !d.measured,
            "invalid_count" => stage == STAGE_QUALITY && !d.measured,
            "pending_count" => stage == STAGE_QUALITY && !d.measured,
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
            other => unreachable!("field {other} is not in the applicability table"),
        };
        if is_null {
            let token = d.tokens.get(field).copied().unwrap_or_else(|| {
                debug_assert!(false, "null field {field} has no token");
                token::QUALITY_REPORT
            });
            missing.push(MissingEntry { field, token });
        }
    }
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
        low_count: if stage == STAGE_QUALITY && d.measured {
            Some(d.low.len())
        } else {
            None
        },
        unscored_count: if stage == STAGE_QUALITY && d.measured {
            Some(d.unscored)
        } else {
            None
        },
        invalid_count: if stage == STAGE_QUALITY && d.measured {
            Some(d.invalid)
        } else {
            None
        },
        pending_count: if stage == STAGE_QUALITY && d.measured {
            Some(d.pending)
        } else {
            None
        },
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
    }
}

fn stage(
    name: &'static str,
    drafts: Vec<Draft>,
    spec_versions_without_review: usize,
) -> StageConvergence {
    let series: Vec<ConvergencePoint> = drafts.into_iter().map(|d| into_point(d, name)).collect();
    let mut counters = ConvergenceCounters {
        rounds_total: series.len(),
        spec_versions_without_review,
        ..Default::default()
    };
    for p in &series {
        if !p.measured {
            counters.rounds_unmeasured += 1;
        }
        if p.time_seconds.is_none() {
            counters.rounds_without_time += 1;
        }
    }
    // REQ-CV-11: the total counts every known round time; the CURRENT time
    // is the profile segment the stage stands in — the last contiguous
    // segment with one known profile for quality, the whole series for spec
    // (a spec point has no profile). An unknown total is null + stage_time,
    // never 0.
    let total: i64 = series.iter().filter_map(|p| p.time_seconds).sum();
    let stage_total_seconds = if series.is_empty() || counters.rounds_without_time == series.len() {
        None
    } else {
        Some(total)
    };
    let segment_start = if name == STAGE_QUALITY {
        match series.last().and_then(|p| p.profile.clone()) {
            Some(tail) => {
                let mut start = series.len().saturating_sub(1);
                while start > 0 && series[start - 1].profile.as_deref() == Some(tail.as_str()) {
                    start -= 1;
                }
                start
            }
            // An unknown last profile proves no segment: there is no
            // "current" span to add up.
            None => series.len(),
        }
    } else {
        0
    };
    let current_known: Vec<i64> = series[segment_start..]
        .iter()
        .filter_map(|p| p.time_seconds)
        .collect();
    let stage_current_seconds = if current_known.is_empty() {
        None
    } else {
        Some(current_known.iter().sum())
    };
    StageConvergence {
        stage: name,
        counters,
        missing: stage_missing(stage_total_seconds, stage_current_seconds.is_some()),
        stage_total_seconds,
        stage_current_seconds,
        series,
    }
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

/// The whole slice of one task. Read-only by construction: the only store
/// capability it asks for is the memoised body reader.
pub fn compute(
    artifacts: &[Artifact],
    executions: &[RoleExecution],
    bodies: &BodyCache<'_>,
) -> ConvergenceView {
    let execs: HashMap<String, RoleExecution> = executions
        .iter()
        .cloned()
        .map(|e| (e.id.to_string(), e))
        .collect();
    let by_id: HashMap<String, Artifact> = artifacts
        .iter()
        .cloned()
        .map(|a| (a.id.to_string(), a))
        .collect();

    let q_rows = carriers(artifacts, ArtifactKind::QualityReport);
    let s_rows = carriers(artifacts, ArtifactKind::ReviewReport);

    // Versions without a review are a COUNTER, not rounds (REQ-CV-12 AC2):
    // count spec rows no review execution names as its single spec input.
    let mut reviewed_specs: HashSet<Id> = HashSet::new();
    for a in &s_rows {
        let Some(exec) = a
            .execution_id
            .as_ref()
            .and_then(|id| execs.get(&id.to_string()))
        else {
            continue;
        };
        let mut found: Vec<&Artifact> = exec
            .inputs
            .iter()
            .filter_map(|i| by_id.get(i))
            .filter(|s| s.kind == ArtifactKind::RequirementsSpec)
            .collect();
        found.dedup_by_key(|s| s.id);
        if found.len() == 1 {
            reviewed_specs.insert(found[0].id);
        }
    }
    let spec_total = artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::RequirementsSpec)
        .count();
    let specs_without_review = spec_total.saturating_sub(reviewed_specs.len());

    let mut stages = Vec::new();

    if !s_rows.is_empty() {
        let mut drafts: Vec<Draft> = s_rows
            .into_iter()
            .enumerate()
            .map(|(i, a)| spec_draft(i as u32 + 1, a, bodies, &execs, &by_id))
            .collect();
        finish_spec(&mut drafts);
        spec_modes(&mut drafts);
        stages.push(stage(STAGE_SPEC_REVIEW, drafts, specs_without_review));
    }

    if !q_rows.is_empty() {
        let mut drafts: Vec<Draft> = q_rows
            .into_iter()
            .enumerate()
            .map(|(i, a)| quality_draft(i as u32 + 1, a, bodies, &execs))
            .collect();
        finish_quality(&mut drafts);
        quality_modes(&mut drafts);
        stages.push(stage(STAGE_QUALITY, drafts, 0));
    }

    ConvergenceView {
        gap_milli: GAP_MILLI,
        stages,
    }
}
