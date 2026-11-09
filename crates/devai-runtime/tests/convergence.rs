//! Behaviour pins of the convergence slice (task 01a1b5be,
//! REQ-CV-01..16): the arithmetic, the closed vocabularies and the round
//! structure, all against injected bodies — no store, no clock, no model.

use std::collections::HashMap;

use chrono::{DateTime, TimeZone, Utc};
use devai_core::types::{
    Artifact, ArtifactKind, ArtifactStatus, DataCategory, Id, RoleExecution, new_id,
};
use devai_runtime::convergence::{
    Body, BodyCache, ConvergencePoint, ConvergenceView, GAP_MILLI, MISSING_TOKENS, MissingEntry,
    STAGE_QUALITY, STAGE_SPEC_REVIEW, compute, mode, reason, rule, token, volume,
};
use devai_store::StoreError;
use serde_json::{Map, Value, json};

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

fn execution(id: Id, inputs: &[Id], started: i64, finished: Option<i64>) -> RoleExecution {
    RoleExecution {
        id,
        task_id: new_id(),
        role_slug: "reviewer".into(),
        role_version: 1,
        agent_name: Some("mock".into()),
        status: if finished.is_some() {
            devai_core::types::ExecutionStatus::Completed
        } else {
            devai_core::types::ExecutionStatus::Running
        },
        inputs: inputs.iter().map(|i| i.to_string()).collect(),
        summary: None,
        error: None,
        started_at: ts(started),
        finished_at: finished.map(ts),
        workspace_revision: None,
        attempt: 1,
        parent_execution: None,
        failure_class: None,
        owner_pid: None,
        owner_host: None,
        owner_process_start: None,
        // Связка моделей (задача 01a1b71f) добавила штамп прогона; тест
        // сходимости его не использует — держим пустым явно.
        bundle: None,
    }
}

fn object(v: Value) -> Body {
    match v {
        Value::Object(map) => Body::Json(map),
        _ => panic!("fixture must be an object"),
    }
}

/// A quality body: score/checks/theta/profile exactly as stored by the
/// platform (`evaluation.score` is already rounded to three decimals).
fn quality_body(
    score: Option<f64>,
    checks: &[(&str, Option<f64>)],
    theta: Option<f64>,
    profile: Option<(&str, u64)>,
) -> Body {
    let checks: Vec<Value> = checks
        .iter()
        .map(|(id, s)| match s {
            Some(x) => json!({ "id": id, "score": x }),
            None => json!({ "id": id }),
        })
        .collect();
    let mut evaluation = Map::new();
    if let Some(s) = score {
        evaluation.insert("score".into(), json!(s));
    }
    evaluation.insert("checks".into(), json!(checks));
    if let Some(t) = theta {
        evaluation.insert("theta".into(), json!(t));
    }
    let mut body = Map::new();
    body.insert("evaluation".into(), Value::Object(evaluation));
    if let Some((project, version)) = profile {
        body.insert(
            "profile".into(),
            json!({ "project": project, "version": version }),
        );
    }
    Body::Json(body)
}

fn spec_body(requirements: usize, oq: usize) -> Body {
    object(json!({
        "requirements": vec![json!({"id": "R"}); requirements],
        "open_questions": vec![json!({"id": "Q"}); oq],
    }))
}

type Checks<'a> = Vec<(&'a str, Option<f64>)>;

fn quality_series(rounds: &[(f64, Checks<'_>)]) -> ConvergenceView {
    let mut bodies = HashMap::new();
    let mut artifacts = Vec::new();
    for (i, (score, checks)) in rounds.iter().enumerate() {
        let a = artifact(ArtifactKind::QualityReport, i as u32 + 1, i as i64, None);
        bodies.insert(
            a.id,
            quality_body(Some(*score), checks, Some(0.8), Some(("home", 12))),
        );
        artifacts.push(a);
    }
    let cache = BodyCache::from_bodies(bodies);
    compute(&artifacts, &[], &cache)
}

fn stage<'a>(
    view: &'a ConvergenceView,
    name: &str,
) -> &'a devai_runtime::convergence::StageConvergence {
    view.stage(name).expect("stage present")
}

fn point(stage: &devai_runtime::convergence::StageConvergence, i: usize) -> &ConvergencePoint {
    &stage.series[i]
}

fn assert_one_token(p: &ConvergencePoint, field: &str, expected: &str) {
    let hits: Vec<&MissingEntry> = p.missing.iter().filter(|m| m.field == field).collect();
    assert_eq!(hits.len(), 1, "field {field} must carry exactly one token");
    assert_eq!(hits[0].token, expected, "field {field}");
}

// ---------------------------------------------------------------------------
// REQ-CV-03: the quality measurement and its counters
// ---------------------------------------------------------------------------

#[test]
fn score_is_quoted_in_thousandths_without_smoothing_or_second_rounding() {
    let view = quality_series(&[
        (0.943, vec![("a", Some(0.5))]),
        (0.997, vec![("a", Some(1.0))]),
        (0.991, vec![("b", Some(0.9))]),
        (1.0, vec![("c", Some(1.0))]),
    ]);
    let s = stage(&view, STAGE_QUALITY);
    assert_eq!(
        s.series.iter().map(|p| p.score_milli).collect::<Vec<_>>(),
        vec![Some(943), Some(997), Some(991), Some(1000)]
    );
    assert_eq!(
        s.series
            .iter()
            .map(|p| p.delta_score_milli)
            .collect::<Vec<_>>(),
        vec![None, Some(54), Some(-6), Some(9)]
    );
    assert_eq!(view.gap_milli, GAP_MILLI);
}

#[test]
fn a_recorded_zero_is_a_measurement_and_null_is_not() {
    let mut bodies = HashMap::new();
    let zero = artifact(ArtifactKind::QualityReport, 1, 0, None);
    bodies.insert(
        zero.id,
        quality_body(
            Some(0.0),
            &[("a", Some(1.0))],
            Some(0.8),
            Some(("home", 12)),
        ),
    );
    let null_score = artifact(ArtifactKind::QualityReport, 2, 1, None);
    bodies.insert(
        null_score.id,
        object(json!({"evaluation": {"score": null, "checks": [], "theta": 0.8}})),
    );
    let str_score = artifact(ArtifactKind::QualityReport, 3, 2, None);
    bodies.insert(
        str_score.id,
        object(json!({"evaluation": {"score": "0.94", "checks": []}})),
    );
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&[zero, null_score, str_score], &[], &cache);
    let s = stage(&view, STAGE_QUALITY);
    assert!(s.series[0].measured);
    assert_eq!(s.series[0].score_milli, Some(0));
    assert_eq!(s.series[0].low_count, Some(0));
    for p in &s.series[1..] {
        assert!(!p.measured, "null and non-numeric scores are unmeasured");
        assert_eq!(p.score_milli, None);
        assert_one_token(p, "score_milli", token::QUALITY_REPORT);
        assert_one_token(p, "low_count", token::QUALITY_REPORT);
        assert_one_token(p, "unscored_count", token::QUALITY_REPORT);
        assert_one_token(p, "invalid_count", token::QUALITY_REPORT);
        assert_one_token(p, "pending_count", token::QUALITY_REPORT);
    }
}

