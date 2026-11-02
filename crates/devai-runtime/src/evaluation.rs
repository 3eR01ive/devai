//! The evaluator: executes an assessment profile against a workspace and
//! assembles the machine-readable quality report.
//!
//! Sovereignty split, straight from `docs/evaluation-schema.md`:
//! * gates and mechanical probes are run BY THE PLATFORM (`execute`) — a
//!   model cannot forge or forget them;
//! * the agent contributes only rubric scores inside its assessment;
//! * the verdict is computed here, by the deterministic function (§2.5):
//!   pending human signature is checked BEFORE comparing S with theta, so a
//!   draft human score can never harden into a final FAIL or PASS;
//! * `report_text` renders §2.6 for the human, and is only a rendering.

use std::collections::HashMap;
use std::path::Path;
use std::process::Command;

use std::sync::LazyLock;

use devai_store::config::Profile;
use serde::Serialize;
use serde_json::Value;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum ProbeState {
    Pass,
    Fail,
    Blocked,
}

#[derive(Clone, Debug, Serialize)]
pub struct GateOutcome {
    pub id: String,
    pub state: ProbeState,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub detail: String,
}

#[derive(Clone, Debug)]
pub struct ProbeOutcome {
    pub state: ProbeState,
    pub detail: String,
    pub output: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct CheckOutcome {
    pub id: String,
    pub verify: String,
    #[serde(rename = "state")]
    pub state: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub score: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub floor: Option<f64>,
    pub weight: f64,
    pub detects: Vec<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub violations: Vec<String>,
    /// QA rule (quality-report@3, carried unchanged into @4): ALWAYS
    /// non-empty after assembly — every criteria entry cites at least one
    /// concrete piece of evidence. Kept serialized even when empty so a
    /// future bug fails the contract gate with `minimum 1` instead of hiding
    /// behind a missing key.
    pub evidence: Vec<String>,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub reason: String,
}

/// Everything the platform learned by running the profile's probes.
pub struct EvalRun {
    pub gates: Vec<GateOutcome>,
    /// mechanical checks: id -> outcome
    pub mech: HashMap<String, ProbeOutcome>,
    /// hybrid checks: id -> candidate outputs collected by probes
    pub hybrid: HashMap<String, Vec<ProbeOutcome>>,
    /// Human-readable dump fed to the auditor alongside the spec.
    pub evidence_text: String,
}

const SNIPPET: usize = 400;

fn exec_probe(cmd: &str, workspace: &Path, pass_exit: &[i64]) -> ProbeOutcome {
    let res = Command::new("sh")
        .arg("-c")
        .arg(cmd)
        .current_dir(workspace)
        .output();
    match res {
        Err(e) => ProbeOutcome {
            state: ProbeState::Blocked,
            detail: format!("probe `{cmd}`: cannot launch: {e}"),
            output: String::new(),
        },
        Ok(out) => {
            let code = out.status.code().unwrap_or(-1) as i64;
            let stdout = String::from_utf8_lossy(&out.stdout);
            let stderr = String::from_utf8_lossy(&out.stderr);
            let text = format!("{stdout}{stderr}");
            let snippet: String = text.trim().chars().take(SNIPPET).collect();
            // Semantics: explicit pass list wins; shell-level errors (126/127:
            // not runnable / missing binary) are BLOCKED — an unperformed
            // check never pretends to pass; any other code is FAIL.
            let state = if pass_exit.is_empty() {
                if code == 0 {
                    ProbeState::Pass
                } else if matches!(code, 126 | 127) {
                    ProbeState::Blocked
                } else {
                    ProbeState::Fail
                }
            } else if pass_exit.contains(&code) {
                ProbeState::Pass
            } else if matches!(code, 126 | 127) {
                ProbeState::Blocked
            } else {
                ProbeState::Fail
            };
            ProbeOutcome {
                state,
                detail: format!("`{cmd}` -> exit {code}{}", if snippet.is_empty() { String::new() } else { format!(": {snippet}") }),
                output: text.to_string(),
            }
        }
    }
}

/// Run all gates and every mechanical/hybrid probe of the profile.
pub fn execute(profile: &Profile, workspace: &Path) -> EvalRun {
    let mut gates = Vec::new();
    for g in &profile.gates {
        let outs: Vec<ProbeOutcome> = g.probes.iter().map(|p| exec_probe(&p.cmd, workspace, &p.pass_exit)).collect();
        let state = reduce_state(&outs);
        let detail = outs.iter().map(|o| o.detail.clone()).collect::<Vec<_>>().join("; ");
        gates.push(GateOutcome { id: g.id.clone(), state, detail });
    }

    let mut mech: HashMap<String, ProbeOutcome> = HashMap::new();
    let mut hybrid: HashMap<String, Vec<ProbeOutcome>> = HashMap::new();
    let mut lines: Vec<String> = Vec::new();
    for g in &gates {
        lines.push(format!("gate {}: {} ({})", g.id, state_name(g.state), g.detail));
    }
    for c in profile.checks() {
        if c.verify == "mechanical" {
            let outs: Vec<ProbeOutcome> = c.probes.iter().map(|p| exec_probe(&p.cmd, workspace, &p.pass_exit)).collect();
            let state = reduce_state(&outs);
            let detail = outs.iter().map(|o| o.detail.clone()).collect::<Vec<_>>().join("; ");
            let output = outs.iter().map(|o| o.output.clone()).collect::<Vec<_>>().join("\n");
            if matches!(state, ProbeState::Fail) {
                lines.push(format!("check {}: FAILED — {detail}", c.id));
            } else {
                lines.push(format!("check {}: {}", c.id, state_name(state)));
            }
            mech.insert(
                c.id.clone(),
                ProbeOutcome {
                    state,
                    detail,
                    output,
                },
            );
        } else if c.verify == "hybrid" {
            // The probe only proposes candidates; the rubric decides.
            let outs: Vec<ProbeOutcome> = c.probes.iter().map(|p| exec_probe(&p.cmd, workspace, &p.pass_exit)).collect();
            let mut kept = Vec::new();
            for o in &outs {
                let out: String = o.output.trim().chars().take(2000).collect();
                if o.state == ProbeState::Pass && !out.is_empty() {
                    lines.push(format!("candidates for {}: \n{}", c.id, out));
                } else {
                    lines.push(format!("candidates for {}: {} ({})", c.id, state_name(o.state), o.detail));
                }
                kept.push(ProbeOutcome { output: out, ..o.clone() });
            }
            hybrid.insert(c.id.clone(), kept);
        }
    }

    EvalRun { gates, mech, hybrid, evidence_text: lines.join("\n") }
}

fn reduce_state(outs: &[ProbeOutcome]) -> ProbeState {
    if outs.iter().any(|o| o.state == ProbeState::Fail) {
        ProbeState::Fail
    } else if outs.iter().any(|o| o.state == ProbeState::Blocked) {
        ProbeState::Blocked
    } else {
        ProbeState::Pass
    }
}

fn state_name(s: ProbeState) -> &'static str {
    match s {
        ProbeState::Pass => "PASS",
        ProbeState::Fail => "FAIL",
        ProbeState::Blocked => "BLOCKED",
    }
}

