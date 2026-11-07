//! The convergence slice (task 01a1b5be, REQ-CV-01..13): a COMPUTED read
//! view over data the store already holds — published QUALITY_REPORT /
//! REVIEW_REPORT rows, the role executions they name and the blobs behind
//! them. Nothing here writes: no rows, no tables, no events, no cache
//! (REQ-CV-01), and no score is re-derived from weights — the slice quotes
//! the stored `evaluation.score` / `evaluation.theta` of the body.
//!
//! One round of a stage is one published carrier artifact. `artifacts.seq`
//! is NOT a round number (it is a per-kind version counter written as
//! MAX(seq)+1), so the round is the 1-based index in the full
//! `(seq, created_at, id)` order computed HERE, never stored (REQ-CV-02).
//!
//! Bodies are parsed as RAW JSON, not through `EvaluationBlock`: both
//! `evaluation.score` and `evaluation.theta` are `serde(default)` in the
//! typed shape, so an absent key there is indistinguishable from `0.0`
//! (REQ-CV-03). The three-outcome read policy (absent blob / unreadable
//! body / usable object) and the closed `missing[]` vocabulary below are
//! what keeps a broken round an honest gap instead of a zero.

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Utc};
use devai_core::types::{Artifact, ArtifactKind, Id, RoleExecution, TaskState};
use devai_store::StoreError;
use serde::Serialize;

/// The step that counts as progress (REQ-CV-06, ratified OQ #1): 0.01,
/// compared in whole thousandths, a platform constant — NOT a config knob.
pub const GAP_MILLI: i64 = 10;

pub const STAGE_SPEC_REVIEW: &str = "spec_review";
pub const STAGE_QUALITY: &str = "quality";

/// The closed mode vocabulary (REQ-CV-05). The same four tokens every
/// consumer branches on; no free text reaches the view.
pub mod mode {
    pub const CONVERGING: &str = "CONVERGING";
    pub const DIVERGING: &str = "DIVERGING";
    pub const PLATEAU: &str = "PLATEAU";
    pub const STUCK: &str = "STUCK";
}

/// The closed reason vocabulary for a point that carries no mode
/// (REQ-CV-05). Fixed order of the note STEPS, not of this list.
pub mod note {
    pub const INSUFFICIENT_DATA: &str = "insufficient_data";
    pub const NO_PREDECESSOR: &str = "no_predecessor";
    pub const PROFILE_CHANGED: &str = "profile_changed";
    pub const PROFILE_UNKNOWN: &str = "profile_unknown";
    pub const NONADJACENT_PREDECESSOR: &str = "nonadjacent_predecessor";
    pub const PLATEAU_PENDING: &str = "plateau_pending";
    pub const PROGRESS_WITHOUT_CLOSURE: &str = "progress_without_closure";
    pub const SHRINKING_WITHOUT_CLOSED_ITEMS: &str = "shrinking_without_closed_items";
    pub const GROWTH_WITH_APPROVED: &str = "growth_with_approved";
}

/// The closed `missing[]` vocabulary (REQ-CV-13), in ITS printed order.
pub mod missing {
    pub const REPORT_BODY_ABSENT: &str = "report_body_absent";
    pub const REPORT_BODY_UNREADABLE: &str = "report_body_unreadable";
    pub const SPEC_BODY_ABSENT: &str = "spec_body_absent";
    pub const SPEC_BODY_UNREADABLE: &str = "spec_body_unreadable";
    pub const SCORE_ABSENT: &str = "score_absent";
    pub const CHECKS_ABSENT: &str = "checks_absent";
    pub const VERDICT_ABSENT: &str = "verdict_absent";
    pub const SPEC_UNBOUND: &str = "spec_unbound";
    pub const REQUIREMENTS_ABSENT: &str = "requirements_absent";
    pub const THETA_ABSENT: &str = "theta_absent";
    pub const TIME_ABSENT: &str = "time_absent";
}

/// REQ-CV-13: the printed order is the enumeration's, never the order the
/// code happened to check things in.
const MISSING_ORDER: &[&str] = &[
    missing::REPORT_BODY_ABSENT,
    missing::REPORT_BODY_UNREADABLE,
    missing::SPEC_BODY_ABSENT,
    missing::SPEC_BODY_UNREADABLE,
    missing::SCORE_ABSENT,
    missing::CHECKS_ABSENT,
    missing::VERDICT_ABSENT,
    missing::SPEC_UNBOUND,
    missing::REQUIREMENTS_ABSENT,
    missing::THETA_ABSENT,
    missing::TIME_ABSENT,
];

/// The nine reasons that make a point UNMEASURED (REQ-CV-13 AC2: measured
/// is false exactly when one of these is present).
fn unmeasuring(reason: &str) -> bool {
    !matches!(reason, missing::THETA_ABSENT | missing::TIME_ABSENT)
}

fn rank(reason: &str) -> usize {
    MISSING_ORDER
        .iter()
        .position(|r| *r == reason)
        .unwrap_or(MISSING_ORDER.len())
}

/// REQ-CV-09: the published per-stage slice of `task.convergence`.
#[derive(Serialize, Clone, Debug)]
pub struct StageConvergence {
    pub stage: String,
    pub series: Vec<ConvergencePoint>,
    pub counters: ConvergenceCounters,
    pub stage_total_seconds: i64,
}

#[derive(Serialize, Clone, Debug, Default)]
pub struct ConvergenceCounters {
    /// Points with no usable measurement (any of the nine reasons).
    pub unmeasured: usize,
    /// REQUIREMENTS_SPEC rows no review round is bound to (REQ-CV-04).
    pub spec_versions_without_review: usize,
    /// Points whose round time is unknown — the coverage denominator fact.
    pub rounds_without_time: usize,
}

/// One round of a stage. Fields that do not apply to the stage are `null`
/// (quality has no verdict/requirements, spec_review has no score/theta).
#[derive(Serialize, Clone, Debug)]
pub struct ConvergencePoint {
    pub round: u32,
    pub artifact_id: String,
    pub seq: u32,
    pub status: String,
    pub created_at: DateTime<Utc>,
    pub measured: bool,
    pub missing: Vec<&'static str>,
    pub profile: Option<String>,
    pub comparable: bool,
    pub time_seconds: Option<i64>,
    pub score: Option<f64>,
    pub score_milli: Option<i64>,
    pub theta_milli: Option<i64>,
    pub low_count: Option<usize>,
    pub unscored_count: Option<usize>,
    pub closed_count: Option<usize>,
    pub delta_score_milli: Option<i64>,
    pub delta_low: Option<i64>,
    pub distance: Option<u32>,
    pub requirements_count: Option<usize>,
    pub oq_count: Option<usize>,
    pub verdict: Option<String>,
    pub delta_requirements: Option<i64>,
    pub delta_oq: Option<i64>,
    pub modes: Vec<&'static str>,
    pub mode_note: Option<&'static str>,
    pub stuck_applicable: bool,
}

/// The whole `task.convergence` response (REQ-CV-09).
#[derive(Serialize, Clone, Debug)]
pub struct ConvergenceView {
    pub task: String,
    pub snapshot_at: DateTime<Utc>,
    pub gap_milli: i64,
    /// `no_rounds` when the task published no carrier at all; `null`
    /// otherwise (an empty `stages[]` is never an error).
    pub reason: Option<&'static str>,
    pub stages: Vec<StageConvergence>,
}

/// The `inbox.snapshot` fact (REQ-CV-09): the mini-chart's data — the last
/// point's modes, note and reasons, never report or spec bodies.
#[derive(Serialize, Clone, Debug)]
pub struct ConvergenceFact {
    pub stage: String,
    pub gap_milli: i64,
    pub rounds: usize,
    pub series: Vec<ConvergenceTick>,
    pub modes: Vec<&'static str>,
    pub mode_note: Option<&'static str>,
    pub missing: Vec<&'static str>,
}

#[derive(Serialize, Clone, Debug)]
pub struct ConvergenceTick {
    pub round: u32,
    pub measured: bool,
    pub score_milli: Option<i64>,
    pub theta_milli: Option<i64>,
    pub requirements_count: Option<usize>,
    pub oq_count: Option<usize>,
    pub modes: Vec<&'static str>,
}

impl ConvergenceFact {
    /// The fact of a gate whose stage has published no carrier yet: the
    /// stage token and the empty series, so the row can be honest about
    /// «кругов пока нет» instead of hiding the panel.
    pub fn empty(stage: &str) -> ConvergenceFact {
        ConvergenceFact {
            stage: stage.to_string(),
            gap_milli: GAP_MILLI,
            rounds: 0,
            series: Vec::new(),
            modes: Vec::new(),
            mode_note: None,
            missing: Vec::new(),
        }
    }