#[test]
fn an_empty_checks_array_is_measured_with_zero_low_items() {
    let view = quality_series(&[(0.94, vec![])]);
    let p = point(stage(&view, STAGE_QUALITY), 0);
    assert!(p.measured);
    assert_eq!(p.low_count, Some(0));
    assert_eq!(p.unscored_count, Some(0));
    assert_eq!(p.invalid_count, Some(0));
    assert_eq!(p.pending_count, Some(0));
}

#[test]
fn low_unscored_invalid_and_pending_come_from_the_checks_array_only() {
    // REQ-CV-03 AC1: 1.0 (pass), 0.0 and 0.8 (low), a block with no score
    // (unscored), "0.7" (invalid), nothing pending → low=2, unscored=1,
    // invalid=1, pending=0.
    let a = artifact(ArtifactKind::QualityReport, 1, 0, None);
    let mut bodies = HashMap::new();
    bodies.insert(
        a.id,
        object(json!({
            "evaluation": {
                "score": 0.7,
                "theta": 0.8,
                "checks": [
                    {"id": "pass", "state": "SCORED", "score": 1.0},
                    {"id": "zero", "state": "MECHANICAL_FAIL", "score": 0.0},
                    {"id": "eight", "state": "SCORED", "score": 0.8},
                    {"id": "block", "state": "BLOCKED"},
                    {"id": "bad", "state": "SCORED", "score": "0.7"},
                    {"id": "human", "state": "PENDING_HUMAN"},
                ]
            },
            "profile": {"project": "home", "version": 12},
        })),
    );
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&[a], &[], &cache);
    let p = point(stage(&view, STAGE_QUALITY), 0);
    assert_eq!(p.low_count, Some(2), "0.0 and 0.8 are below 1.0");
    assert_eq!(p.unscored_count, Some(1), "the block has no score");
    assert_eq!(p.invalid_count, Some(1), "\"0.7\" is not a number");
    assert_eq!(p.pending_count, Some(1), "the human signature is pending");
}

#[test]
fn a_missing_theta_keeps_the_point_measured_with_its_own_token() {
    let mut bodies = HashMap::new();
    let a = artifact(ArtifactKind::QualityReport, 1, 0, None);
    bodies.insert(
        a.id,
        object(json!({
            "evaluation": {"score": 0.94, "checks": []},
            "profile": {"project": "p", "version": 1},
        })),
    );
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&[a], &[], &cache);
    let p = point(stage(&view, STAGE_QUALITY), 0);
    assert!(p.measured);
    assert_eq!(p.theta_milli, None);
    assert_one_token(p, "theta_milli", token::QUALITY_THETA);
}

#[test]
fn a_body_without_score_and_checks_is_a_gap_not_a_zero() {
    let mut bodies = HashMap::new();
    let a = artifact(ArtifactKind::QualityReport, 1, 0, None);
    bodies.insert(a.id, object(json!({"verdict": "FAIL"})));
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&[a], &[], &cache);
    let p = point(stage(&view, STAGE_QUALITY), 0);
    assert!(!p.measured);
    assert_eq!(p.score_milli, None);
    assert_eq!(p.low_count, None);
    assert_eq!(p.unscored_count, None);
    assert_one_token(p, "score_milli", token::QUALITY_REPORT);
    assert_one_token(p, "time_seconds", token::TIME);
    assert_one_token(p, "profile", token::PROFILE);
    assert_one_token(p, "closed_count", token::CURRENT_UNMEASURED);
    assert_one_token(p, "delta_score_milli", token::CURRENT_UNMEASURED);
}

// ---------------------------------------------------------------------------
// REQ-CV-06: deltas and closed items
// ---------------------------------------------------------------------------

#[test]
fn closed_count_sees_below_one_and_unscored_turning_clean() {
    let mut bodies = HashMap::new();
    let first = artifact(ArtifactKind::QualityReport, 1, 0, None);
    bodies.insert(
        first.id,
        quality_body(
            Some(0.8),
            &[
                ("a", Some(0.5)),
                ("b", None),
                ("c", Some(1.0)),
                ("gone", Some(0.2)),
            ],
            Some(0.8),
            Some(("home", 12)),
        ),
    );
    let second = artifact(ArtifactKind::QualityReport, 2, 1, None);
    bodies.insert(
        second.id,
        quality_body(
            Some(0.9),
            &[
                ("a", Some(1.0)),
                ("b", Some(1.0)),
                ("c", None),
                ("new", Some(1.0)),
            ],
            Some(0.8),
            Some(("home", 12)),
        ),
    );
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&[first, second], &[], &cache);
    let s = stage(&view, STAGE_QUALITY);
    assert_eq!(s.series[1].closed_count, Some(2), "a and b closed");
    assert_eq!(s.series[1].low_count, Some(0));
}

#[test]
fn an_unmeasured_round_breaks_comparison_and_carries_predecessor_unmeasured() {
    let mut bodies = HashMap::new();
    let a1 = artifact(ArtifactKind::QualityReport, 1, 0, None);
    bodies.insert(
        a1.id,
        quality_body(
            Some(0.9),
            &[("a", Some(0.9))],
            Some(0.8),
            Some(("home", 12)),
        ),
    );
    let a2 = artifact(ArtifactKind::QualityReport, 2, 1, None);
    // no body at all -> absent, still a round
    let a3 = artifact(ArtifactKind::QualityReport, 3, 2, None);
    bodies.insert(
        a3.id,
        quality_body(
            Some(1.0),
            &[("a", Some(1.0))],
            Some(0.8),
            Some(("home", 12)),
        ),
    );
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&[a1, a2, a3], &[], &cache);
    let s = stage(&view, STAGE_QUALITY);
    assert!(!s.series[1].measured);
    assert_one_token(&s.series[1], "delta_score_milli", token::CURRENT_UNMEASURED);
    let third = &s.series[2];
    assert!(third.measured);
    assert!(!third.comparable);
    assert_eq!(third.delta_score_milli, None);
    assert_one_token(third, "delta_score_milli", token::PREDECESSOR_UNMEASURED);
    assert_one_token(third, "closed_count", token::PREDECESSOR_UNMEASURED);
}