/// QA rule "evidence per criterion" (born in `quality-report@3`, carried
/// unchanged into @4): the platform never emits a criteria entry without a
/// citation. The check's own observation
/// counts first — the probe command + exit code (`detail`), the gap record,
/// the discard reason; a rubric score with no finding cited is at least
/// traceable to the auditor's assessment execution artifact. Blank strings
/// are stripped so `minItems: 1` can never be satisfied by filler.
fn guarantee_evidence(c: &mut CheckOutcome, assessment_execution: &str) {
    c.evidence.retain(|e| !e.trim().is_empty());
    if c.evidence.is_empty() {
        let citation = if !c.reason.trim().is_empty() {
            c.reason.clone()
        } else {
            format!(
                "auditor assessment execution {assessment_execution}: score {} recorded for `{}` with no finding cited",
                c.score.map(|s| format!("{s:.2}")).unwrap_or_else(|| "n/a".into()),
                c.id
            )
        };
        c.evidence.push(citation);
    }
}

pub struct Assembled {
    /// Full quality-report@4 document (evidence rule from @3 + the
    /// platform-captured `workspace_revision` block).
    pub json: Value,
    /// ACCEPT | RETURN | ESCALATE — platform recommendation for the human.
    pub recommendation: &'static str,
    /// PASS | FAIL | INCONCLUSIVE — the machine verdict.
    pub evaluation_verdict: &'static str,
    pub score: f64,
    pub report_text: String,
}