    /// The last round's modes/note/reasons — the row's highlight and its
    /// honest gap list. A stage with no rounds carries empty vectors.
    pub fn of_stage(stage: &StageConvergence) -> ConvergenceFact {
        let last = stage.series.last();
        ConvergenceFact {
            stage: stage.stage.clone(),
            gap_milli: GAP_MILLI,
            rounds: stage.series.len(),
            series: stage
                .series
                .iter()
                .map(|p| ConvergenceTick {
                    round: p.round,
                    measured: p.measured,
                    score_milli: p.score_milli,
                    theta_milli: p.theta_milli,
                    requirements_count: p.requirements_count,
                    oq_count: p.oq_count,
                    modes: p.modes.clone(),
                })
                .collect(),
            modes: last.map(|p| p.modes.clone()).unwrap_or_default(),
            mode_note: last.and_then(|p| p.mode_note),
            missing: last.map(|p| p.missing.clone()).unwrap_or_default(),
        }
    }
}

/// The grid stage token of a stored state (REQ-CV-09): only the two
/// signable gates carry a fact; every other state answers `None`.
pub fn stage_for_state(state: TaskState) -> Option<&'static str> {
    match state {
        TaskState::Approval => Some(STAGE_SPEC_REVIEW),
        TaskState::Qa => Some(STAGE_QUALITY),
        _ => None,
    }
}

/// The body reader seam: the API hands the store's ONE read policy
/// (`Store::read_artifact_body`); tests hand a map. `Err` is classified
/// below, never folded into an empty body.
type BodyReader<'a> = &'a dyn Fn(&Artifact) -> Result<Vec<u8>, StoreError>;

enum Body {
    Absent,
    Unreadable,
    Json(serde_json::Map<String, serde_json::Value>),
}

/// REQ-CV-03 three outcomes: no blob, a blob that cannot be read/parsed as
/// a JSON object, a usable object. A `Corrupt` store error, a non-UTF-8
/// body and a JSON array are all the SAME outcome — unreadable — because
/// none of them yields fields this slice may quote.
fn read_object(read: BodyReader<'_>, a: &Artifact) -> (Body, Option<&'static str>) {
    match read(a) {
        Err(StoreError::NotFound(..)) => (Body::Absent, Some(missing::REPORT_BODY_ABSENT)),
        Err(_) => (Body::Unreadable, Some(missing::REPORT_BODY_UNREADABLE)),
        Ok(bytes) => match std::str::from_utf8(&bytes)
            .ok()
            .and_then(|s| serde_json::from_str::<serde_json::Value>(s).ok())
        {
            Some(serde_json::Value::Object(map)) => (Body::Json(map), None),
            _ => (Body::Unreadable, Some(missing::REPORT_BODY_UNREADABLE)),
        },
    }
}

fn read_spec_object(read: BodyReader<'_>, a: &Artifact) -> (Body, Option<&'static str>) {
    match read_object(read, a) {
        (Body::Absent, _) => (Body::Absent, Some(missing::SPEC_BODY_ABSENT)),
        (Body::Unreadable, _) => (Body::Unreadable, Some(missing::SPEC_BODY_UNREADABLE)),
        (Body::Json(m), _) => (Body::Json(m), None),
    }
}

/// `round(x * 1000)` half-up (REQ-CV-06): scores are non-negative, so
/// `f64::round` (half away from zero) is the ratified comparison.
fn milli(x: f64) -> i64 {
    (x * 1000.0).round() as i64
}

/// Finished minus started, in whole seconds; a negative difference and an
/// unfinished execution give NO time (REQ-CV-08), never a zero.
fn round_seconds(exec: Option<&RoleExecution>) -> Option<i64> {
    let e = exec?;
    let finished = e.finished_at?;
    let secs = (finished - e.started_at).num_seconds();
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

/// A point mid-flight: measurement is decided first, modes last (they need
/// the neighbours).
struct Draft {
    round: u32,
    artifact: Artifact,
    measured: bool,
    missing: Vec<&'static str>,
    profile: Option<String>,
    score: Option<f64>,
    score_milli: Option<i64>,
    theta_milli: Option<i64>,
    low: HashSet<String>,
    unscored: usize,
    scores: HashMap<String, Option<f64>>,
    requirements: Option<usize>,
    oq: Option<usize>,
    verdict: Option<String>,
    time_seconds: Option<i64>,
    // filled in the second pass
    comparable: bool,
    distance: Option<u32>,
    closed: Option<usize>,
    delta_score: Option<i64>,
    delta_low: Option<i64>,
    delta_requirements: Option<i64>,
    delta_oq: Option<i64>,
    modes: Vec<&'static str>,
    mode_note: Option<&'static str>,
}

impl Draft {
    fn new(round: u32, artifact: Artifact) -> Draft {
        Draft {
            round,
            artifact,
            measured: false,
            missing: Vec::new(),
            profile: None,
            score: None,
            score_milli: None,
            theta_milli: None,
            low: HashSet::new(),
            unscored: 0,
            scores: HashMap::new(),
            requirements: None,
            oq: None,
            verdict: None,
            time_seconds: None,
            comparable: false,
            distance: None,
            closed: None,
            delta_score: None,
            delta_low: None,
            delta_requirements: None,
            delta_oq: None,
            modes: Vec::new(),
            mode_note: None,
        }
    }
}

fn profile_of(map: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
    // REQ-CV-12: "project@version" from the stored body's top-level
    // `profile` object; anything unreadable is `null`, never a guess.
    let p = map.get("profile")?.as_object()?;
    let project = p.get("project")?.as_str()?;
    let version = p.get("version")?.as_u64()?;
    Some(format!("{project}@{version}"))
}

fn quality_draft(
    round: u32,
    a: Artifact,
    read: BodyReader<'_>,
    execs: &HashMap<String, RoleExecution>,
) -> Draft {
    let mut d = Draft::new(round, a);
    let (body, reason) = read_object(read, &d.artifact);
    if let Some(r) = reason {
        d.missing.push(r);
        return d;
    }
    let Body::Json(map) = body else {
        unreachable!("read_object returns Json or a reason");
    };

    d.profile = profile_of(&map);
    let evaluation = map.get("evaluation").and_then(|v| v.as_object());
    let score = evaluation
        .and_then(|e| e.get("score"))
        .and_then(|v| v.as_f64());
    let checks = evaluation
        .and_then(|e| e.get("checks"))
        .and_then(|v| v.as_array());
    let theta = evaluation
        .and_then(|e| e.get("theta"))
        .and_then(|v| v.as_f64());

    if let Some(s) = score {
        d.score = Some(s);
        d.score_milli = Some(milli(s));
    } else {
        d.missing.push(missing::SCORE_ABSENT);
    }
    if checks.is_none() {
        d.missing.push(missing::CHECKS_ABSENT);
    }

    if let Some(checks) = checks {
        for c in checks {
            let Some(obj) = c.as_object() else { continue };
            let Some(id) = obj.get("id").and_then(|v| v.as_str()) else {
                continue;
            };
            let s = obj.get("score").and_then(|v| v.as_f64());
            match s {
                None => d.unscored += 1,
                Some(x) if x < 1.0 => {
                    d.low.insert(id.to_string());
                }
                Some(_) => {}
            }
            d.scores.insert(id.to_string(), s);
        }
    }

    d.measured = score.is_some() && checks.is_some();
    if !d.measured {
        // An unmeasured point carries no time and no threshold: reasons do
        // not cascade (REQ-CV-13 AC5), so a broken body is never padded
        // with theta_absent/time_absent noise.
        d.theta_milli = None;
        d.low.clear();
        d.scores.clear();
        return d;
    }
    // REQ-CV-13: theta_absent belongs to a MEASURED point — the score is
    // there, only the line of the threshold is missing.
    if theta.is_some() {
        d.theta_milli = theta.map(milli);
    } else {
        d.missing.push(missing::THETA_ABSENT);
    }

    match round_seconds(exec_of(&d.artifact, execs)) {
        Some(s) => d.time_seconds = Some(s),
        None => d.missing.push(missing::TIME_ABSENT),
    }
    d
}

/// The execution row a carrier names, if it resolves inside this task.
fn exec_of<'a>(
    a: &Artifact,
    execs: &'a HashMap<String, RoleExecution>,
) -> Option<&'a RoleExecution> {
    execs.get(&a.execution_id?.to_string())
}