#[test]
fn a_profile_change_breaks_the_segment_and_says_so() {
    let mut bodies = HashMap::new();
    let a1 = artifact(ArtifactKind::QualityReport, 1, 0, None);
    bodies.insert(
        a1.id,
        quality_body(
            Some(0.9),
            &[("a", Some(0.9))],
            Some(0.8),
            Some(("home", 12)),
        ),
    );
    let a2 = artifact(ArtifactKind::QualityReport, 2, 1, None);
    bodies.insert(
        a2.id,
        quality_body(
            Some(1.0),
            &[("a", Some(1.0))],
            Some(0.8),
            Some(("home", 13)),
        ),
    );
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&[a1, a2], &[], &cache);
    let p = point(stage(&view, STAGE_QUALITY), 1);
    assert!(!p.comparable);
    assert_one_token(p, "delta_score_milli", token::SEGMENT_BOUNDARY);
    assert_eq!(p.mode, mode::UNKNOWN);
    assert_eq!(p.mode_basis.reason, Some(reason::INSUFFICIENT_DATA));
}

#[test]
fn an_unknown_profile_breaks_the_segment_too() {
    let mut bodies = HashMap::new();
    let a1 = artifact(ArtifactKind::QualityReport, 1, 0, None);
    bodies.insert(
        a1.id,
        quality_body(
            Some(0.9),
            &[("a", Some(0.9))],
            Some(0.8),
            Some(("home", 12)),
        ),
    );
    let a2 = artifact(ArtifactKind::QualityReport, 2, 1, None);
    bodies.insert(
        a2.id,
        quality_body(Some(1.0), &[("a", Some(1.0))], Some(0.8), None),
    );
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&[a1, a2], &[], &cache);
    let p = point(stage(&view, STAGE_QUALITY), 1);
    assert!(!p.comparable);
    assert_one_token(p, "delta_score_milli", token::SEGMENT_BOUNDARY);
    assert_one_token(p, "profile", token::PROFILE);
}

// ---------------------------------------------------------------------------
// REQ-CV-07/09: quality modes
// ---------------------------------------------------------------------------

#[test]
fn converging_needs_only_the_gap_and_not_a_closed_item() {
    // REQ-CV-07 / ratified 08.11: +10 or more converges whether or not an
    // item closed.
    let view = quality_series(&[
        (0.94, vec![("a", Some(0.6))]),
        (0.98, vec![("a", Some(1.0))]),
    ]);
    let s = stage(&view, STAGE_QUALITY);
    assert_eq!(s.series[0].mode, mode::UNKNOWN);
    assert_eq!(
        s.series[0].mode_basis.reason,
        Some(reason::INSUFFICIENT_DATA)
    );
    assert_eq!(s.series[1].mode, mode::CONVERGING);
    assert_eq!(s.series[1].mode_basis.rule, rule::CONVERGING);
    assert_eq!(s.series[1].mode_basis.rounds, vec![1, 2]);

    // +9 milli is below the gap and therefore a plateau, not convergence.
    let view = quality_series(&[
        (0.9, vec![("a", Some(0.6))]),
        (0.909, vec![("a", Some(1.0))]),
    ]);
    let s = stage(&view, STAGE_QUALITY);
    assert_eq!(s.series[1].mode, mode::PLATEAU);
}

#[test]
fn the_gap_boundary_is_exact_in_thousandths() {
    let view = quality_series(&[
        (0.9, vec![("a", Some(0.2))]),
        (0.91, vec![("a", Some(1.0))]),
    ]);
    assert_eq!(point(stage(&view, STAGE_QUALITY), 1).mode, mode::CONVERGING);
}

#[test]
fn progress_without_a_closed_item_still_converges() {
    // REQ-CV-07 / ratified 08.11: closure is a counter, never a gate.
    let view = quality_series(&[
        (0.9, vec![("a", Some(1.0))]),
        (0.95, vec![("a", Some(1.0))]),
    ]);
    let p = point(stage(&view, STAGE_QUALITY), 1);
    assert_eq!(p.closed_count, Some(0));
    assert_eq!(p.mode, mode::CONVERGING);
    assert_eq!(p.mode_basis.rule, rule::CONVERGING);
}

#[test]
fn diverging_and_plateau_follow_the_measured_curve() {
    let view = quality_series(&[
        (0.943, vec![("a", Some(0.9))]),
        (0.997, vec![("b", Some(0.9))]),
        (0.991, vec![("c", Some(0.9))]),
        (1.0, vec![("d", Some(0.9))]),
    ]);
    let s = stage(&view, STAGE_QUALITY);
    assert_eq!(s.series[1].mode, mode::CONVERGING);
    assert_eq!(s.series[2].mode, mode::DIVERGING);
    assert_eq!(s.series[2].mode_basis.rounds, vec![2, 3]);
    assert_eq!(s.series[3].mode, mode::PLATEAU);
    assert_eq!(s.series[3].mode_basis.rounds, vec![3, 4]);
}

#[test]
fn a_single_flat_step_is_a_plateau_from_the_first_round() {
    // REQ-CV-07 / ratified 08.11: no two-round warm-up.
    let view = quality_series(&[
        (0.94, vec![("a", Some(0.9))]),
        (0.94, vec![("b", Some(0.9))]),
    ]);
    let s = stage(&view, STAGE_QUALITY);
    assert_eq!(s.series[1].delta_score_milli, Some(0));
    assert_eq!(s.series[1].mode, mode::PLATEAU);
    assert_eq!(s.series[1].mode_basis.rounds, vec![1, 2]);
}

#[test]
fn stuck_is_one_check_id_in_three_adjacent_rounds_at_any_score() {
    for checks in [
        vec![("same", Some(1.0)), ("zzz", Some(0.9))],
        vec![("same", Some(0.6)), ("zzz", Some(0.9))],
        vec![("same", None), ("zzz", Some(0.9))],
    ] {
        let view = quality_series(&[
            (0.9, checks.clone()),
            (0.8, checks.clone()),
            (0.9, checks.clone()),
        ]);
        let s = stage(&view, STAGE_QUALITY);
        let p = &s.series[2];
        assert_eq!(p.mode, mode::STUCK, "score never enters the STUCK rule");
        assert_eq!(p.mode_basis.rule, rule::STUCK);
        assert_eq!(p.mode_basis.check_id.as_deref(), Some("same"));
        assert_eq!(p.mode_basis.rounds, vec![1, 2, 3]);
    }
}

#[test]
fn stuck_wins_over_a_negative_delta() {
    let view = quality_series(&[
        (0.9, vec![("same", Some(0.5))]),
        (0.85, vec![("same", Some(0.5))]),
        (0.7, vec![("same", Some(0.5))]),
    ]);
    let p = point(stage(&view, STAGE_QUALITY), 2);
    assert_eq!(p.delta_score_milli, Some(-150));
    assert_eq!(p.mode, mode::STUCK);
}

#[test]
fn a_check_absent_from_the_middle_round_breaks_the_stuck_chain() {
    let view = quality_series(&[
        (0.9, vec![("same", Some(0.9))]),
        (0.9, vec![("same", Some(0.9))]),
        (0.9, vec![("other", Some(0.9))]),
        (0.9, vec![("same", Some(0.9))]),
    ]);
    let s = stage(&view, STAGE_QUALITY);
    assert_ne!(s.series[3].mode, mode::STUCK);
}