/// Merge probe results + the auditor's assessment into quality-report@4 and
/// apply the deterministic verdict function (§2.5). `revision` is the
/// workspace state the PLATFORM captured at execution start — embedded
/// verbatim so every file:line finding is traceable to a bound workspace.
pub fn assemble(profile: &Profile, run: &EvalRun, assessment: &Value, assessment_execution: &str, revision: &devai_core::types::WorkspaceRevision) -> Assembled {
    let scores: HashMap<String, f64> = assessment
        .get("scores")
        .and_then(|s| s.as_object())
        .map(|o| o.iter().filter_map(|(k, v)| v.as_f64().map(|x| (k.clone(), x))).collect())
        .unwrap_or_default();
    let gap_reasons: HashMap<String, String> = assessment
        .get("gaps")
        .and_then(|g| g.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|g| Some((g.get("check")?.as_str()?.to_string(), g.get("reason")?.as_str()?.to_string())))
                .collect()
        })
        .unwrap_or_default();
    let findings: Vec<(String, String, Vec<String>)> = assessment
        .get("findings")
        .and_then(|f| f.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|f| {
                    Some((
                        f.get("check")?.as_str()?.to_string(),
                        f.get("text")?.as_str()?.to_string(),
                        f.get("evidence")
                            .and_then(|e| e.as_array())
                            .map(|e| e.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                            .unwrap_or_default(),
                    ))
                })
                .collect()
        })
        .unwrap_or_default();

    let mut checks: Vec<CheckOutcome> = Vec::new();
    let mut pending: Vec<String> = Vec::new();
    let mut blocked_weight = 0.0f64;
    let mut scored: Vec<(String, f64, f64)> = Vec::new(); // id, score, weight

    for c in profile.checks() {
        let weight = profile.weight_of(&c.id);
        let floor = profile.floor_for(c);
        let find_ev = |id: &str, probe_out: Option<&Vec<ProbeOutcome>>| -> Vec<String> {
            let mut ev: Vec<String> =
                findings.iter().filter(|(cid, _, _)| cid == id).flat_map(|(_, _, e)| e.clone()).collect();
            if let Some(outs) = probe_out {
                ev.extend(outs.iter().filter(|o| !o.output.trim().is_empty()).map(|o| {
                    let head: String = o.output.trim().chars().take(160).collect();
                    head
                }));
            }
            ev
        };

        let mut outcome = if c.verify == "mechanical" || c.reviewer == "tool" {
            match run.mech.get(&c.id) {
                Some(o) if o.state == ProbeState::Pass => {
                    // Platform-run results count into S exactly like rubric ones.
                    scored.push((c.id.clone(), 1.0, weight));
                    CheckOutcome {
                    id: c.id.clone(),
                    verify: c.verify.clone(),
                    state: "MECHANICAL_PASS".into(),
                    score: Some(1.0),
                    floor,
                    weight,
                    detects: c.detects.clone(),
                        violations: vec![],
                        evidence: vec![o.detail.clone()],
                        reason: String::new(),
                    }
                }
                Some(o) if o.state == ProbeState::Fail => {
                    scored.push((c.id.clone(), 0.0, weight));
                    CheckOutcome {
                        id: c.id.clone(),
                        verify: c.verify.clone(),
                        state: "MECHANICAL_FAIL".into(),
                        score: Some(0.0),
                        floor,
                        weight,
                        detects: c.detects.clone(),
                        violations: vec![o.detail.clone()],
                        evidence: vec![o.detail.clone()],
                        reason: String::new(),
                    }
                }
                Some(o) => {
                    blocked_weight += weight;
                    CheckOutcome {
                        id: c.id.clone(),
                        verify: c.verify.clone(),
                        state: "BLOCKED".into(),
                        score: None,
                        floor,
                        weight,
                        detects: c.detects.clone(),
                        violations: vec![],
                        evidence: vec![],
                        reason: o.detail.clone(),
                    }
                }
                // hybrid checks with reviewer `tool`: use probe state directly.
                None if c.reviewer == "tool" => {
                    blocked_weight += weight;
                    CheckOutcome {
                        id: c.id.clone(),
                        verify: c.verify.clone(),
                        state: "BLOCKED".into(),
                        score: None,
                        floor,
                        weight,
                        detects: c.detects.clone(),
                        violations: vec![],
                        evidence: vec![],
                        reason: "mechanical reviewer `tool` but no mechanical probes ran".into(),
                    }
                }
                None => {
                    blocked_weight += weight;
                    CheckOutcome {
                        id: c.id.clone(),
                        verify: c.verify.clone(),
                        state: "BLOCKED".into(),
                        score: None,
                        floor,
                        weight,
                        detects: c.detects.clone(),
                        violations: vec![],
                        evidence: vec![],
                        reason: "no probe result for mechanical check".into(),
                    }
                }
            }
        } else if c.reviewer == "human" {
            pending.push(c.id.clone());
            CheckOutcome {
                id: c.id.clone(),
                verify: c.verify.clone(),
                state: "PENDING_HUMAN".into(),
                score: None,
                floor,
                weight,
                detects: c.detects.clone(),
                violations: vec![],
                evidence: vec![],
                reason: "reviewer: human — platform leaves it unsigned".into(),
            }
        } else {
            // Rubric/hybrid scored by the agent.
            match scores.get(&c.id) {
                None => {
                    let reason = gap_reasons
                        .get(&c.id)
                        .cloned()
                        .or_else(|| gap_reasons.get("*").cloned())
                        .unwrap_or_else(|| "auditor returned no score for this check".into());
                    blocked_weight += weight;
                    CheckOutcome {
                        id: c.id.clone(),
                        verify: c.verify.clone(),
                        state: "BLOCKED".into(),
                        score: None,
                        floor,
                        weight,
                        detects: c.detects.clone(),
                        violations: vec![],
                        evidence: vec![],
                        reason,
                    }
                }
                Some(s) => {
                    let probe_out = run.hybrid.get(&c.id);
                    let evidence = find_ev(&c.id, probe_out);
                    // proc/evidence-completeness (meta, weight 0.0) enforced
                    // HERE: a sub-1.0 rubric score without evidence is not a
                    // score at all.
                    if *s < 1.0 && c.evidence == "required" && evidence.iter().all(|e| e.trim().is_empty()) {
                        blocked_weight += weight;
                        CheckOutcome {
                            id: c.id.clone(),
                            verify: c.verify.clone(),
                            state: "BLOCKED".into(),
                            score: None,
                            floor,
                            weight,
                            detects: c.detects.clone(),
                            violations: vec![],
                            evidence: vec![],
                            reason: format!("score {s} without evidence — discarded (proc/evidence-completeness)"),
                        }
                    } else {
                        let violations: Vec<String> =
                            findings.iter().filter(|(cid, _, _)| cid == &c.id).map(|(_, t, _)| t.clone()).collect();
                        scored.push((c.id.clone(), *s, weight));
                        CheckOutcome {
                            id: c.id.clone(),
                            verify: c.verify.clone(),
                            state: "SCORED".into(),
                            score: Some(*s),
                            floor,
                            weight,
                            detects: c.detects.clone(),
                            violations,
                            evidence,
                            reason: String::new(),
                        }
                    }
                }
            }
        };
        guarantee_evidence(&mut outcome, assessment_execution);
        checks.push(outcome);
    }

    // S: weighted mean over SCORED checks only (gates unscored, blocked and
    // pending excluded — under-cap renormalisation, §2.5).
    let w_sum: f64 = scored.iter().map(|(_, _, w)| w).sum();
    let score = if w_sum > 1e-9 {
        scored.iter().map(|(_, s, w)| s * w).sum::<f64>() / w_sum
    } else {
        0.0
    };
    let floors_breached: Vec<String> = checks
        .iter()
        .filter_map(|c| match (c.score, c.floor) {
            (Some(s), Some(f)) if s < f => Some(format!("{}={:.2}<floor {:.2}", c.id, s, f)),
            _ => None,
        })
        .collect();

    let gate_fail = run.gates.iter().find(|g| g.state == ProbeState::Fail);
    let gate_blocked = run.gates.iter().find(|g| g.state == ProbeState::Blocked);
    let theta = profile.scoring.theta;
    let cap = profile.scoring.blocked_policy.max_blocked_weight;

    // Deterministic verdict function — the ORDER is the calibration fix:
    // pending signature outranks any S comparison.
    let (verdict, rec, basis) = if let Some(g) = gate_fail {
        ("FAIL", "RETURN", format!("gate `{}` не пройден: {}", g.id, g.detail))
    } else if let Some(g) = gate_blocked {
        ("INCONCLUSIVE", "ESCALATE", format!("gate `{}` не выполнен: {}", g.id, g.detail))
    } else if !pending.is_empty() {
        ("INCONCLUSIVE", "ESCALATE", format!("нет человеческой подписи: {} — вердикт не готов, черновик балла не финализируется", pending.join(", ")))
    } else if blocked_weight > cap {
        ("INCONCLUSIVE", "ESCALATE", format!("невыполненных проверок на вес {blocked_weight:.2} > {cap}"))
    } else if !floors_breached.is_empty() {
        ("FAIL", "RETURN", format!("красные линии: {}", floors_breached.join(", ")))
    } else if score < theta {
        ("FAIL", "RETURN", format!("S={:.3} < порога {theta}", score))
    } else {
        ("PASS", "ACCEPT", format!("шлюзы зелёные, полы целы, S={:.3} ≥ {theta}", score))
    };

    let report_text = render(profile, verdict, score, basis.clone(), &checks, &run.gates, &pending, blocked_weight);
    let json = serde_json::json!({
        "verdict": rec,
        "recommendation_basis": basis,
        "report_text": report_text,
        "profile": { "project": profile.project, "version": profile.version },
        // Workspace provenance: the exact state this report judges. Equal to
        // the revision on the auditor's execution row — recorded, never
        // recomputed here.
        "workspace_revision": revision,
        "assessment_execution": assessment_execution,
        "evaluation": {
            "verdict": verdict,
            "score": (score * 1000.0).round() / 1000.0,
            "theta": theta,
            "blocked_weight": (blocked_weight * 1000.0).round() / 1000.0,
            "max_blocked_weight": cap,
            "floors_breached": floors_breached,
            "pending_human": pending,
            "gates": run.gates,
            "checks": checks,
        }
    });
    Assembled { json, recommendation: rec, evaluation_verdict: verdict, score, report_text }
}