fn spec_draft(
    round: u32,
    a: Artifact,
    read: BodyReader<'_>,
    execs: &HashMap<String, RoleExecution>,
    by_id: &HashMap<String, Artifact>,
) -> Draft {
    let mut d = Draft::new(round, a);
    let (body, reason) = read_object(read, &d.artifact);
    if let Some(r) = reason {
        d.missing.push(r);
        return d;
    }
    let Body::Json(map) = body else {
        unreachable!("read_object returns Json or a reason");
    };

    let verdict = map
        .get("verdict")
        .and_then(|v| v.as_str())
        .filter(|v| matches!(*v, "APPROVED" | "NEEDS_REVISION" | "REJECTED"));
    match verdict {
        Some(v) => d.verdict = Some(v.to_string()),
        None => d.missing.push(missing::VERDICT_ABSENT),
    }

    // REQ-CV-04: the spec version is bound by the stored execution row —
    // exactly one resolvable REQUIREMENTS_SPEC among its inputs. Time,
    // seq or neighbouring events never restore it.
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

    let spec = if let Some(spec) = spec {
        match read_spec_object(read, &spec) {
            (Body::Json(smap), _) => match smap.get("requirements").and_then(|v| v.as_array()) {
                Some(reqs) => {
                    d.requirements = Some(reqs.len());
                    d.oq = Some(
                        smap.get("open_questions")
                            .and_then(|v| v.as_array())
                            .map(|v| v.len())
                            .unwrap_or(0),
                    );
                    true
                }
                None => {
                    d.missing.push(missing::REQUIREMENTS_ABSENT);
                    false
                }
            },
            (_, Some(r)) => {
                d.missing.push(r);
                false
            }
            _ => false,
        }
    } else {
        d.missing.push(missing::SPEC_UNBOUND);
        false
    };

    d.measured = verdict.is_some() && spec;
    if !d.measured {
        d.requirements = None;
        d.oq = None;
        return d;
    }

    match round_seconds(
        execs.get(
            &d.artifact
                .execution_id
                .map(|e| e.to_string())
                .unwrap_or_default(),
        ),
    ) {
        Some(s) => d.time_seconds = Some(s),
        None => d.missing.push(missing::TIME_ABSENT),
    }
    d
}

fn finish_quality(drafts: Vec<Draft>) -> Vec<Draft> {
    let mut out = drafts;
    for i in 0..out.len() {
        if !out[i].measured {
            continue;
        }
        let prev = (0..i).rev().find(|j| out[*j].measured);
        let Some(prev) = prev else {
            continue;
        };
        out[i].distance = Some(out[i].round - out[prev].round);
        let comparable = match (&out[prev].profile, &out[i].profile) {
            (Some(p), Some(c)) => p == c,
            _ => false,
        };
        out[i].comparable = comparable;
        if !comparable {
            continue;
        }
        let prev_score = out[prev].score_milli;
        let prev_low = out[prev].low_count_value();
        out[i].delta_score = prev_score.map(|p| out[i].score_milli.unwrap() - p);
        out[i].delta_low = prev_low.map(|p| out[i].low.len() as i64 - p as i64);
        let closed = out[i]
            .scores
            .iter()
            .filter(|(id, s)| s.map(|x| x >= 1.0).unwrap_or(false) && out[prev].low.contains(*id))
            .count();
        out[i].closed = Some(closed);
    }
    out
}

impl Draft {
    fn low_count_value(&self) -> Option<usize> {
        Some(self.low.len())
    }
}

fn finish_spec(drafts: Vec<Draft>) -> Vec<Draft> {
    let mut out = drafts;
    for i in 0..out.len() {
        if !out[i].measured {
            continue;
        }
        let prev = (0..i).rev().find(|j| out[*j].measured);
        let Some(prev) = prev else {
            continue;
        };
        out[i].distance = Some(out[i].round - out[prev].round);
        out[i].comparable = true;
        out[i].delta_requirements =
            Some(out[i].requirements.unwrap() as i64 - out[prev].requirements.unwrap() as i64);
        out[i].delta_oq = Some(out[i].oq.unwrap() as i64 - out[prev].oq.unwrap() as i64);
    }
    out
}

fn prev_measured_adjacent(out: &[Draft], i: usize) -> Option<usize> {
    i.checked_sub(1)
        .filter(|j| out[*j].measured && out[*j].distance == Some(1) && out[*j].comparable)
}

fn quality_modes(out: &mut [Draft]) {
    for i in 0..out.len() {
        if !out[i].measured {
            out[i].mode_note = Some(note::INSUFFICIENT_DATA);
            continue;
        }
        let mut modes = Vec::new();
        if let Some(delta) = out[i].delta_score {
            if delta >= GAP_MILLI && out[i].closed.unwrap_or(0) >= 1 {
                modes.push(mode::CONVERGING);
            }
            if delta < 0 {
                modes.push(mode::DIVERGING);
            }
            if (0..GAP_MILLI).contains(&delta) {
                // REQ-CV-06: two adjacent rounds in a row under the step.
                let prev = i.checked_sub(1).filter(|j| {
                    out[*j].measured && out[*j].distance == Some(1) && out[*j].comparable
                });
                let plateau = prev
                    .and_then(|j| out[j].delta_score)
                    .map(|d| (0..GAP_MILLI).contains(&d))
                    .unwrap_or(false);
                if out[i].distance == Some(1) && plateau {
                    modes.push(mode::PLATEAU);
                }
            }
        }
        if stuck_at(out, i) {
            modes.push(mode::STUCK);
        }
        if !modes.is_empty() {
            out[i].modes = modes;
            out[i].mode_note = None;
            continue;
        }
        out[i].mode_note = Some(quality_note(out, i));
    }
}

/// STUCK (REQ-CV-06, ratified decision on OQ #3): the same check-id in
/// three CONSECUTIVE measured rounds. The ratified decision forbids the
/// chain to cross a profile boundary — a profile change breaks the series,
/// so the segment check is part of the predicate here.
fn stuck_at(out: &[Draft], i: usize) -> bool {
    if i < 2 {
        return false;
    }
    let (a, b, c) = (&out[i - 2], &out[i - 1], &out[i]);
    if !(a.measured && b.measured && c.measured) {
        return false;
    }
    if c.distance != Some(1) || b.distance != Some(1) {
        return false;
    }
    let Some(p) = &c.profile else { return false };
    if a.profile.as_ref() != Some(p) || b.profile.as_ref() != Some(p) {
        return false;
    }
    a.scores
        .keys()
        .any(|id| b.scores.contains_key(id) && c.scores.contains_key(id))
}

fn quality_note(out: &[Draft], i: usize) -> &'static str {
    let p = &out[i];
    if !p.measured {
        return note::INSUFFICIENT_DATA;
    }
    let Some(prev) = (0..i).rev().find(|j| out[*j].measured) else {
        return note::NO_PREDECESSOR;
    };
    if !p.comparable {
        return match (&out[prev].profile, &p.profile) {
            (Some(a), Some(b)) if a != b => note::PROFILE_CHANGED,
            _ => note::PROFILE_UNKNOWN,
        };
    }
    let delta = p.delta_score.unwrap_or(0);
    if (0..GAP_MILLI).contains(&delta) {
        if p.distance != Some(1) {
            return note::NONADJACENT_PREDECESSOR;
        }
        return note::PLATEAU_PENDING;
    }
    if delta >= GAP_MILLI {
        return note::PROGRESS_WITHOUT_CLOSURE;
    }
    // delta < 0 is DIVERGING; unreachable, kept honest.
    note::NO_PREDECESSOR
}

fn spec_modes(out: &mut [Draft]) {
    for i in 0..out.len() {
        if !out[i].measured {
            if out[i].modes.is_empty() {
                out[i].mode_note = Some(note::INSUFFICIENT_DATA);
            }
            continue;
        }
        let Some(dr) = out[i].delta_requirements else {
            out[i].mode_note = Some(note::NO_PREDECESSOR);
            continue;
        };
        let dq = out[i].delta_oq.unwrap_or(0);
        let growing = dr > 0 || dq > 0;
        let shrinking = dr <= 0 && dq <= 0 && (dr < 0 || dq < 0);
        let flat = dr == 0 && dq == 0;
        let approved = out[i].verdict.as_deref() == Some("APPROVED");

        let mut modes = Vec::new();
        if dr <= 0 && dq <= -1 {
            modes.push(mode::CONVERGING);
        }
        if growing && !approved {
            modes.push(mode::DIVERGING);
        }
        if flat {
            let prev_flat = if let Some(j) = prev_measured_adjacent(out, i) {
                out[j].delta_requirements == Some(0) && out[j].delta_oq == Some(0)
            } else {
                false
            };
            if out[i].distance == Some(1) && prev_flat {
                modes.push(mode::PLATEAU);
            }
        }
        if !modes.is_empty() {
            out[i].modes = modes;
            out[i].mode_note = None;
            continue;
        }
        out[i].mode_note = Some(if growing {
            note::GROWTH_WITH_APPROVED
        } else if flat {
            if out[i].distance == Some(1) {
                note::PLATEAU_PENDING
            } else {
                note::NONADJACENT_PREDECESSOR
            }
        } else if shrinking {
            note::SHRINKING_WITHOUT_CLOSED_ITEMS
        } else {
            note::NO_PREDECESSOR
        });
    }
}