#[test]
fn an_unmeasured_round_with_a_readable_check_list_does_not_break_stuck() {
    // REQ-CV-09 AC2: a round without evaluation.score is still a STUCK
    // member when its checklist is readable; the middle round does NOT get
    // insufficient_data.
    let mut bodies = HashMap::new();
    let a1 = artifact(ArtifactKind::QualityReport, 1, 0, None);
    let a2 = artifact(ArtifactKind::QualityReport, 2, 1, None);
    let a3 = artifact(ArtifactKind::QualityReport, 3, 2, None);
    bodies.insert(
        a1.id,
        object(json!({
            "evaluation": {"score": 1.0, "theta": 0.8,
                "checks": [{"id": "same", "state": "SCORED", "score": 1.0}]},
            "profile": {"project": "home", "version": 12},
        })),
    );
    bodies.insert(
        a2.id,
        object(json!({
            "evaluation": {"theta": 0.8,
                "checks": [{"id": "same", "state": "SCORED", "score": 1.0}]},
            "profile": {"project": "home", "version": 12},
        })),
    );
    bodies.insert(
        a3.id,
        object(json!({
            "evaluation": {"score": 1.0, "theta": 0.8,
                "checks": [{"id": "same", "state": "SCORED", "score": 1.0}]},
            "profile": {"project": "home", "version": 12},
        })),
    );
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&[a1, a2, a3], &[], &cache);
    let s = stage(&view, STAGE_QUALITY);
    assert!(!s.series[1].measured);
    for p in &s.series {
        assert_eq!(p.mode, mode::STUCK, "round {} is stuck", p.round);
        assert_eq!(p.mode_basis.reason, None, "STUCK has no reason");
        assert_eq!(p.mode_basis.check_id.as_deref(), Some("same"));
        assert_eq!(p.mode_basis.rounds, vec![1, 2, 3]);
    }
}

#[test]
fn an_unmeasured_round_without_a_check_list_breaks_the_stuck_chain() {
    // REQ-CV-07: a round whose body has no readable evaluation.checks is
    // not a member of the chain, even if its id list would have matched.
    let mut bodies = HashMap::new();
    let a1 = artifact(ArtifactKind::QualityReport, 1, 0, None);
    let a2 = artifact(ArtifactKind::QualityReport, 2, 1, None);
    let a3 = artifact(ArtifactKind::QualityReport, 3, 2, None);
    for a in [&a1, &a3] {
        bodies.insert(
            a.id,
            quality_body(
                Some(0.9),
                &[("same", Some(0.9))],
                Some(0.8),
                Some(("home", 12)),
            ),
        );
    }
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&[a1, a2, a3], &[], &cache);
    let s = stage(&view, STAGE_QUALITY);
    assert_ne!(s.series[2].mode, mode::STUCK);
    assert_eq!(
        s.series[2].mode_basis.reason,
        Some(reason::INSUFFICIENT_DATA)
    );
}

// ---------------------------------------------------------------------------
// REQ-CV-04/06/08: the spec stage
// ---------------------------------------------------------------------------

/// One review round per (verdict, spec counts): each round is bound to its
/// OWN spec version, exactly how a real cycle accumulates versions.
fn spec_fixture(rounds: &[(&str, usize, usize)]) -> ConvergenceView {
    let mut artifacts = Vec::new();
    let mut executions = Vec::new();
    let mut bodies = HashMap::new();
    for (i, (verdict, req, oq)) in rounds.iter().enumerate() {
        let spec = artifact(
            ArtifactKind::RequirementsSpec,
            i as u32 + 1,
            i as i64 - 10,
            None,
        );
        bodies.insert(spec.id, spec_body(*req, *oq));
        let exec_id = new_id();
        let review = artifact(
            ArtifactKind::ReviewReport,
            i as u32 + 1,
            i as i64,
            Some(exec_id),
        );
        executions.push(execution(
            exec_id,
            &[spec.id],
            i as i64,
            Some(i as i64 + 10),
        ));
        bodies.insert(review.id, object(json!({ "verdict": verdict })));
        artifacts.push(spec);
        artifacts.push(review);
    }
    let cache = BodyCache::from_bodies(bodies);
    compute(&artifacts, &executions, &cache)
}

#[test]
fn spec_volume_and_deltas_are_counted_from_the_bound_spec() {
    let view = spec_fixture(&[("NEEDS_REVISION", 14, 5), ("NEEDS_REVISION", 22, 2)]);
    let s = stage(&view, STAGE_SPEC_REVIEW);
    let second = &s.series[1];
    assert_eq!(second.requirements_count, Some(22));
    assert_eq!(second.oq_count, Some(2));
    assert_eq!(second.delta_requirements, Some(8));
    assert_eq!(second.delta_oq, Some(-3));
    assert_eq!(second.volume_direction, Some(volume::GROWING));
    assert_eq!(second.verdict.as_deref(), Some("NEEDS_REVISION"));
    assert_eq!(second.mode, mode::DIVERGING);
    assert_eq!(second.mode_basis.rounds, vec![1, 2]);
}

#[test]
fn shrinking_converges_and_approved_growth_has_no_predicate() {
    let view = spec_fixture(&[("NEEDS_REVISION", 14, 5), ("NEEDS_REVISION", 22, 2)]);
    assert_eq!(
        stage(&view, STAGE_SPEC_REVIEW).series[1].mode,
        mode::DIVERGING
    );

    let view = spec_fixture(&[("NEEDS_REVISION", 14, 5), ("APPROVED", 14, 2)]);
    let s = stage(&view, STAGE_SPEC_REVIEW);
    assert_eq!(s.series[1].volume_direction, Some(volume::SHRINKING));
    assert_eq!(s.series[1].mode, mode::CONVERGING);

    let view = spec_fixture(&[("NEEDS_REVISION", 14, 5), ("APPROVED", 22, 5)]);
    let p = point(stage(&view, STAGE_SPEC_REVIEW), 1);
    assert_eq!(p.volume_direction, Some(volume::GROWING));
    assert_eq!(p.mode, mode::UNKNOWN);
    assert_eq!(p.mode_basis.reason, Some(reason::NO_PREDICATE));
}