/// §2.6 rendering: number first, then only complaints, worst-first;
/// unperformed checks listed, never folded into a pass.
fn render(
    profile: &Profile,
    verdict: &str,
    score: f64,
    basis: String,
    checks: &[CheckOutcome],
    gates: &[GateOutcome],
    pending: &[String],
    blocked_weight: f64,
) -> String {
    let head = match verdict {
        "PASS" => "ПРОШЁЛ",
        "FAIL" => "НЕ ПРОШЁЛ",
        _ => "ВЕРДИКТ НЕ ГОТОВ",
    };
    let mut lines = vec![format!("ВЕРДИКТ: {head} (S={score:.3}/1.00, порог {}, {basis})", profile.scoring.theta)];
    if let Some(g) = gates.iter().find(|g| g.state != ProbeState::Pass) {
        lines.push(format!("ШЛЮЗ: {} — {}", g.id, state_name(g.state)));
    }

    // Worst-first by the severity of the check's heaviest detectable type.
    let rank = |id: &str| -> usize {
        let c = profile.checks().into_iter().find(|c| c.id == id);
        c.and_then(|c| profile.worst_severity(c)).map(|s| SEV_RANK[s]).unwrap_or(usize::MAX)
    };
    let mut complaints: Vec<(usize, String)> = Vec::new();
    for c in checks {
        for v in &c.violations {
            complaints.push((rank(&c.id), format!("[{}] {v}", c.id)));
        }
    }
    if !complaints.is_empty() {
        complaints.sort_by_key(|(r, _)| *r);
        lines.push(String::new());
        lines.push("ЧТО ПЛОХО:".into());
        for (i, (_, text)) in complaints.iter().enumerate() {
            lines.push(format!("{}. {text}", i + 1));
        }
    }

    let blocked: Vec<String> = checks
        .iter()
        .filter(|c| c.state == "BLOCKED")
        .map(|c| format!("{} — {}", c.id, c.reason))
        .collect();
    if !blocked.is_empty() {
        lines.push(String::new());
        lines.push(format!("НЕ ПРОВЕРЕНО (вес {:.2}): {}", blocked_weight, blocked.join("; ")));
    }
    if !pending.is_empty() {
        lines.push(String::new());
        lines.push(format!("ЖДЁТ ЧЕЛОВЕКА: {} — подпись нужна до того, как вердикт станет готов", pending.join(", ")));
    }
    lines.join("\n")
}