fn into_points(
    drafts: Vec<Draft>,
    stuck_applicable: bool,
) -> (Vec<ConvergencePoint>, usize, usize) {
    let is_quality = stuck_applicable;
    let mut unmeasured = 0;
    let mut without_time = 0;
    let points = drafts
        .into_iter()
        .map(|d| {
            let mut missing = d.missing;
            missing.sort_by_key(|r| rank(r));
            missing.dedup();
            debug_assert!(d.measured == !missing.iter().any(|r| unmeasuring(r)));
            if !d.measured {
                unmeasured += 1;
            }
            if d.time_seconds.is_none() {
                without_time += 1;
            }
            ConvergencePoint {
                round: d.round,
                artifact_id: d.artifact.id.to_string(),
                seq: d.artifact.seq,
                status: d.artifact.status.as_str().to_string(),
                created_at: d.artifact.created_at,
                measured: d.measured,
                missing,
                profile: d.profile,
                comparable: d.comparable,
                time_seconds: d.time_seconds,
                score: d.score,
                score_milli: d.score_milli,
                theta_milli: d.theta_milli,
                // The check-level counters belong to the quality stage: a
                // spec round has no checks, and a zero there would read as
                // «no low items» instead of «not applicable».
                low_count: is_quality.then_some(d.low.len()),
                unscored_count: is_quality.then_some(d.unscored),
                closed_count: d.closed,
                delta_score_milli: d.delta_score,
                delta_low: d.delta_low,
                distance: d.distance,
                requirements_count: d.requirements,
                oq_count: d.oq,
                verdict: d.verdict,
                delta_requirements: d.delta_requirements,
                delta_oq: d.delta_oq,
                modes: d.modes,
                mode_note: d.mode_note,
                stuck_applicable,
            }
        })
        .collect();
    (points, unmeasured, without_time)
}