#[test]
fn spec_plateau_starts_at_the_first_flat_round_and_needs_no_verdict() {
    // REQ-CV-08 / ratified 08.11: flat is a plateau from the FIRST flat
    // round, whatever the verdict (APPROVED included).
    let view = spec_fixture(&[("NEEDS_REVISION", 14, 5), ("NEEDS_REVISION", 14, 5)]);
    let s = stage(&view, STAGE_SPEC_REVIEW);
    assert_eq!(s.series[1].volume_direction, Some(volume::FLAT));
    assert_eq!(s.series[1].mode, mode::PLATEAU);
    assert_eq!(s.series[1].mode_basis.rounds, vec![1, 2]);

    let view = spec_fixture(&[("APPROVED", 14, 5), ("APPROVED", 14, 5)]);
    let s = stage(&view, STAGE_SPEC_REVIEW);
    assert_eq!(
        s.series[1].mode,
        mode::PLATEAU,
        "an approved flat spec is still a plateau"
    );
}

#[test]
fn a_null_verdict_is_not_approved_and_keeps_the_point_measured() {
    let spec = artifact(ArtifactKind::RequirementsSpec, 1, -1, None);
    let mut artifacts = vec![spec.clone()];
    let mut executions = Vec::new();
    let mut bodies = HashMap::new();
    bodies.insert(spec.id, spec_body(14, 5));
    for i in 0..2 {
        let exec_id = new_id();
        let review = artifact(ArtifactKind::ReviewReport, i + 1, i as i64, Some(exec_id));
        executions.push(execution(exec_id, &[spec.id], i as i64, Some(i as i64 + 5)));
        bodies.insert(review.id, object(json!({ "note": "verdict missing" })));
        artifacts.push(review);
    }
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&artifacts, &executions, &cache);
    let p = point(stage(&view, STAGE_SPEC_REVIEW), 1);
    assert!(p.measured);
    assert_eq!(p.verdict, None);
    assert_one_token(p, "verdict", token::REVIEW_VERDICT);
    // A null verdict is not APPROVED; with flat volume the round is a
    // PLATEAU (REQ-CV-08), not an unfired predicate.
    assert_eq!(p.volume_direction, Some(volume::FLAT));
    assert_eq!(p.mode, mode::PLATEAU);
}

#[test]
fn an_unknown_verdict_is_null_but_the_point_stays_measured() {
    let view = spec_fixture(&[("WEIRD", 14, 5)]);
    let p = point(stage(&view, STAGE_SPEC_REVIEW), 0);
    assert!(p.measured, "a readable review body is measured");
    assert_eq!(p.verdict, None, "only the three tokens are verdicts");
    assert_one_token(p, "verdict", token::REVIEW_VERDICT);
}

#[test]
fn an_absent_review_body_is_not_a_measured_round() {
    let spec = artifact(ArtifactKind::RequirementsSpec, 1, -1, None);
    let exec_id = new_id();
    let review = artifact(ArtifactKind::ReviewReport, 1, 0, Some(exec_id));
    let exec = execution(exec_id, &[spec.id], 0, Some(5));
    let mut bodies = HashMap::new();
    bodies.insert(spec.id, spec_body(14, 5));
    // The review artifact exists but its blob does not.
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&[spec, review], &[exec], &cache);
    let p = point(stage(&view, STAGE_SPEC_REVIEW), 0);
    assert!(!p.measured);
    assert_one_token(p, "verdict", token::REVIEW_VERDICT);
    assert_eq!(p.requirements_count, Some(14), "counts survive the gap");
}

#[test]
fn an_unreadable_review_body_keeps_the_counters_from_the_bound_spec() {
    let spec = artifact(ArtifactKind::RequirementsSpec, 1, -1, None);
    let exec_id = new_id();
    let review = artifact(ArtifactKind::ReviewReport, 1, 0, Some(exec_id));
    let exec = execution(exec_id, &[spec.id], 0, Some(5));
    let mut bodies = HashMap::new();
    bodies.insert(spec.id, spec_body(14, 5));
    bodies.insert(review.id, Body::Unreadable);
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&[spec, review], &[exec], &cache);
    let p = point(stage(&view, STAGE_SPEC_REVIEW), 0);
    assert!(!p.measured);
    assert_one_token(p, "verdict", token::REVIEW_VERDICT);
    assert_eq!(p.requirements_count, Some(14));
    assert_eq!(p.oq_count, Some(5));
    assert_one_token(p, "delta_requirements", token::CURRENT_UNMEASURED);
}

#[test]
fn spec_gaps_are_named_not_zeroed() {
    // binding absent: the review names no execution
    let review = artifact(ArtifactKind::ReviewReport, 1, 0, None);
    let mut bodies = HashMap::new();
    bodies.insert(review.id, object(json!({ "verdict": "NEEDS_REVISION" })));
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&[review], &[], &cache);
    let p = point(stage(&view, STAGE_SPEC_REVIEW), 0);
    assert_eq!(p.requirements_count, None);
    assert_one_token(p, "requirements_count", token::SPEC_BINDING);

    // spec body absent
    let exec_id = new_id();
    let bound = artifact(ArtifactKind::ReviewReport, 1, 0, Some(exec_id));
    let spec = artifact(ArtifactKind::RequirementsSpec, 1, -1, None);
    let exec = execution(exec_id, &[spec.id], 0, Some(5));
    let mut bodies = HashMap::new();
    bodies.insert(bound.id, object(json!({ "verdict": "NEEDS_REVISION" })));
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&[spec, bound], &[exec], &cache);
    let p = point(stage(&view, STAGE_SPEC_REVIEW), 0);
    assert_one_token(p, "oq_count", token::SPEC_BODY);
    assert!(p.measured, "a readable review body stays measured");

    // key absent -> spec_counts (an explicit empty array is 0)
    let spec = artifact(ArtifactKind::RequirementsSpec, 1, -1, None);
    let exec_id = new_id();
    let bound = artifact(ArtifactKind::ReviewReport, 1, 0, Some(exec_id));
    let exec = execution(exec_id, &[spec.id], 0, Some(5));
    let mut bodies = HashMap::new();
    bodies.insert(spec.id, object(json!({ "requirements": [] })));
    bodies.insert(bound.id, object(json!({ "verdict": "NEEDS_REVISION" })));
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&[spec, bound], &[exec], &cache);
    let p = point(stage(&view, STAGE_SPEC_REVIEW), 0);
    assert_eq!(p.requirements_count, Some(0));
    assert_one_token(p, "oq_count", token::SPEC_COUNTS);
}