static SEV_RANK: LazyLock<HashMap<&'static str, usize>> = LazyLock::new(|| {
    [("catastrophic", 0), ("major", 1), ("minor", 2), ("cosmetic", 3)].into_iter().collect()
});

#[cfg(test)]
mod tests {
    use super::*;

    /// Two-check profile: one mechanical (true/false), one rubric (either),
    /// one human-signed. theta/floors/cap as in the factory profile.
    fn tiny_profile(gate_cmd: &str, mech_cmd: &str) -> Profile {
        let yaml = format!(
            r#"
version: 1
project: testproj
applies_to_roles: [quality-auditor]
error_types:
  correctness/state-transition: {{ desc: d, severity: catastrophic }}
  process/untested-branch: {{ desc: d, severity: minor }}
gates:
  - id: gate-1
    desc: trivial gate
    probes: [{{ cmd: "{gate_cmd}", pass_exit: [0] }}]
evaluations:
  - id: correctness
    title: T
    checks:
      - id: mech/a
        statement: mechanical check
        detects: [process/untested-branch]
        verify: mechanical
        probes: [{{ cmd: "{mech_cmd}", pass_exit: [0] }}]
        reviewer: tool
      - id: rub/b
        statement: rubric check
        detects: [process/untested-branch]
        verify: rubric
        anchors:
          "1.0": ok
          "0.4": mid
          "0.0": bad
        reviewer: either
      - id: hum/c
        statement: human check
        detects: [correctness/state-transition]
        verify: rubric
        anchors:
          "1.0": ok
          "0.4": mid
          "0.0": bad
        reviewer: human
scoring:
  theta: 0.8
  floors: {{ catastrophic: 0.7, major: 0.4, minor: null }}
  blocked_policy: {{ max_blocked_weight: 0.2 }}
  weights:
    mech/a: 0.5
    rub/b: 0.3
    hum/c: 0.2
"#
        );
        serde_yaml::from_str(&yaml).expect("tiny profile parses")
    }