/// The whole slice for one task. Read-only by construction: the only store
/// capability it asks for is the body reader.
pub fn compute(
    task_id: Id,
    artifacts: &[Artifact],
    executions: &[RoleExecution],
    read: BodyReader<'_>,
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

    // Every spec version a review round is bound to — the denominator of
    // `spec_versions_without_review` (REQ-CV-04: versions without a review
    // are a counter, not rounds).
    let mut reviewed_specs: HashSet<Id> = HashSet::new();
    for a in &s_rows {
        if let Some(e) = a
            .execution_id
            .as_ref()
            .and_then(|id| execs.get(&id.to_string()))
        {
            for input in &e.inputs {
                if let Some(spec) = by_id.get(input) {
                    if spec.kind == ArtifactKind::RequirementsSpec {
                        reviewed_specs.insert(spec.id);
                    }
                }
            }
        }
    }
    let spec_total = artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::RequirementsSpec)
        .count();
    let specs_without_review = spec_total.saturating_sub(reviewed_specs.len());

    let mut stages = Vec::new();

    if !s_rows.is_empty() {
        let drafts = s_rows
            .into_iter()
            .enumerate()
            .map(|(i, a)| spec_draft(i as u32 + 1, a, read, &execs, &by_id))
            .collect();
        let mut drafts = finish_spec(drafts);
        spec_modes(&mut drafts);
        let total: i64 = drafts.iter().filter_map(|d| d.time_seconds).sum();
        let (series, unmeasured, without_time) = into_points(drafts, false);
        stages.push(StageConvergence {
            stage: STAGE_SPEC_REVIEW.to_string(),
            series,
            counters: ConvergenceCounters {
                unmeasured,
                spec_versions_without_review: specs_without_review,
                rounds_without_time: without_time,
            },
            stage_total_seconds: total,
        });
    }

    if !q_rows.is_empty() {
        let drafts = q_rows
            .into_iter()
            .enumerate()
            .map(|(i, a)| quality_draft(i as u32 + 1, a, read, &execs))
            .collect();
        let mut drafts = finish_quality(drafts);
        quality_modes(&mut drafts);
        let total: i64 = drafts.iter().filter_map(|d| d.time_seconds).sum();
        let (series, unmeasured, without_time) = into_points(drafts, true);
        stages.push(StageConvergence {
            stage: STAGE_QUALITY.to_string(),
            series,
            counters: ConvergenceCounters {
                unmeasured,
                spec_versions_without_review: 0,
                rounds_without_time: without_time,
            },
            stage_total_seconds: total,
        });
    }

    let reason = stages.is_empty().then_some("no_rounds");
    ConvergenceView {
        task: task_id.to_string(),
        snapshot_at: Utc::now(),
        gap_milli: GAP_MILLI,
        reason,
        stages,
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;
    use devai_core::types::{ArtifactStatus, DataCategory, ExecutionStatus, new_id};
    use serde_json::json;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    fn artifact(kind: ArtifactKind, seq: u32, at: i64, execution: Option<Id>) -> Artifact {
        Artifact {
            id: new_id(),
            task_id: new_id(),
            kind,
            name: kind.as_str().to_string(),
            seq,
            category: DataCategory::Artifact,
            status: ArtifactStatus::Current,
            sha256: "0".repeat(64),
            size: 0,
            media_type: "application/json".into(),
            contract: None,
            created_by: "tester".into(),
            execution_id: execution,
            depends_on: vec![],
            created_at: ts(at),
        }
    }

    fn execution(id: Id, inputs: &[Id], seconds: Option<i64>) -> RoleExecution {
        RoleExecution {
            id,
            task_id: new_id(),
            role_slug: "qa".into(),
            role_version: 1,
            agent_name: None,
            status: ExecutionStatus::Completed,
            inputs: inputs.iter().map(|i| i.to_string()).collect(),
            summary: None,
            error: None,
            started_at: ts(0),
            finished_at: seconds.map(ts),
            workspace_revision: None,
            attempt: 1,
            parent_execution: None,
            failure_class: None,
            owner_pid: None,
            owner_host: None,
            owner_process_start: None,
        }
    }

    #[derive(Clone)]
    enum Blob {
        Body(String),
        Absent,
        Corrupt,
    }

    /// The body-reader seam: the SAME three outcomes the store's ONE read
    /// policy produces, without a store home.
    fn reader(blobs: Vec<(Id, Blob)>) -> impl Fn(&Artifact) -> Result<Vec<u8>, StoreError> {
        let map: HashMap<String, Blob> = blobs
            .into_iter()
            .map(|(id, b)| (id.to_string(), b))
            .collect();
        move |a: &Artifact| match map.get(&a.id.to_string()).cloned().unwrap_or(Blob::Absent) {
            Blob::Body(s) => Ok(s.into_bytes()),
            Blob::Absent => Err(StoreError::NotFound(
                "artifact blob".into(),
                a.sha256.clone(),
            )),
            Blob::Corrupt => Err(StoreError::Corrupt(format!("body of {} unreadable", a.id))),
        }
    }

    fn qbody(
        score: Option<f64>,
        theta: Option<f64>,
        profile: Option<(&str, u64)>,
        checks: Option<Vec<(&str, Option<f64>)>>,
    ) -> String {
        let mut ev = json!({ "verdict": "PASS", "gates": [] });
        if let Some(s) = score {
            ev["score"] = json!(s);
        }
        if let Some(t) = theta {
            ev["theta"] = json!(t);
        }
        if let Some(checks) = checks {
            let arr: Vec<serde_json::Value> = checks
                .iter()
                .map(|(id, s)| match s {
                    Some(x) => json!({ "id": id, "score": x }),
                    None => json!({ "id": id }),
                })
                .collect();
            ev["checks"] = json!(arr);
        }
        let mut doc = json!({ "verdict": "ACCEPT", "evaluation": ev });
        if let Some((p, v)) = profile {
            doc["profile"] = json!({ "project": p, "version": v });
        }
        doc.to_string()
    }

    fn one_check(score: f64) -> Option<Vec<(&'static str, Option<f64>)>> {
        Some(vec![("gates.pass", Some(score))])
    }

    /// A body whose check ids are unique PER ROUND (`id@round`): the fixture
    /// for the delta/mode arithmetic, where a repeated check id would — by
    /// the literal STUCK rule — legitimately fire on the third round and
    /// hide what the test is pinning. STUCK itself is pinned separately
    /// with deliberately repeated ids.
    fn qbody_round(
        score: Option<f64>,
        theta: Option<f64>,
        profile: Option<(&str, u64)>,
        checks: Option<Vec<(&str, Option<f64>)>>,
        round: u32,
    ) -> String {
        match checks {
            None => qbody(score, theta, profile, None),
            Some(list) => {
                let owned: Vec<(String, Option<f64>)> = list
                    .iter()
                    .map(|(id, s)| (format!("{id}@{round}"), *s))
                    .collect();
                let refs: Vec<(&str, Option<f64>)> =
                    owned.iter().map(|(i, s)| (i.as_str(), *s)).collect();
                qbody(score, theta, profile, Some(refs))
            }
        }
    }

    fn quality(
        scores: &[(f64, &str, u64)],
        checks: Option<Vec<(&str, Option<f64>)>>,
    ) -> (ConvergenceView, Vec<Artifact>) {
        let arts: Vec<Artifact> = scores
            .iter()
            .enumerate()
            .map(|(i, _)| artifact(ArtifactKind::QualityReport, i as u32 + 1, i as i64, None))
            .collect();
        let blobs: Vec<(Id, Blob)> = arts
            .iter()
            .enumerate()
            .zip(scores)
            .map(|((i, a), (s, p, v))| {
                (
                    a.id,
                    Blob::Body(qbody_round(
                        Some(*s),
                        None,
                        Some((*p, *v)),
                        checks.clone(),
                        i as u32 + 1,
                    )),
                )
            })
            .collect();
        let read = reader(blobs);
        let view = compute(new_id(), &arts, &[], &read);
        (view, arts)
    }

    fn stage<'a>(view: &'a ConvergenceView, name: &str) -> &'a StageConvergence {
        view.stages
            .iter()
            .find(|s| s.stage == name)
            .unwrap_or_else(|| panic!("stage {name} absent: {:?}", view.reason))
    }

    fn notes(view: &ConvergenceView) -> Vec<Option<&'static str>> {
        stage(view, STAGE_QUALITY)
            .series
            .iter()
            .map(|p| p.mode_note)
            .collect()
    }

    // ---- REQ-CV-06: the numbers of the measured curve -------------------

    #[test]
    fn quality_curve_is_quoted_in_thousandths_without_smoothing() {
        // The operator's own curve: 0.943 → 0.997 → 0.991 → 1.000.
        let (view, _) = quality(
            &[
                (0.943, "gates", 12),
                (0.997, "gates", 12),
                (0.991, "gates", 12),
                (1.000, "gates", 12),
            ],
            one_check(1.0),
        );
        let s = stage(&view, STAGE_QUALITY);
        let m: Vec<i64> = s.series.iter().filter_map(|p| p.score_milli).collect();
        assert_eq!(m, vec![943, 997, 991, 1000], "no smoothing, no re-scoring");
        assert_eq!(
            s.series[0].delta_score_milli, None,
            "first point has no delta"
        );
        assert_eq!(s.series[1].delta_score_milli, Some(54));
        assert_eq!(s.series[2].delta_score_milli, Some(-6));
        assert_eq!(s.series[3].delta_score_milli, Some(9));
        assert_eq!(s.series[2].modes, vec!["DIVERGING"], "the drop is the mode");
        assert_eq!(
            s.series[3].mode_note,
            Some(note::PLATEAU_PENDING),
            "+9 is under the step and the previous round fell, so no plateau yet"
        );
        assert_eq!(view.gap_milli, 10);
    }

    #[test]
    fn converging_needs_progress_and_a_closed_item() {
        let (view, _) = quality(
            &[(0.940, "p", 1), (0.980, "p", 1)],
            Some(vec![("gates.pass", Some(1.0))]),
        );
        assert_eq!(
            stage(&view, STAGE_QUALITY).series[1].modes,
            Vec::<&str>::new(),
            "no item was closed: +40 alone is progress without closure"
        );
        assert_eq!(notes(&view)[1], Some(note::PROGRESS_WITHOUT_CLOSURE));

        let (view, _) = quality(
            &[(0.940, "p", 1), (0.980, "p", 1)],
            Some(vec![("gates.pass", Some(1.0)), ("gates.low", Some(0.4))]),
        );
        // Round 1 has a low item, round 2 sets it to 1.0 → one closure.
        assert_eq!(notes(&view)[1], Some(note::PROGRESS_WITHOUT_CLOSURE));
    }

    #[test]
    fn converging_fires_when_the_low_item_closes() {
        let a1 = artifact(ArtifactKind::QualityReport, 1, 0, None);
        let a2 = artifact(ArtifactKind::QualityReport, 2, 1, None);
        let read = reader(vec![
            (
                a1.id,
                Blob::Body(qbody(
                    Some(0.940),
                    None,
                    Some(("p", 1)),
                    Some(vec![("gates.low", Some(0.4)), ("gates.pass", Some(1.0))]),
                )),
            ),
            (
                a2.id,
                Blob::Body(qbody(
                    Some(0.980),
                    None,
                    Some(("p", 1)),
                    Some(vec![("gates.low", Some(1.0)), ("gates.pass", Some(1.0))]),
                )),
            ),
        ]);
        let view = compute(new_id(), &[a1, a2], &[], &read);
        let p = &stage(&view, STAGE_QUALITY).series[1];
        assert_eq!(p.delta_score_milli, Some(40));
        assert_eq!(p.closed_count, Some(1));
        assert_eq!(p.modes, vec!["CONVERGING"]);
        assert_eq!(p.mode_note, None);
    }

    #[test]
    fn gap_boundary_is_exact_in_thousandths() {
        let at = |delta: f64| {
            let (view, _) = quality(
                &[(0.940, "p", 1), (0.940 + delta, "p", 1)],
                Some(vec![("c", Some(0.5)), ("d", Some(1.0))]),
            );
            view.stages[0].series[1].delta_score_milli
        };
        assert_eq!(at(0.010), Some(10));
        assert_eq!(at(0.009), Some(9));
    }

    // ---- REQ-CV-03: raw-JSON presence, not the typed defaults -----------

    #[test]
    fn absent_score_and_checks_are_reasons_not_zeros() {
        let a = artifact(ArtifactKind::QualityReport, 1, 0, None);
        let read = reader(vec![(
            a.id,
            Blob::Body(qbody(None, Some(0.8), None, one_check(1.0))),
        )]);
        let view = compute(new_id(), &[a], &[], &read);
        let p = &stage(&view, STAGE_QUALITY).series[0];
        assert!(!p.measured);
        assert_eq!(p.score, None);
        assert_eq!(p.score_milli, None);
        assert_eq!(p.missing, vec![missing::SCORE_ABSENT]);

        let a = artifact(ArtifactKind::QualityReport, 1, 0, None);
        let read = reader(vec![(a.id, Blob::Body(qbody(Some(0.5), None, None, None)))]);
        let view = compute(new_id(), &[a], &[], &read);
        let p = &stage(&view, STAGE_QUALITY).series[0];
        assert!(!p.measured);
        assert_eq!(p.missing, vec![missing::CHECKS_ABSENT]);
    }

    #[test]
    fn a_recorded_zero_score_stays_a_measurement() {
        let a = artifact(ArtifactKind::QualityReport, 1, 0, None);
        let read = reader(vec![(
            a.id,
            Blob::Body(qbody(Some(0.0), Some(0.0), None, one_check(0.0))),
        )]);
        let view = compute(new_id(), &[a], &[], &read);
        let p = &stage(&view, STAGE_QUALITY).series[0];
        assert!(p.measured);
        assert_eq!(p.score, Some(0.0));
        assert_eq!(p.score_milli, Some(0));
        assert_eq!(
            p.theta_milli,
            Some(0),
            "a stored theta of 0 is a line at zero"
        );
        assert!(!p.missing.contains(&missing::THETA_ABSENT));
        assert_eq!(p.low_count, Some(1));
    }

    #[test]
    fn absent_theta_keeps_the_point_measured() {
        let a = artifact(ArtifactKind::QualityReport, 1, 0, None);
        let read = reader(vec![(
            a.id,
            Blob::Body(qbody(Some(0.7), None, None, one_check(1.0))),
        )]);
        let view = compute(new_id(), &[a], &[], &read);
        let p = &stage(&view, STAGE_QUALITY).series[0];
        assert!(p.measured);
        assert_eq!(p.theta_milli, None, "no threshold is no line, never 0.80");
        assert!(p.missing.contains(&missing::THETA_ABSENT));
    }

    #[test]
    fn missing_and_unreadable_bodies_are_gaps_not_zeros() {
        let absent = artifact(ArtifactKind::QualityReport, 1, 0, None);
        let corrupt = artifact(ArtifactKind::QualityReport, 2, 1, None);
        let array = artifact(ArtifactKind::QualityReport, 3, 2, None);
        let healthy = artifact(ArtifactKind::QualityReport, 4, 3, None);
        let read = reader(vec![
            (absent.id, Blob::Absent),
            (corrupt.id, Blob::Corrupt),
            (array.id, Blob::Body("[1,2,3]".into())),
            (
                healthy.id,
                Blob::Body(qbody(Some(0.85), None, Some(("p", 1)), one_check(1.0))),
            ),
        ]);
        let view = compute(new_id(), &[absent, corrupt, array, healthy], &[], &read);
        let s = stage(&view, STAGE_QUALITY);
        for p in &s.series[..3] {
            assert!(!p.measured);
            assert_eq!(p.score, None, "a broken round is never a zero score");
            assert_eq!(p.score_milli, None);
            assert_eq!(p.missing.len(), 1, "reasons never cascade: {:?}", p.missing);
        }
        assert_eq!(s.series[0].missing, vec![missing::REPORT_BODY_ABSENT]);
        assert_eq!(s.series[1].missing, vec![missing::REPORT_BODY_UNREADABLE]);
        assert_eq!(s.series[2].missing, vec![missing::REPORT_BODY_UNREADABLE]);
        assert_eq!(s.series[0].mode_note, Some(note::INSUFFICIENT_DATA));
        assert_eq!(
            s.series[3].score_milli,
            Some(850),
            "a healthy neighbour keeps its own fact"
        );
        assert_eq!(s.counters.unmeasured, 3);
    }

    #[test]
    fn missing_reasons_print_in_the_enumeration_order() {
        let a = artifact(ArtifactKind::QualityReport, 1, 0, None);
        let mut doc: serde_json::Value =
            serde_json::from_str(&qbody(Some(0.5), None, None, None)).unwrap();
        doc["evaluation"].as_object_mut().unwrap().remove("score");
        let read = reader(vec![(a.id, Blob::Body(doc.to_string()))]);
        let view = compute(new_id(), &[a], &[], &read);
        assert_eq!(
            stage(&view, STAGE_QUALITY).series[0].missing,
            vec![missing::SCORE_ABSENT, missing::CHECKS_ABSENT],
            "the printed order is the vocabulary's, not the check order"
        );
    }

    #[test]
    fn low_and_unscored_counts_come_from_the_checks_array() {
        let a = artifact(ArtifactKind::QualityReport, 1, 0, None);
        let read = reader(vec![(
            a.id,
            Blob::Body(qbody(
                Some(0.66),
                None,
                Some(("p", 1)),
                Some(vec![
                    ("mech.fail", Some(0.0)),
                    ("mech.pass", Some(1.0)),
                    ("blocked", None),
                    ("pending-human", None),
                ]),
            )),
        )]);
        let view = compute(new_id(), &[a], &[], &read);
        let p = &stage(&view, STAGE_QUALITY).series[0];
        assert_eq!(p.low_count, Some(1), "MECHANICAL_FAIL = 0.0 is below 1.0");
        assert_eq!(p.unscored_count, Some(2), "BLOCKED/PENDING have no score");
    }

    // ---- REQ-CV-06: PLATEAU, non-adjacency, STUCK -----------------------

    #[test]
    fn plateau_needs_two_adjacent_weak_steps() {
        let (view, _) = quality(
            &[(0.900, "p", 1), (0.905, "p", 1), (0.908, "p", 1)],
            one_check(1.0),
        );
        let s = stage(&view, STAGE_QUALITY);
        assert_eq!(
            s.series[1].mode_note,
            Some(note::PLATEAU_PENDING),
            "one weak step is not a plateau"
        );
        assert_eq!(s.series[2].modes, vec!["PLATEAU"]);
        assert_eq!(s.series[2].mode_note, None);
    }

    #[test]
    fn a_skipped_round_is_non_adjacency_not_plateau() {
        let a1 = artifact(ArtifactKind::QualityReport, 1, 0, None);
        let broken = artifact(ArtifactKind::QualityReport, 2, 1, None);
        let a3 = artifact(ArtifactKind::QualityReport, 3, 2, None);
        let read = reader(vec![
            (
                a1.id,
                Blob::Body(qbody(Some(0.800), None, Some(("p", 1)), one_check(1.0))),
            ),
            (broken.id, Blob::Absent),
            (
                a3.id,
                Blob::Body(qbody(Some(0.805), None, Some(("p", 1)), one_check(1.0))),
            ),
        ]);
        let view = compute(new_id(), &[a1, broken, a3], &[], &read);
        let p = &stage(&view, STAGE_QUALITY).series[2];
        assert_eq!(p.distance, Some(2));
        assert_eq!(p.mode_note, Some(note::NONADJACENT_PREDECESSOR));
    }

    #[test]
    fn stuck_is_the_same_check_id_in_three_rounds_at_any_score() {
        let series = |scores: &[Option<f64>]| {
            let arts: Vec<Artifact> = (0..scores.len())
                .map(|i| artifact(ArtifactKind::QualityReport, i as u32 + 1, i as i64, None))
                .collect();
            let blobs: Vec<(Id, Blob)> = arts
                .iter()
                .zip(scores)
                .map(|(a, s)| {
                    (
                        a.id,
                        Blob::Body(qbody(
                            Some(0.9),
                            None,
                            Some(("p", 1)),
                            Some(vec![("gates.same", *s)]),
                        )),
                    )
                })
                .collect();
            let read = reader(blobs);
            compute(new_id(), &arts, &[], &read)
        };

        for scores in [
            vec![Some(0.7), Some(0.6), Some(0.7)],
            vec![Some(1.0), Some(1.0), Some(1.0)],
            vec![None, None, None],
        ] {
            let view = series(&scores);
            let s = stage(&view, STAGE_QUALITY);
            assert!(
                s.series[2].modes.contains(&"STUCK"),
                "scores: {scores:?} modes: {:?}",
                s.series[2].modes
            );
            assert!(
                s.series[2].modes.len() <= 2,
                "STUCK + at most one sign mode"
            );
            assert_eq!(s.series[0].modes, Vec::<&str>::new());
        }
    }

    #[test]
    fn an_unmeasured_round_breaks_the_stuck_chain() {
        let a1 = artifact(ArtifactKind::QualityReport, 1, 0, None);
        let a2 = artifact(ArtifactKind::QualityReport, 2, 1, None);
        let a3 = artifact(ArtifactKind::QualityReport, 3, 2, None);
        let a4 = artifact(ArtifactKind::QualityReport, 4, 3, None);
        let body = Blob::Body(qbody(
            Some(0.9),
            None,
            Some(("p", 1)),
            Some(vec![("gates.same", Some(1.0))]),
        ));
        let read = reader(vec![
            (a1.id, body.clone()),
            (a2.id, body.clone()),
            (a3.id, Blob::Absent),
            (a4.id, body),
        ]);
        let view = compute(new_id(), &[a1, a2, a3, a4], &[], &read);
        let s = stage(&view, STAGE_QUALITY);
        assert_eq!(s.series[3].modes, Vec::<&str>::new(), "3 rounds, not 4");
        assert_eq!(s.series[3].distance, Some(2));
    }

    // ---- REQ-CV-12: the profile segment ---------------------------------

    #[test]
    fn a_profile_change_breaks_deltas_and_the_stuck_chain() {
        let a1 = artifact(ArtifactKind::QualityReport, 1, 0, None);
        let a2 = artifact(ArtifactKind::QualityReport, 2, 1, None);
        let a3 = artifact(ArtifactKind::QualityReport, 3, 2, None);
        let body = |v: u64| {
            Blob::Body(qbody(
                Some(0.9),
                None,
                Some(("p", v)),
                Some(vec![("gates.same", Some(1.0))]),
            ))
        };
        let read = reader(vec![
            (a1.id, body(12)),
            (a2.id, body(13)),
            (a3.id, body(13)),
        ]);
        let view = compute(new_id(), &[a1, a2, a3], &[], &read);
        let s = stage(&view, STAGE_QUALITY);
        assert_eq!(s.series[1].profile.as_deref(), Some("p@13"));
        assert!(!s.series[1].comparable);
        assert_eq!(s.series[1].delta_score_milli, None);
        assert_eq!(s.series[1].closed_count, None);
        assert_eq!(s.series[1].distance, Some(1));
        assert_eq!(
            s.series[1].mode_note,
            Some(note::PROFILE_CHANGED),
            "ratified OQ #3: a profile change also breaks the STUCK series"
        );
        assert_eq!(
            s.series[2].modes,
            Vec::<&str>::new(),
            "12,13,13 is not three rounds inside one segment (ratified decision on OQ #3)"
        );
        assert_eq!(s.series[2].delta_score_milli, Some(0));
    }

    #[test]
    fn stuck_still_fires_inside_one_segment() {
        let a1 = artifact(ArtifactKind::QualityReport, 1, 0, None);
        let a2 = artifact(ArtifactKind::QualityReport, 2, 1, None);
        let a3 = artifact(ArtifactKind::QualityReport, 3, 2, None);
        let body = Blob::Body(qbody(
            Some(0.9),
            None,
            Some(("p", 13)),
            Some(vec![("gates.same", Some(1.0))]),
        ));
        let read = reader(vec![
            (a1.id, body.clone()),
            (a2.id, body.clone()),
            (a3.id, body),
        ]);
        let view = compute(new_id(), &[a1, a2, a3], &[], &read);
        assert!(
            stage(&view, STAGE_QUALITY).series[2]
                .modes
                .contains(&"STUCK")
        );
    }

    #[test]
    fn a_body_without_profile_keeps_its_score_and_says_unknown() {
        let a1 = artifact(ArtifactKind::QualityReport, 1, 0, None);
        let a2 = artifact(ArtifactKind::QualityReport, 2, 1, None);
        let read = reader(vec![
            (
                a1.id,
                Blob::Body(qbody(Some(0.8), None, Some(("p", 1)), one_check(1.0))),
            ),
            (
                a2.id,
                Blob::Body(qbody(Some(0.9), None, None, one_check(1.0))),
            ),
        ]);
        let view = compute(new_id(), &[a1, a2], &[], &read);
        let p = &stage(&view, STAGE_QUALITY).series[1];
        assert!(p.measured);
        assert_eq!(p.profile, None);
        assert_eq!(p.score_milli, Some(900));
        assert_eq!(p.mode_note, Some(note::PROFILE_UNKNOWN));
    }

    // ---- REQ-CV-04/07: the spec series ----------------------------------

    struct SpecRound {
        requirements: Option<usize>,
        oq: usize,
        body: Blob,
    }

    fn spec_round(verdict: &str, req: usize, oq: usize) -> SpecRound {
        let mut doc = json!({
            "summary": "review",
            "human_paragraph": "абзац",
            "verdict": verdict,
            "findings": [],
        });
        if verdict.is_empty() {
            doc.as_object_mut().unwrap().remove("verdict");
        }
        SpecRound {
            requirements: Some(req),
            oq,
            body: Blob::Body(doc.to_string()),
        }
    }

    /// One spec version + one review round per item, each bound through a
    /// stored execution row (the ONLY binding REQ-CV-04 allows). Every
    /// review gets its OWN spec version, so `requirements`/`open_questions`
    /// follow the round the review was actually made against.
    fn spec_series(rounds: &[SpecRound]) -> (ConvergenceView, Vec<Artifact>) {
        let (view, arts, _) = spec_series_with_execs(rounds);
        (view, arts)
    }

    fn spec_series_with_execs(
        rounds: &[SpecRound],
    ) -> (ConvergenceView, Vec<Artifact>, Vec<RoleExecution>) {
        let mut arts = Vec::new();
        let mut execs = Vec::new();
        let mut blobs: Vec<(Id, Blob)> = Vec::new();
        for (i, r) in rounds.iter().enumerate() {
            let spec = artifact(
                ArtifactKind::RequirementsSpec,
                i as u32 + 1,
                i as i64 * 2,
                None,
            );
            if let Some(req) = r.requirements {
                blobs.push((
                    spec.id,
                    Blob::Body(
                        json!({
                            "requirements": vec![json!({}); req],
                            "open_questions": vec!["q"; r.oq],
                        })
                        .to_string(),
                    ),
                ));
            }
            let exec = execution(new_id(), &[spec.id], None);
            let a = artifact(
                ArtifactKind::ReviewReport,
                i as u32 + 1,
                i as i64 * 2 + 1,
                Some(exec.id),
            );
            blobs.push((a.id, r.body.clone()));
            arts.push(spec);
            arts.push(a);
            execs.push(exec);
        }
        let read = reader(blobs);
        let view = compute(new_id(), &arts, &execs, &read);
        (view, arts, execs)
    }

    #[test]
    fn spec_volume_and_deltas_are_counted_from_the_bound_spec() {
        let mut rounds = vec![
            spec_round("NEEDS_REVISION", 14, 5),
            spec_round("NEEDS_REVISION", 22, 2),
        ];
        rounds[1].requirements = Some(22);
        let (view, _) = spec_series(&rounds);
        let s = stage(&view, STAGE_SPEC_REVIEW);
        assert_eq!(s.series[0].requirements_count, Some(14));
        assert_eq!(s.series[0].oq_count, Some(5));
        assert_eq!(s.series[0].mode_note, Some(note::NO_PREDECESSOR));
        assert_eq!(s.series[1].delta_requirements, Some(8));
        assert_eq!(s.series[1].delta_oq, Some(-3));
        assert_eq!(
            s.series[1].modes,
            vec!["DIVERGING"],
            "volume grew without APPROVED: divergence, not convergence"
        );
        assert!(!s.series[1].stuck_applicable);
        assert!(s.series.iter().all(|p| p.modes.len() <= 1));
    }

    #[test]
    fn spec_converges_on_a_closed_question_with_a_flat_volume() {
        let (view, _) = spec_series(&[
            spec_round("NEEDS_REVISION", 14, 5),
            spec_round("NEEDS_REVISION", 14, 2),
        ]);
        let p = &stage(&view, STAGE_SPEC_REVIEW).series[1];
        assert_eq!(p.delta_requirements, Some(0));
        assert_eq!(p.delta_oq, Some(-3));
        assert_eq!(p.modes, vec!["CONVERGING"]);
    }

    #[test]
    fn approved_does_not_suppress_plateau_and_suppresses_only_divergence() {
        let (view, _) = spec_series(&[
            spec_round("NEEDS_REVISION", 14, 5),
            spec_round("APPROVED", 14, 5),
            spec_round("APPROVED", 14, 5),
        ]);
        let s = stage(&view, STAGE_SPEC_REVIEW);
        assert_eq!(s.series[1].mode_note, Some(note::PLATEAU_PENDING));
        assert_eq!(s.series[2].modes, vec!["PLATEAU"]);
        assert_eq!(s.series[2].mode_note, None);

        let (view, _) = spec_series(&[
            spec_round("NEEDS_REVISION", 14, 5),
            spec_round("APPROVED", 22, 5),
        ]);
        let p = &stage(&view, STAGE_SPEC_REVIEW).series[1];
        assert_eq!(p.modes, Vec::<&str>::new());
        assert_eq!(p.mode_note, Some(note::GROWTH_WITH_APPROVED));
    }

    #[test]
    fn shrinking_without_a_closed_question_is_its_own_reason() {
        let (view, _) = spec_series(&[
            spec_round("NEEDS_REVISION", 22, 0),
            spec_round("NEEDS_REVISION", 20, 0),
        ]);
        let p = &stage(&view, STAGE_SPEC_REVIEW).series[1];
        assert_eq!(p.modes, Vec::<&str>::new());
        assert_eq!(p.mode_note, Some(note::SHRINKING_WITHOUT_CLOSED_ITEMS));
    }

    #[test]
    fn a_spec_reason_never_becomes_a_zero_volume() {
        let broken = SpecRound {
            requirements: None,
            oq: 0,
            body: Blob::Absent,
        };
        let (view, _) = spec_series(&[
            spec_round("NEEDS_REVISION", 14, 5),
            broken,
            spec_round("NEEDS_REVISION", 14, 2),
        ]);
        let s = stage(&view, STAGE_SPEC_REVIEW);
        assert!(!s.series[1].measured);
        assert_eq!(s.series[1].missing, vec![missing::REPORT_BODY_ABSENT]);
        assert_eq!(s.series[1].requirements_count, None);
        assert_eq!(s.series[1].mode_note, Some(note::INSUFFICIENT_DATA));
        // The measured neighbour keeps its own comparison, over a distance.
        assert_eq!(s.series[2].distance, Some(2));
        assert_eq!(
            s.series[2].modes,
            vec!["CONVERGING"],
            "counting progress does not need an adjacent round"
        );
    }

    #[test]
    fn spec_without_a_resolvable_spec_or_verdict_is_unmeasured() {
        // A review whose execution inputs name no spec.
        let review = artifact(ArtifactKind::ReviewReport, 1, 0, None);
        let read = reader(vec![(review.id, spec_round("APPROVED", 14, 5).body)]);
        let view = compute(new_id(), &[review], &[], &read);
        let s = stage(&view, STAGE_SPEC_REVIEW);
        assert!(!s.series[0].measured);
        assert!(s.series[0].missing.contains(&missing::SPEC_UNBOUND));

        // A review with a valid spec but no verdict token.
        let mut no_verdict = spec_round("APPROVED", 14, 5);
        no_verdict.body = Blob::Body(json!({ "summary": "x", "findings": [] }).to_string());
        let (view, _) = spec_series(&[no_verdict]);
        assert_eq!(
            stage(&view, STAGE_SPEC_REVIEW).series[0].missing,
            vec![missing::VERDICT_ABSENT]
        );
    }

    #[test]
    fn spec_versions_without_review_are_a_counter_not_rounds() {
        let (_initial, mut arts, execs) =
            spec_series_with_execs(&[spec_round("NEEDS_REVISION", 14, 5)]);
        // Two more spec versions nobody reviewed, plus a review bound to the
        // first spec: the counter counts versions, the series counts rounds.
        arts.push(artifact(ArtifactKind::RequirementsSpec, 2, 5, None));
        arts.push(artifact(ArtifactKind::RequirementsSpec, 3, 6, None));
        let spec_body =
            json!({ "requirements": vec![json!({}); 14], "open_questions": vec!["q"; 5] })
                .to_string();
        let read = reader(vec![(arts[0].id, Blob::Body(spec_body))]);
        let view = compute(new_id(), &arts, &execs, &read);
        let s = stage(&view, STAGE_SPEC_REVIEW);
        assert_eq!(s.series.len(), 1, "a spec version is not a round");
        assert_eq!(s.counters.spec_versions_without_review, 2);
    }

    // ---- REQ-CV-08: round and accumulated time --------------------------

    #[test]
    fn round_time_is_finished_minus_started_and_sums() {
        let e1 = execution(new_id(), &[], Some(100));
        let e2 = execution(new_id(), &[], Some(50));
        let a1 = artifact(ArtifactKind::QualityReport, 1, 0, Some(e1.id));
        let a2 = artifact(ArtifactKind::QualityReport, 2, 1, Some(e2.id));
        let read = reader(vec![
            (
                a1.id,
                Blob::Body(qbody(Some(0.9), None, Some(("p", 1)), one_check(1.0))),
            ),
            (
                a2.id,
                Blob::Body(qbody(Some(0.92), None, Some(("p", 1)), one_check(1.0))),
            ),
        ]);
        let view = compute(new_id(), &[a1, a2], &[e1, e2], &read);
        let s = stage(&view, STAGE_QUALITY);
        assert_eq!(s.series[0].time_seconds, Some(100));
        assert_eq!(s.series[1].time_seconds, Some(50));
        assert_eq!(s.stage_total_seconds, 150);
        assert_eq!(s.counters.rounds_without_time, 0);
    }

    #[test]
    fn an_unfinished_execution_gives_no_time_and_halves_the_coverage() {
        let e1 = execution(new_id(), &[], Some(100));
        let e2 = execution(new_id(), &[], None);
        let a1 = artifact(ArtifactKind::QualityReport, 1, 0, Some(e1.id));
        let a2 = artifact(ArtifactKind::QualityReport, 2, 1, Some(e2.id));
        let read = reader(vec![
            (
                a1.id,
                Blob::Body(qbody(Some(0.9), None, Some(("p", 1)), one_check(1.0))),
            ),
            (
                a2.id,
                Blob::Body(qbody(Some(0.92), None, Some(("p", 1)), one_check(1.0))),
            ),
        ]);
        let view = compute(new_id(), &[a1, a2], &[e1, e2], &read);
        let s = stage(&view, STAGE_QUALITY);
        assert_eq!(s.series[1].time_seconds, None);
        assert!(s.series[1].missing.contains(&missing::TIME_ABSENT));
        assert!(s.series[1].measured, "time is not a measurement");
        assert_eq!(s.stage_total_seconds, 100);
        assert_eq!(s.counters.rounds_without_time, 1, "coverage 1/2");
    }

    // ---- REQ-CV-02/05/09: order, determinism, no rounds -----------------

    #[test]
    fn rounds_are_ordered_by_seq_then_created_at_then_id() {
        let a_late = {
            let mut a = artifact(ArtifactKind::QualityReport, 1, 10, None);
            a.name = "late".into();
            a
        };
        let a_early = {
            let mut a = artifact(ArtifactKind::QualityReport, 1, 5, None);
            a.name = "early".into();
            a
        };
        let body = |s: f64| Blob::Body(qbody(Some(s), None, Some(("p", 1)), one_check(1.0)));
        let read = reader(vec![(a_late.id, body(0.9)), (a_early.id, body(0.8))]);
        // Deliberately handed in the "wrong" order: the slice sorts itself.
        let view = compute(new_id(), &[a_late, a_early], &[], &read);
        let s = stage(&view, STAGE_QUALITY);
        assert_eq!(s.series[0].seq, 1);
        assert_eq!(s.series[0].round, 1);
        assert_eq!(s.series[0].score_milli, Some(800), "created_at decides");
        assert_eq!(s.series[1].round, 2);
    }

    #[test]
    fn two_reads_return_the_same_series() {
        let (first, arts) = quality(&[(0.94, "p", 1), (0.99, "p", 1)], one_check(1.0));
        let read = reader(vec![
            (
                arts[0].id,
                Blob::Body(qbody(Some(0.94), None, Some(("p", 1)), one_check(1.0))),
            ),
            (
                arts[1].id,
                Blob::Body(qbody(Some(0.99), None, Some(("p", 1)), one_check(1.0))),
            ),
        ]);
        let second = compute(new_id(), &arts, &[], &read);
        assert_eq!(
            serde_json::to_string(&first.stages).unwrap(),
            serde_json::to_string(&second.stages).unwrap(),
            "only snapshot_at may differ between two reads of one store"
        );
    }

    // ---- REQ-CV-05/13: the closed vocabularies --------------------------

    #[test]
    fn mode_note_and_missing_vocabularies_are_closed() {
        let modes = [
            mode::CONVERGING,
            mode::DIVERGING,
            mode::PLATEAU,
            mode::STUCK,
        ];
        let notes = [
            note::INSUFFICIENT_DATA,
            note::NO_PREDECESSOR,
            note::PROFILE_CHANGED,
            note::PROFILE_UNKNOWN,
            note::NONADJACENT_PREDECESSOR,
            note::PLATEAU_PENDING,
            note::PROGRESS_WITHOUT_CLOSURE,
            note::SHRINKING_WITHOUT_CLOSED_ITEMS,
            note::GROWTH_WITH_APPROVED,
        ];
        assert_eq!(modes.len(), 4, "four modes, no more");
        assert_eq!(notes.len(), 9, "nine reasons, no more");
        assert_eq!(MISSING_ORDER.len(), 11, "eleven data gaps, no more");

        // A series that exercises every branch: the emitted tokens must stay
        // inside the vocabularies (a new literal in the computation would
        // fail here before it reaches a consumer).
        let (view, _) = quality(
            &[
                (0.90, "p", 1),
                (0.905, "p", 1),
                (0.95, "p", 1),
                (0.94, "p", 1),
            ],
            one_check(1.0),
        );
        for p in &stage(&view, STAGE_QUALITY).series {
            for m in &p.modes {
                assert!(modes.contains(m), "{m} is not a mode token");
            }
            if let Some(n) = p.mode_note {
                assert!(notes.contains(&n), "{n} is not a note token");
            }
            for r in &p.missing {
                assert!(MISSING_ORDER.contains(r), "{r} is not a missing token");
            }
        }
        let (view, _) = spec_series(&[
            spec_round("NEEDS_REVISION", 14, 5),
            spec_round("APPROVED", 14, 5),
        ]);
        for p in &stage(&view, STAGE_SPEC_REVIEW).series {
            for m in &p.modes {
                assert!(modes.contains(m));
            }
            if let Some(n) = p.mode_note {
                assert!(notes.contains(&n));
            }
        }
    }

    #[test]
    fn no_carriers_is_no_rounds_without_an_error() {
        let read = reader(vec![]);
        let view = compute(new_id(), &[], &[], &read);
        assert!(view.stages.is_empty());
        assert_eq!(view.reason, Some("no_rounds"));
        assert_eq!(view.gap_milli, 10);
    }

    #[test]
    fn stages_are_ordered_spec_then_quality_and_the_fact_quotes_the_last_round() {
        let (view, _) = quality(&[(0.90, "p", 1), (0.95, "p", 1)], one_check(1.0));
        let fact = ConvergenceFact::of_stage(&stage(&view, STAGE_QUALITY));
        assert_eq!(fact.stage, STAGE_QUALITY);
        assert_eq!(fact.rounds, 2);
        assert_eq!(fact.series.len(), 2);
        assert_eq!(fact.series[1].score_milli, Some(950));
        assert_eq!(fact.mode_note, Some(note::PROGRESS_WITHOUT_CLOSURE));
        assert_eq!(fact.gap_milli, 10);
    }

    #[test]
    fn only_the_two_gates_carry_a_stage() {
        assert_eq!(
            stage_for_state(TaskState::Approval),
            Some(STAGE_SPEC_REVIEW)
        );
        assert_eq!(stage_for_state(TaskState::Qa), Some(STAGE_QUALITY));
        for s in [
            TaskState::WaitingForInput,
            TaskState::Paused,
            TaskState::Blocked,
            TaskState::Failed,
            TaskState::Draft,
        ] {
            assert_eq!(stage_for_state(s), None, "{s:?} has no chart");
        }
    }
}