#[test]
fn a_null_spec_counter_makes_the_delta_and_direction_unavailable() {
    let spec1 = artifact(ArtifactKind::RequirementsSpec, 1, -2, None);
    let spec2 = artifact(ArtifactKind::RequirementsSpec, 2, -1, None);
    let mut artifacts = vec![spec1.clone(), spec2.clone()];
    let mut executions = Vec::new();
    let mut bodies = HashMap::new();
    bodies.insert(spec1.id, spec_body(14, 5));
    bodies.insert(
        spec2.id,
        object(json!({ "requirements": vec![json!({"id": "R"}); 22] })),
    );
    for (i, spec) in [&spec1, &spec2].iter().enumerate() {
        let exec_id = new_id();
        let review = artifact(
            ArtifactKind::ReviewReport,
            i as u32 + 1,
            i as i64,
            Some(exec_id),
        );
        executions.push(execution(exec_id, &[spec.id], i as i64, Some(i as i64 + 1)));
        bodies.insert(review.id, object(json!({ "verdict": "NEEDS_REVISION" })));
        artifacts.push(review);
    }
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&artifacts, &executions, &cache);
    let p = point(stage(&view, STAGE_SPEC_REVIEW), 1);
    assert!(p.comparable);
    assert_eq!(p.delta_requirements, Some(8));
    assert_eq!(p.delta_oq, None);
    assert_one_token(p, "delta_oq", token::COUNTS_UNAVAILABLE);
    assert_eq!(p.volume_direction, None);
    assert_one_token(p, "volume_direction", token::COUNTS_UNAVAILABLE);
    assert_eq!(p.mode, mode::UNKNOWN);
    assert_eq!(p.mode_basis.reason, Some(reason::NO_PREDICATE));
}

// ---------------------------------------------------------------------------
// REQ-CV-02/10/11/12: rounds, profile, time, counters
// ---------------------------------------------------------------------------

#[test]
fn rounds_are_ordered_by_seq_then_created_at_then_id() {
    let a = artifact(ArtifactKind::QualityReport, 2, 5, None);
    let b = artifact(ArtifactKind::QualityReport, 1, 9, None);
    let c = artifact(ArtifactKind::QualityReport, 1, 9, None);
    let mut bodies = HashMap::new();
    for x in [&a, &b, &c] {
        bodies.insert(
            x.id,
            quality_body(Some(0.9), &[], Some(0.8), Some(("home", 12))),
        );
    }
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&[a.clone(), b.clone(), c.clone()], &[], &cache);
    let s = stage(&view, STAGE_QUALITY);
    assert_eq!(s.series.len(), 3);
    let mut expected = [b.id, c.id, a.id];
    if c.id < b.id {
        expected.swap(0, 1);
    }
    assert_eq!(
        s.series
            .iter()
            .map(|p| p.artifact_id.clone())
            .collect::<Vec<_>>(),
        expected.iter().map(|i| i.to_string()).collect::<Vec<_>>()
    );
    assert_eq!(
        s.series.iter().map(|p| p.round).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
}

#[test]
fn stale_and_superseded_reports_are_still_rounds() {
    // REQ-CV-02 AC1: the round count is the number of reports of the kind,
    // status included — a superseded carrier is history, not a hole.
    let mut bodies = HashMap::new();
    let mut artifacts = Vec::new();
    for (i, status) in [
        ArtifactStatus::Current,
        ArtifactStatus::Stale,
        ArtifactStatus::Superseded,
    ]
    .iter()
    .enumerate()
    {
        let mut a = artifact(ArtifactKind::QualityReport, i as u32 + 1, i as i64, None);
        a.status = *status;
        bodies.insert(
            a.id,
            quality_body(Some(0.9), &[], Some(0.8), Some(("home", 12))),
        );
        artifacts.push(a);
    }
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&artifacts, &[], &cache);
    let s = stage(&view, STAGE_QUALITY);
    assert_eq!(s.series.len(), 3, "all three carriers are rounds");
    assert_eq!(s.counters.rounds_total, 3);
    assert_eq!(
        s.series
            .iter()
            .map(|p| p.status.as_str())
            .collect::<Vec<_>>(),
        vec!["CURRENT", "STALE", "SUPERSEDED"]
    );
}

#[test]
fn profile_boundary_breaks_stuck_series() {
    // REQ-CV-10 AC1: 12,13,13 is not a stuck series; 13,13,13 is.
    let build = |profiles: [u64; 3]| {
        let mut bodies = HashMap::new();
        let mut artifacts = Vec::new();
        for (i, version) in profiles.iter().enumerate() {
            let a = artifact(ArtifactKind::QualityReport, i as u32 + 1, i as i64, None);
            bodies.insert(
                a.id,
                quality_body(
                    Some(0.9),
                    &[("same", Some(0.9))],
                    Some(0.8),
                    Some(("home", *version)),
                ),
            );
            artifacts.push(a);
        }
        let cache = BodyCache::from_bodies(bodies);
        compute(&artifacts, &[], &cache)
    };
    let crossing = build([12, 13, 13]);
    let s = stage(&crossing, STAGE_QUALITY);
    assert_ne!(s.series[2].mode, mode::STUCK);
    assert_one_token(&s.series[1], "delta_score_milli", token::SEGMENT_BOUNDARY);
    let same = build([13, 13, 13]);
    let s = stage(&same, STAGE_QUALITY);
    assert_eq!(s.series[2].mode, mode::STUCK);
}

#[test]
fn time_is_finished_minus_started_and_an_absent_time_is_not_zero() {
    let mut bodies = HashMap::new();
    let e1 = new_id();
    let a1 = artifact(ArtifactKind::QualityReport, 1, 0, Some(e1));
    let e2 = new_id();
    let a2 = artifact(ArtifactKind::QualityReport, 2, 1, Some(e2));
    for a in [&a1, &a2] {
        bodies.insert(
            a.id,
            quality_body(Some(0.9), &[], Some(0.8), Some(("home", 12))),
        );
    }
    let cache = BodyCache::from_bodies(bodies);
    let ex1 = execution(e1, &[], 0, Some(100));
    let ex2 = execution(e2, &[], 105, None);
    let view = compute(&[a1, a2], &[ex1, ex2], &cache);
    let s = stage(&view, STAGE_QUALITY);
    assert_eq!(s.series[0].time_seconds, Some(100));
    assert_eq!(s.series[1].time_seconds, None);
    assert_one_token(&s.series[1], "time_seconds", token::TIME);
    assert_eq!(s.stage_total_seconds, Some(100));
    assert_eq!(
        s.stage_current_seconds,
        Some(100),
        "both rounds are home@12"
    );
    assert_eq!(s.counters.rounds_without_time, 1);
}

#[test]
fn a_stage_without_any_known_time_says_stage_time() {
    let view = quality_series(&[(0.9, vec![])]);
    let s = stage(&view, STAGE_QUALITY);
    assert_eq!(s.stage_total_seconds, None);
    assert_eq!(s.stage_current_seconds, None);
    assert_eq!(
        s.missing,
        vec![
            MissingEntry {
                field: "stage_total_seconds",
                token: token::STAGE_TIME,
            },
            MissingEntry {
                field: "stage_current_seconds",
                token: token::STAGE_TIME,
            },
        ]
    );
    assert_eq!(s.counters.rounds_without_time, 1);
}