    fn assessment(pairs: &[(&str, f64)], gaps: &[(&str, &str)], findings: &[(&str, &str, &[&str])]) -> Value {
        serde_json::json!({
            "scores": pairs.iter().map(|(k, v)| (k.to_string(), serde_json::json!(v))).collect::<serde_json::Map<_, _>>(),
            "findings": findings.iter().map(|(c, t, e)| serde_json::json!({"check": c, "text": t, "evidence": e})).collect::<Vec<_>>(),
            "gaps": gaps.iter().map(|(c, r)| serde_json::json!({"check": c, "reason": r})).collect::<Vec<_>>(),
        })
    }

    fn run_ok(profile: &Profile) -> EvalRun {
        execute(profile, Path::new(env!("CARGO_MANIFEST_DIR")))
    }

    /// A stand-in for the platform-captured workspace revision: assemble
    /// embeds it verbatim; the engine passes the real computed object.
    fn fixture_revision() -> devai_core::types::WorkspaceRevision {
        devai_core::types::WorkspaceRevision {
            git_head: Some("a".repeat(40)),
            dirty_digest: Some("b".repeat(64)),
            profile: Some("testproj@1".into()),
            roles: vec!["quality-auditor@1".into()],
        }
    }

    #[test]
    fn pending_human_outranks_score_comparison() {
        // The calibration fix: even a perfect S with an unsigned human check
        // must NOT finalize — verdict is not-ready, not FAIL, not PASS.
        let p = tiny_profile("true", "true");
        let run = run_ok(&p);
        let a = assessment(&[("rub/b", 1.0)], &[], &[]);
        let out = assemble(&p, &run, &a, "exec-1", &fixture_revision());
        assert_eq!(out.evaluation_verdict, "INCONCLUSIVE");
        assert_eq!(out.recommendation, "ESCALATE");
        assert!(out.report_text.contains("ЖДЁТ ЧЕЛОВЕКА: hum/c"), "{}", out.report_text);
        assert!(out.report_text.starts_with("ВЕРДИКТ: ВЕРДИКТ НЕ ГОТОВ"));
    }