#[test]
fn stage_current_time_is_the_last_profile_segment_not_the_whole_series() {
    // REQ-CV-11 AC1: 100 s + 50 s = 150 s total; the current segment is the
    // last profile (home@13), so its own sum is 50 s.
    let mut bodies = HashMap::new();
    let mut artifacts = Vec::new();
    for (i, (secs, version)) in [(100i64, 12u64), (50, 13)].iter().enumerate() {
        let e = new_id();
        let a = artifact(ArtifactKind::QualityReport, i as u32 + 1, i as i64, Some(e));
        bodies.insert(
            a.id,
            quality_body(Some(0.9), &[], Some(0.8), Some(("home", *version))),
        );
        artifacts.push((a, e, *secs));
    }
    let execs: Vec<_> = artifacts
        .iter()
        .map(|(_, e, secs)| execution(*e, &[], 0, Some(*secs)))
        .collect();
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(
        &artifacts
            .iter()
            .map(|(a, _, _)| a.clone())
            .collect::<Vec<_>>(),
        &execs,
        &cache,
    );
    let s = stage(&view, STAGE_QUALITY);
    assert_eq!(s.stage_total_seconds, Some(150), "100 + 50");
    assert_eq!(
        s.stage_current_seconds,
        Some(50),
        "the current segment is home@13 (round 2 only)"
    );
}

#[test]
fn a_profile_change_in_the_middle_breaks_both_neighbours_and_keeps_the_last_segment() {
    // 12, 12, 13, 13: the boundary sits between rounds 2 and 3; the last
    // segment (13) is the current one. A boundary on the far side does not
    // retroactively merge the segments.
    let mut bodies = HashMap::new();
    let mut artifacts = Vec::new();
    for (i, version) in [12u64, 12, 13, 13].iter().enumerate() {
        let e = new_id();
        let a = artifact(ArtifactKind::QualityReport, i as u32 + 1, i as i64, Some(e));
        bodies.insert(
            a.id,
            quality_body(
                Some(0.9),
                &[("a", Some(0.9))],
                Some(0.8),
                Some(("home", *version)),
            ),
        );
        artifacts.push((a, e));
    }
    let execs: Vec<_> = artifacts
        .iter()
        .map(|(_, e)| execution(*e, &[], 0, Some(10)))
        .collect();
    let cache = BodyCache::from_bodies(bodies);
    let rows: Vec<Artifact> = artifacts.iter().map(|(a, _)| a.clone()).collect();
    let view = compute(&rows, &execs, &cache);
    let s = stage(&view, STAGE_QUALITY);
    assert!(
        !s.series[0].comparable,
        "the first round has no predecessor"
    );
    assert!(s.series[1].comparable, "12 -> 12 is one segment");
    assert!(!s.series[2].comparable, "12 -> 13 is a boundary");
    assert_one_token(&s.series[2], "delta_score_milli", token::SEGMENT_BOUNDARY);
    assert!(s.series[3].comparable, "13 -> 13 is one segment");
    assert_eq!(s.stage_total_seconds, Some(40), "four rounds of 10 s");
    assert_eq!(s.stage_current_seconds, Some(20), "two rounds of home@13");
}

#[test]
fn an_unknown_last_profile_has_no_current_segment_even_with_known_times() {
    let mut bodies = HashMap::new();
    let mut artifacts = Vec::new();
    for (i, profile) in [Some(("home", 12u64)), None].iter().enumerate() {
        let e = new_id();
        let a = artifact(ArtifactKind::QualityReport, i as u32 + 1, i as i64, Some(e));
        bodies.insert(a.id, quality_body(Some(0.9), &[], Some(0.8), *profile));
        artifacts.push((a, e));
    }
    let execs: Vec<_> = artifacts
        .iter()
        .map(|(_, e)| execution(*e, &[], 0, Some(7)))
        .collect();
    let cache = BodyCache::from_bodies(bodies);
    let rows: Vec<Artifact> = artifacts.iter().map(|(a, _)| a.clone()).collect();
    let view = compute(&rows, &execs, &cache);
    let s = stage(&view, STAGE_QUALITY);
    assert_eq!(s.stage_total_seconds, Some(14), "the total is known");
    assert_eq!(s.stage_current_seconds, None, "no segment is provable");
    assert!(
        s.missing
            .iter()
            .any(|m| m.field == "stage_current_seconds" && m.token == token::STAGE_TIME)
    );
}

#[test]
fn spec_versions_without_review_are_a_counter_not_rounds() {
    let spec1 = artifact(ArtifactKind::RequirementsSpec, 1, -3, None);
    let spec2 = artifact(ArtifactKind::RequirementsSpec, 2, -2, None);
    let spec3 = artifact(ArtifactKind::RequirementsSpec, 3, -1, None);
    let mut artifacts = vec![spec1.clone(), spec2.clone(), spec3.clone()];
    let mut executions = Vec::new();
    let mut bodies = HashMap::new();
    for (i, spec) in [&spec1, &spec2].iter().enumerate() {
        bodies.insert(spec.id, spec_body(14, 5));
        let exec_id = new_id();
        let review = artifact(
            ArtifactKind::ReviewReport,
            i as u32 + 1,
            i as i64,
            Some(exec_id),
        );
        executions.push(execution(exec_id, &[spec.id], i as i64, Some(i as i64 + 1)));
        bodies.insert(review.id, object(json!({ "verdict": "NEEDS_REVISION" })));
        artifacts.push(review);
    }
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&artifacts, &executions, &cache);
    let s = stage(&view, STAGE_SPEC_REVIEW);
    assert_eq!(s.series.len(), 2);
    assert_eq!(s.counters.spec_versions_without_review, 1);
    assert_eq!(s.counters.rounds_total, 2);
}

#[test]
fn the_stages_are_spec_then_quality_and_counters_count_the_series() {
    let mut artifacts = Vec::new();
    let mut bodies = HashMap::new();
    let q = artifact(ArtifactKind::QualityReport, 1, 0, None);
    bodies.insert(
        q.id,
        quality_body(Some(0.9), &[], Some(0.8), Some(("home", 12))),
    );
    artifacts.push(q);
    let r = artifact(ArtifactKind::ReviewReport, 1, 0, None);
    bodies.insert(r.id, object(json!({ "verdict": "NEEDS_REVISION" })));
    artifacts.push(r);
    let cache = BodyCache::from_bodies(bodies);
    let view = compute(&artifacts, &[], &cache);
    assert_eq!(
        view.stages.iter().map(|s| s.stage).collect::<Vec<_>>(),
        vec![STAGE_SPEC_REVIEW, STAGE_QUALITY]
    );
    assert_eq!(stage(&view, STAGE_SPEC_REVIEW).counters.rounds_total, 1);
    assert_eq!(stage(&view, STAGE_QUALITY).counters.rounds_total, 1);
}