    #[test]
    fn gate_failure_beats_everything_with_return() {
        let p = tiny_profile("true", "false");
        let run = run_ok(&p);
        let a = assessment(&[], &[("rub/b", "no access")], &[]);
        let out = assemble(&p, &run, &a, "exec-1", &fixture_revision());
        // gate ok, but mech FAIL -> S drops; pending human -> inconclusive first.
        assert_eq!(out.evaluation_verdict, "INCONCLUSIVE");
        let p2 = tiny_profile("false", "true");
        let run2 = execute(&p2, Path::new(env!("CARGO_MANIFEST_DIR")));
        let out2 = assemble(&p2, &run2, &a, "exec-2", &fixture_revision());
        assert_eq!(out2.evaluation_verdict, "FAIL");
        assert_eq!(out2.recommendation, "RETURN");
        assert!(out2.report_text.contains("ШЛЮЗ: gate-1 — FAIL"));
    }

    #[test]
    fn blocked_beyond_cap_is_not_ready() {
        let p = tiny_profile("true", "exit 127");
        let run = run_ok(&p);
        // No assessment at all: mech blocked (127) weight .5 > cap .2.
        let a = assessment(&[], &[], &[]);
        let _out = assemble(&p, &run, &a, "exec-0", &fixture_revision());
        // pending human fires first here; drop hum/c to test the cap path:
        let mut p2 = p.clone();
        p2.evaluations[0].checks.retain(|c| c.id != "hum/c");
        p2.scoring.weights = vec![("mech/a".into(), 0.5), ("rub/b".into(), 0.3)].into_iter().collect();
        // V7 sum != 1 by design for this check path — validate isn't called here.
        let run2 = execute(&p2, Path::new(env!("CARGO_MANIFEST_DIR")));
        let out2 = assemble(&p2, &run2, &a, "exec-1", &fixture_revision());
        assert_eq!(out2.evaluation_verdict, "INCONCLUSIVE");
        assert!(out2.report_text.contains("НЕ ПРОВЕРЕНО"), "{}", out2.report_text);
    }