#[test]
fn a_body_is_read_once_per_artifact_for_the_whole_slice() {
    let spec = artifact(ArtifactKind::RequirementsSpec, 1, -1, None);
    let mut artifacts = vec![spec.clone()];
    let mut executions = Vec::new();
    let mut bodies = HashMap::new();
    bodies.insert(spec.id, spec_body(14, 5));
    for i in 0..5 {
        let exec_id = new_id();
        let review = artifact(ArtifactKind::ReviewReport, i + 1, i as i64, Some(exec_id));
        executions.push(execution(exec_id, &[spec.id], i as i64, Some(i as i64 + 1)));
        bodies.insert(review.id, object(json!({ "verdict": "NEEDS_REVISION" })));
        artifacts.push(review);
    }
    let cache = BodyCache::from_bodies(bodies);
    let _ = compute(&artifacts, &executions, &cache);
    assert_eq!(
        cache.reads(),
        6,
        "five review bodies + ONE shared spec body (REQ-CV-12 AC7)"
    );
}

#[test]
fn two_reads_of_unchanged_data_are_byte_identical() {
    let view = quality_series(&[
        (0.943, vec![("a", Some(0.5))]),
        (0.997, vec![("a", Some(1.0))]),
        (0.991, vec![("b", Some(0.9))]),
    ]);
    let first = serde_json::to_string(&view).unwrap();
    let second = serde_json::to_string(&view).unwrap();
    assert_eq!(first, second);
}

#[test]
fn a_store_read_failure_is_named_apart_from_a_corrupt_body() {
    // QA R2: a store fault keeps its code and message on the point and
    // carries `read_error`; bytes that were produced but are not JSON keep
    // the report token and carry NO read error.
    let failed = artifact(ArtifactKind::QualityReport, 1, 0, None);
    let corrupt = artifact(ArtifactKind::QualityReport, 2, 1, None);
    let failed_id = failed.id;
    let read = move |a: &Artifact| {
        if a.id == failed_id {
            Err(StoreError::Corrupt(
                "blob 0 is stored but unreadable: EIO".into(),
            ))
        } else {
            Ok(b"not a json object".to_vec())
        }
    };
    let cache = BodyCache::from_reader(&read);
    let view = compute(&[failed, corrupt], &[], &cache);
    let s = stage(&view, STAGE_QUALITY);

    let err = s.series[0]
        .read_error
        .as_ref()
        .expect("the store failure is kept on the point");
    assert_eq!(err.code, "corrupt");
    assert!(err.message.contains("unreadable"));
    assert!(!s.series[0].measured);
    assert_one_token(&s.series[0], "score_milli", token::READ_ERROR);

    assert!(s.series[1].read_error.is_none(), "corrupt is not a fault");
    assert_one_token(&s.series[1], "score_milli", token::QUALITY_REPORT);
}

#[test]
fn a_failed_spec_read_carries_the_store_code_and_keeps_the_review_measured() {
    let spec = artifact(ArtifactKind::RequirementsSpec, 1, -1, None);
    let exec_id = new_id();
    let review = artifact(ArtifactKind::ReviewReport, 1, 0, Some(exec_id));
    let exec = execution(exec_id, &[spec.id], 0, Some(5));
    let spec_id = spec.id;
    let read = move |a: &Artifact| {
        if a.id == spec_id {
            Err(StoreError::Io(std::io::Error::new(
                std::io::ErrorKind::Other,
                "disk gone",
            )))
        } else {
            Ok(br#"{"verdict":"NEEDS_REVISION"}"#.to_vec())
        }
    };
    let cache = BodyCache::from_reader(&read);
    let view = compute(&[spec, review], &[exec], &cache);
    let p = point(stage(&view, STAGE_SPEC_REVIEW), 0);
    let err = p.read_error.as_ref().expect("spec read failure kept");
    assert_eq!(err.code, "io");
    assert!(err.message.contains("disk gone"));
    assert_one_token(p, "requirements_count", token::READ_ERROR);
    assert!(p.measured, "the review body itself was readable");
}

// ---------------------------------------------------------------------------
// REQ-CV-05: the shape and the closed vocabularies
// ---------------------------------------------------------------------------

#[test]
fn every_point_carries_27_keys_and_a_closed_missing_vocabulary() {
    let view = quality_series(&[
        (0.943, vec![("a", Some(0.5))]),
        (0.997, vec![("b", Some(0.9))]),
    ]);
    let v = serde_json::to_value(&view).unwrap();
    for p in v["stages"][0]["series"].as_array().unwrap() {
        let obj = p.as_object().unwrap();
        // 26 REQ-CV-05 keys + QA R2 `read_error` (the store code/message).
        assert_eq!(obj.len(), 27, "the point shape is 27 keys");
        for entry in p["missing"].as_array().unwrap() {
            let t = entry["token"].as_str().unwrap();
            assert!(MISSING_TOKENS.contains(&t), "closed vocabulary: {t}");
        }
    }
}

#[test]
fn an_inapplicable_field_stays_null_without_a_token() {
    let view = quality_series(&[(0.94, vec![("a", Some(1.0))])]);
    let p = point(stage(&view, STAGE_QUALITY), 0);
    assert_eq!(p.verdict, None);
    assert_eq!(p.requirements_count, None);
    assert_eq!(p.volume_direction, None);
    for field in [
        "verdict",
        "requirements_count",
        "oq_count",
        "delta_requirements",
        "delta_oq",
        "volume_direction",
    ] {
        assert!(
            !p.missing.iter().any(|m| m.field == field),
            "quality must not carry a token on {field}"
        );
    }
}

#[test]
fn a_spec_point_never_carries_a_quality_token() {
    let view = spec_fixture(&[("NEEDS_REVISION", 14, 5)]);
    let p = point(stage(&view, STAGE_SPEC_REVIEW), 0);
    for m in &p.missing {
        assert!(
            !matches!(
                m.token,
                token::QUALITY_REPORT | token::QUALITY_THETA | token::PROFILE
            ),
            "spec point carries a quality-only token: {m:?}"
        );
    }
    assert_eq!(p.score_milli, None);
    assert_eq!(p.profile, None);
    assert_eq!(p.volume_direction, None, "first round has no direction");
}

#[test]
fn unknown_is_a_closed_mode_with_its_reason() {
    let view = quality_series(&[(0.94, vec![("a", Some(0.9))])]);
    let p = point(stage(&view, STAGE_QUALITY), 0);
    assert_eq!(p.mode, mode::UNKNOWN);
    assert_eq!(p.mode_basis.rule, rule::UNKNOWN);
    assert_eq!(p.mode_basis.reason, Some(reason::INSUFFICIENT_DATA));
    assert!(p.mode_basis.rounds.is_empty());
    assert_eq!(p.mode_basis.check_id, None);
}

#[test]
fn no_carriers_means_no_stages_not_an_error() {
    let cache = BodyCache::from_bodies(HashMap::new());
    let view = compute(&[], &[], &cache);
    assert!(view.stages.is_empty());
    assert_eq!(view.gap_milli, GAP_MILLI);
}