    #[test]
    fn rubric_score_without_evidence_is_discarded() {
        // proc/evidence-completeness enforced by the platform, not the model.
        let mut p = tiny_profile("true", "true");
        p.evaluations[0].checks.retain(|c| c.id != "hum/c");
        let run = execute(&p, Path::new(env!("CARGO_MANIFEST_DIR")));
        let a = assessment(&[("rub/b", 0.4)], &[], &[]);
        let out = assemble(&p, &run, &a, "exec-1", &fixture_revision());
        let j = &out.json["evaluation"]["checks"];
        let b = j
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["id"] == "rub/b")
            .unwrap();
        assert_eq!(b["state"], "BLOCKED");
        assert!(b["reason"].as_str().unwrap().contains("without evidence"));
    }

    #[test]
    fn pass_needs_all_floors_and_theta() {
        let mut p = tiny_profile("true", "true");
        p.evaluations[0].checks.retain(|c| c.id != "hum/c");
        let run = execute(&p, Path::new(env!("CARGO_MANIFEST_DIR")));
        // 0.9 < 1.0 must carry evidence, or our own meta-check discards it.
        let a = assessment(&[("rub/b", 0.9)], &[], &[("rub/b", "single style drift", &["x.rs:1"])]);
        let out = assemble(&p, &run, &a, "exec-1", &fixture_revision());
        assert_eq!(out.evaluation_verdict, "PASS");
        assert_eq!(out.recommendation, "ACCEPT");
        assert!((out.score - (0.5 * 1.0 + 0.3 * 0.9) / 0.8).abs() < 1e-6, "S={}", out.score);
        assert!(out.report_text.starts_with("ВЕРДИКТ: ПРОШЁЛ"), "{}", out.report_text);
        // Floor breach: catastrophic floor 0.7 on a 0.4-scored major-detecting
        // check does not exist here (minor), so push rub/b below its floor via
        // a catastrophic detects swap.
        p.evaluations[0].checks[1].detects = vec!["correctness/state-transition".into()];
        let run = execute(&p, Path::new(env!("CARGO_MANIFEST_DIR")));
        let a = assessment(&[("rub/b", 0.4)], &[], &[("rub/b", "leak", &["x.rs:1"])]);
        let out = assemble(&p, &run, &a, "exec-1", &fixture_revision());
        assert_eq!(out.evaluation_verdict, "FAIL");
        assert!(out.report_text.contains("красные линии"), "{}", out.report_text);
        // findings render worst-first with the complaint text.
        assert!(out.report_text.contains("[rub/b] leak"));
    }

    #[test]
    fn probe_exit_semantics_match_grammar() {
        // 126/127 -> BLOCKED, other codes -> FAIL, listed pass_exit -> PASS.
        let out = exec_probe("exit 127", Path::new(env!("CARGO_MANIFEST_DIR")), &[0]);
        assert_eq!(out.state, ProbeState::Blocked);
        let out = exec_probe("exit 1", Path::new(env!("CARGO_MANIFEST_DIR")), &[0]);
        assert_eq!(out.state, ProbeState::Fail);
        let out = exec_probe("exit 1", Path::new(env!("CARGO_MANIFEST_DIR")), &[0, 1]);
        assert_eq!(out.state, ProbeState::Pass);
        // hybrid probes that find candidates still PASS; output is carried.
        let out = exec_probe("echo candidate.rs:10", Path::new(env!("CARGO_MANIFEST_DIR")), &[0]);
        assert_eq!(out.state, ProbeState::Pass);
        assert!(out.output.contains("candidate.rs:10"));
    }

    #[test]
    fn assembled_report_cites_evidence_on_every_check() {
        // QA rule: every criteria entry carries concrete evidence for ANY
        // mix of check states, and the assembled doc passes the very gate
        // the engine runs against the SEEDED @4 schema (which additionally
        // requires the platform-captured workspace revision).
        let mut p = tiny_profile("true", "true");
        p.evaluations[0].checks.retain(|c| c.id != "hum/c");
        let run = execute(&p, Path::new(env!("CARGO_MANIFEST_DIR")));
        let schema = devai_store::seed::contracts()
            .into_iter()
            .find(|(n, v, _)| *n == "quality-report" && *v == 4)
            .expect("quality-report@4 is seeded")
            .2;

        // Rubric 1.0 with no findings: previously an evidence-less entry.
        let a = assessment(&[("rub/b", 1.0)], &[], &[]);
        let out = assemble(&p, &run, &a, "exec-42", &fixture_revision());
        let checks = out.json["evaluation"]["checks"].as_array().unwrap();
        assert!(!checks.is_empty());
        for c in checks {
            let ev = c["evidence"]
                .as_array()
                .unwrap_or_else(|| panic!("check {} must serialize evidence", c["id"]));
            assert!(!ev.is_empty(), "check {} must cite at least one concrete evidence item", c["id"]);
            assert!(
                ev.iter().all(|e| e.as_str().map(|s| !s.trim().is_empty()).unwrap_or(false)),
                "check {} evidence entries must be non-blank strings: {ev:?}",
                c["id"]
            );
        }
        let b = checks.iter().find(|c| c["id"] == "rub/b").unwrap();
        assert!(b["evidence"][0].as_str().unwrap().contains("exec-42"), "uncited score stays traceable");
        assert_eq!(
            out.json["workspace_revision"], serde_json::json!(fixture_revision()),
            "assemble must embed the platform revision verbatim"
        );
        crate::adapters::validate_against_contract(&out.json.to_string(), Some(schema))
            .expect("assembled report must satisfy quality-report@4");

        // Gap-only assessment (mock-cycle shape): BLOCKED entries cite their
        // gap record, the report still satisfies the contract.
        let a2 = assessment(&[], &[("rub/b", "offline: no access to the diff")], &[]);
        let out2 = assemble(&p, &run, &a2, "exec-43", &fixture_revision());
        for c in out2.json["evaluation"]["checks"].as_array().unwrap() {
            let ev = c["evidence"].as_array().unwrap();
            assert!(!ev.is_empty(), "state {} of {} must still cite evidence", c["state"], c["id"]);
        }
        crate::adapters::validate_against_contract(&out2.json.to_string(), Some(schema))
            .expect("blocked-mix report must satisfy quality-report@4");
    }
}
