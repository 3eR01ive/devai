//! Workflow state machine and sovereignty rules.
//!
//! Pure domain logic: every function takes the current task (and whatever
//! domain data the operation inspects), validates the operation against the
//! workflow, and returns a [`Mutation`] — the next task state plus the
//! immutable events to append. No IO happens here.

use serde_json::json;

use crate::error::{CoreError, CoreResult};
use crate::types::*;

/// The effect of one validated workflow command.
#[derive(Clone, Debug)]
pub struct Mutation {
    /// Task after the change: new state, `lock_version` incremented,
    /// `updated_at` refreshed.
    pub task: Task,
    pub events: Vec<EventDraft>,
    /// Artifact status downgrades (STALE / SUPERSEDED). Results are never
    /// deleted, only marked (history is immutable).
    pub artifact_status: Vec<(Id, ArtifactStatus)>,
    /// Question rows a blocking command parks the task on. Persisted by the
    /// store in the SAME transaction as the state change and the events, so
    /// a held task is always answerable (REQ-RG-02: the empty-rework gate's
    /// loud state is a real, actionable question, not only a log line).
    pub questions: Vec<QuestionDraft>,
}

// ---------------------------------------------------------------------------
// helpers
// ---------------------------------------------------------------------------

const ACTIVE_STATES: &[TaskState] = &[
    TaskState::Draft,
    TaskState::Analysis,
    TaskState::Review,
    TaskState::Approval,
    TaskState::Implementation,
    TaskState::Qa,
    TaskState::Rework,
];

fn require_state(task: &Task, allowed: &[TaskState], op: &'static str, hint: Option<String>) -> CoreResult<()> {
    if allowed.contains(&task.state) {
        Ok(())
    } else {
        Err(CoreError::InvalidTransition {
            op,
            from: task.state.as_str().to_string(),
            hint,
        })
    }
}

/// Builds a mutation that changes the task state and appends one event.
fn transition(
    task: &Task,
    to: TaskState,
    kind: &'static str,
    actor: &str,
    now: Timestamp,
    mut payload: serde_json::Value,
) -> Mutation {
    let mut next = task.clone();
    let from = next.state;
    next.state = to;
    next.updated_at = now;
    next.lock_version += 1;
    if to.is_hold() {
        next.return_state = Some(from);
    } else {
        next.return_state = None;
    }
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("from".into(), json!(from.as_str()));
        obj.insert("to".into(), json!(to.as_str()));
    }
    Mutation {
        task: next,
        events: vec![EventDraft { kind, actor: actor.to_string(), payload }],
        artifact_status: Vec::new(),
        questions: Vec::new(),
    }
}

/// Builds a mutation for an event that does not change the workflow state
/// (the lock version still advances: something happened to the aggregate).
fn note(task: &Task, kind: &'static str, actor: &str, now: Timestamp, payload: serde_json::Value) -> Mutation {
    let mut next = task.clone();
    next.updated_at = now;
    next.lock_version += 1;
    Mutation {
        task: next,
        events: vec![EventDraft { kind, actor: actor.to_string(), payload }],
        artifact_status: Vec::new(),
        questions: Vec::new(),
    }
}

/// The CURRENT requirements spec, if any. Since schema v3 the store keeps
/// at most one CURRENT row per (task_id, kind), so the filter is already
/// single-valued for every platform caller; the `seq` tie-break stays as a
/// defensive belt for legacy-shape slices handed to this pure function by
/// older callers/tests (REQ-005 scoped the workaround removal to the CLI
/// reads, which now select by status alone).
fn current_spec(artifacts: &[Artifact]) -> Option<&Artifact> {
    artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::RequirementsSpec && a.status == ArtifactStatus::Current)
        .max_by_key(|a| a.seq)
}

// ---------------------------------------------------------------------------
// dependency gate (REQ-03/04)
// ---------------------------------------------------------------------------

/// The task ids that still constrain `task` through an OPEN `blocks` edge
/// (REQ-03: resume and human acceptance are refused while this is non-empty;
/// REQ-04 AC2: readiness is computable from stored data alone).
///
/// RATIFIED DECISION OQ1 defines "closed": a `blocks` edge is CLOSED only
/// when the depended-on task is ACCEPTED. CANCELLED and FAILED do NOT close
/// it — a dead dependency means an unresolved problem, and re-tuning the
/// graph (superseding the edge, OQ7) is a human act, not an implicit
/// platform release. Every other state — the hold family included — keeps
/// the edge open: a PAUSED or FAILED blocker may still be resumed and
/// finish. The deadlock this deliberately accepts (a CANCELLED blocker pins
/// its dependents until a human unlinks the edge or ratifies the override)
/// is the ratified trade-off: the platform never decides for the operator.
///
/// RATIFIED DECISION OQ3: `parent` edges NEVER gate — pure genealogy.
pub fn open_blockers(task: &Task, deps: &[TaskDependency]) -> Vec<String> {
    deps.iter()
        .filter(|d| d.task_id == task.id && d.kind == DependencyKind::Blocks && d.depends_on_state != TaskState::Accepted)
        .map(|d| d.depends_on_id.to_string())
        .collect()
}

/// RATIFIED DECISION OQ2 — the human-escape of the dependency gate. The
/// refusal of resume/acceptance is unconditional, BUT an operator may
/// explicitly ratify "continue despite" through the ordinary decisions
/// mechanism: a `dependency_override` decision with verdict APPROVED whose
/// `refs` list every currently-open blocker id. Coverage is EXACT: a
/// decision made before a NEW blocker appeared does not release the gate
/// for it, and only verdict APPROVED counts. When it does not cover, the
/// gate stands exactly as REQ-03 describes.
pub fn override_covers(task: &Task, blockers: &[String], decisions: &[Decision]) -> bool {
    if blockers.is_empty() {
        return false;
    }
    decisions.iter().any(|d| {
        d.task_id == task.id
            && d.kind == decision_kind::DEPENDENCY_OVERRIDE
            && d.verdict == Verdict::Approved
            && blockers.iter().all(|b| d.refs.iter().any(|r| r == b))
    })
}

/// The effective blockers of `task`: OPEN `blocks` edges (OQ1/OQ3) minus
/// those covered by a human override decision (OQ2). An empty result lets
/// resume/acceptance through; the non-empty result IS the refusal payload —
/// blocking ids as data (REQ-03 AC1).
pub fn effective_blockers(task: &Task, deps: &[TaskDependency], decisions: &[Decision]) -> Vec<String> {
    let open = open_blockers(task, deps);
    if override_covers(task, &open, decisions) {
        Vec::new()
    } else {
        open
    }
}

// ---------------------------------------------------------------------------
// commands
// ---------------------------------------------------------------------------

pub fn start(task: &Task, actor: &str, now: Timestamp) -> CoreResult<Mutation> {
    require_state(task, &[TaskState::Draft], "start", CoreError::hint("task is created but its workflow not started"))?;
    Ok(transition(task, TaskState::Analysis, event_kind::WORKFLOW_STARTED, actor, now, json!({})))
}

/// A new Requirements Specification version was published (by the Analyst
/// role or by the user via `revise`). The spec artifact must already exist in
/// `artifacts` with `status = CURRENT`. Supersedes previous versions, marks
/// dependent results stale and returns the task to REVIEW.
pub fn requirements_published(
    task: &Task,
    artifacts: &[Artifact],
    reason: Option<&str>,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(
        task,
        &[TaskState::Analysis, TaskState::Review, TaskState::Approval, TaskState::Implementation, TaskState::Qa, TaskState::Rework],
        if task.requirements_seq == 0 { "publish-requirements" } else { "revise" },
        CoreError::hint("run `devai task start` first; a specification can only be published while the task is in ANALYSIS/REVIEW/APPROVAL/IMPLEMENTATION/QA/REWORK"),
    )?;
    let Some(spec) = current_spec(artifacts) else {
        return Err(CoreError::guard("no CURRENT REQUIREMENTS_SPEC artifact found"));
    };

    let is_revise = task.requirements_seq > 0;
    let mut m = transition(task, TaskState::Review, if is_revise { event_kind::REQUIREMENTS_REVISED } else { event_kind::REQUIREMENTS_PUBLISHED }, actor, now, json!({
        (json_field::REQUIREMENTS_SEQ): spec.seq,
        (json_field::SPEC_ID): spec.id.to_string(),
        (json_field::REASON): reason,
    }));
    m.task.requirements_seq = spec.seq;

    // The predecessor specs this publication replaces. The set is keyed on
    // (kind, older seq), NOT on CURRENT: since the store-level generic
    // supersession (REQ-001) the publication already flipped prior CURRENT
    // rows to SUPERSEDED inside its own transaction before this transition
    // ever sees the snapshot, and a CURRENT-only filter would resolve to an
    // empty set — silently killing the stale-closure propagation below
    // (REQ-007). Only rows not yet marked SUPERSEDED get a status flip in
    // this mutation, so the store event log stays the single source of
    // supersession events and no already-flipped row is written twice
    // (REQ-007 acceptance: no double flip, no competing sources).
    let predecessors: Vec<&Artifact> = artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::RequirementsSpec && a.id != spec.id && a.seq < spec.seq)
        .collect();
    let superseded: Vec<Id> = predecessors.iter().map(|a| a.id).collect();
    for a in predecessors.iter().filter(|a| a.status != ArtifactStatus::Superseded) {
        m.artifact_status.push((a.id, ArtifactStatus::Superseded));
    }

    // Downstream results of the superseded specification become STALE.
    if is_revise {
        let staled = stale_closure(artifacts, &superseded);
        if !staled.is_empty() {
            m.artifact_status.extend(staled.iter().map(|id| (*id, ArtifactStatus::Stale)));
            m.events.push(EventDraft {
                kind: event_kind::ARTIFACTS_STALED,
                actor: actor.to_string(),
                payload: json!({ (json_field::STALED): staled.iter().map(|i| i.to_string()).collect::<Vec<_>>(), (json_field::CAUSE): "requirements revision" }),
            });
        }
    }
    Ok(m)
}

pub fn review_recorded(
    task: &Task,
    verdict: Verdict,
    summary: Option<&str>,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(task, &[TaskState::Review], "record-review", CoreError::hint("an independent review applies only in REVIEW"))?;
    let to = match verdict {
        Verdict::Approved => TaskState::Approval,
        _ => TaskState::Analysis,
    };
    Ok(transition(task, to, event_kind::REVIEW_RECORDED, actor, now, json!({ (json_field::VERDICT): verdict.as_str(), (json_field::SUMMARY): summary })))
}

pub fn approve_requirements(task: &Task, rationale: Option<&str>, actor: &str, now: Timestamp) -> CoreResult<Mutation> {
    require_state(task, &[TaskState::Approval], "approve", CoreError::hint(
        "implementation may only start after requirements were independently reviewed and approved here",
    ))?;
    if task.requirements_seq == 0 {
        return Err(CoreError::guard("no requirements version to approve"));
    }
    Ok(transition(task, TaskState::Implementation, event_kind::REQUIREMENTS_APPROVED, actor, now, json!({
        "requirements_seq": task.requirements_seq,
        "rationale": rationale,
    })))
}

pub fn reject_requirements(task: &Task, rationale: Option<&str>, actor: &str, now: Timestamp) -> CoreResult<Mutation> {
    require_state(task, &[TaskState::Approval], "reject", CoreError::hint("requirements can be rejected only while in APPROVAL"))?;
    Ok(transition(task, TaskState::Analysis, event_kind::REQUIREMENTS_REJECTED, actor, now, json!({ (json_field::RATIONALE): rationale })))
}

/// One previous IMPLEMENTATION_REPORT as the producer read it from stored
/// values: identity + version order + the platform-stamped `commit` the body
/// carries (engine-side stamp of implementation-report@2). `None` commit means
/// "this report names no comparable commit" (read-only producer, pre-@2 body,
/// manual publication without the field) — a fact the gate must SEE, not guess.
#[derive(Clone, Debug)]
pub struct PriorImplementationReport {
    pub artifact_id: Id,
    pub seq: u32,
    pub commit: Option<String>,
}

/// The stored facts a producer assembles AT PUBLICATION TIME for
/// [`implementation_reported`] (REQ-RG-01: producer-side detection, never
/// read-time; comparison over stored values only, no git recompute). The gate
/// itself lives in this module — the single serialization point every
/// publication path passes through, so no producer can bypass it.
#[derive(Clone, Debug)]
pub struct ImplementationReportFacts {
    pub summary: Option<String>,
    /// The report being published (already stored when this command runs).
    pub current_report: Id,
    /// Its stamped `commit`, read from the stored body.
    pub current_commit: Option<String>,
    /// Every other IMPLEMENTATION_REPORT of the task; the gate compares
    /// against the one with the highest `seq`.
    pub priors: Vec<PriorImplementationReport>,
    /// The task's stored `rework_noop_ratified` decisions (REQ-RG-03: only a
    /// human command records one; the engine never does).
    pub ratifications: Vec<Decision>,
    /// The `quality.returned` rationale that opened the current REWORK cycle,
    /// quoted into the gate event (REQ-RG-02 AC1: the loud record names the
    /// points the skipped rework was supposed to fix).
    pub qa_return_rationale: Option<String>,
}

impl ImplementationReportFacts {
    /// Facts for a producer with nothing to compare against — a first report,
    /// an IMPLEMENTATION publication the gate never touches (REQ-RG-01), or a
    /// test fixture. A REWORK publication built from `bare` facts resolves to
    /// the distinct INCOMPARABLE outcome of REQ-RG-04, never to a silent pass.
    pub fn bare(current_report: Id) -> Self {
        Self { summary: None, current_report, current_commit: None, priors: Vec::new(), ratifications: Vec::new(), qa_return_rationale: None }
    }
}

/// Publish of an IMPLEMENTATION_REPORT: IMPLEMENTATION → QA as always;
/// REWORK → QA only through the empty-rework gate (REQ-RG-01..04). The gate
/// compares the stamped `commit` of the report being published with the stamped
/// `commit` of the previous report (by seq) of the same task. Identical — and
/// not yet ratified for this exact pair — is NOT a silent ride into QA: the
/// task parks in WAITING_FOR_INPUT (returning to REWORK) on a blocking question
/// and an immutable trip event naming both reports, both shas and the QA
/// return that opened the cycle (REQ-RG-02). A human `rework_noop_ratified`
/// decision covering the pair re-opens the passage with `ratified_by` on the
/// transition event (REQ-RG-03); a ratification belongs to its cycle's pair
/// only, never to the task at large (AC4). Uncomparable sides produce the
/// distinct `qa.rework_gate_incomparable` record and NO verdict claim
/// (REQ-RG-04).
pub fn implementation_reported(
    task: &Task,
    facts: &ImplementationReportFacts,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(task, &[TaskState::Implementation, TaskState::Rework], "publish-implementation-report", CoreError::hint(
        "implementation reports are recorded in IMPLEMENTATION or REWORK",
    ))?;

    let mut payload = json!({ "summary": facts.summary });
    let mut incomparable: Option<(&'static str, Option<String>)> = None;
    // The gate binds REWORK publications only: the first report of a task and
    // an IMPLEMENTATION publication have nothing to compare against by
    // construction and pass exactly as before (REQ-RG-01).
    if task.state == TaskState::Rework {
        match facts.priors.iter().max_by_key(|p| p.seq) {
            None => {
                incomparable = Some(("no prior implementation report to compare against", None));
            }
            Some(prior) => {
                let prev_sha = prior.commit.as_deref();
                let cur_sha = facts.current_commit.as_deref();
                match (prev_sha, cur_sha) {
                    (Some(p), Some(c)) if p == c => {
                        // Identical tips: with the worktree discipline an
                        // unchanged rework re-names the same commit
                        // (worktree.rs `commit_platform_work` returns the tip,
                        // never an empty commit) — so equality means the
                        // rework brought no new work.
                        let ratified = facts.ratifications.iter().find(|d| {
                            d.kind == decision_kind::REWORK_NOOP_RATIFIED
                                && d.verdict == Verdict::Approved
                                && d.refs.iter().any(|r| *r == prior.artifact_id.to_string())
                                && d.refs.iter().any(|r| *r == facts.current_report.to_string())
                        });
                        match ratified {
                            None => return Ok(empty_rework_trip(task, facts, prior, p, c, actor, now)),
                            Some(d) => {
                                payload[json_field::GATE] = json!("empty-rework");
                                payload[json_field::VERDICT] = json!("IDENTICAL_RATIFIED");
                                payload[json_field::PREV_REPORT] = json!(prior.artifact_id.to_string());
                                payload[json_field::PREV_COMMIT] = json!(p);
                                payload[json_field::RATIFIED_BY] = json!(d.id.to_string());
                            }
                        }
                    }
                    (Some(p), Some(c)) => {
                        payload[json_field::GATE] = json!("empty-rework");
                        payload[json_field::VERDICT] = json!("DISTINCT");
                        payload[json_field::PREV_REPORT] = json!(prior.artifact_id.to_string());
                        payload[json_field::PREV_COMMIT] = json!(p);
                        payload[json_field::CURRENT_COMMIT] = json!(c);
                    }
                    _ => {
                        // REQ-RG-04: at least one side names no commit. The
                        // passage proceeds (the contract leaves `commit`
                        // optional by construction — forcing it would punish
                        // the legitimate read-only producer), but the record
                        // says the comparison NEVER RAN.
                        let side = if prev_sha.is_none() && cur_sha.is_none() {
                            "neither report names a stamped commit"
                        } else if prev_sha.is_none() {
                            "the previous report names no stamped commit"
                        } else {
                            "the published report names no stamped commit"
                        };
                        incomparable = Some((side, Some(prior.artifact_id.to_string())));
                    }
                }
            }
        }
    }

    let mut m = transition(task, TaskState::Qa, event_kind::IMPLEMENTATION_REPORTED, actor, now, payload);
    if let Some((reason, prev_report)) = incomparable {
        m.events.push(EventDraft {
            kind: event_kind::QA_REWORK_GATE_INCOMPARABLE,
            actor: actor.to_string(),
            payload: json!({
                (json_field::PREV_REPORT): prev_report,
                (json_field::CURRENT_REPORT): facts.current_report.to_string(),
                (json_field::REASON): reason,
            }),
        });
    }
    Ok(m)
}

/// The loud outcome of an unratified identical-commit REWORK publication:
/// the task parks in WAITING_FOR_INPUT returning to REWORK, one immutable
/// `qa.rework_gate_tripped` event names both reports, both shas and the QA
/// points that were supposed to be fixed, and the blocking question the
/// operator acts on is carried IN this mutation so the store persists it in
/// the same transaction (REQ-RG-02: no quiet log-only exit, no state change
/// without an answerable question).
fn empty_rework_trip(
    task: &Task,
    facts: &ImplementationReportFacts,
    prior: &PriorImplementationReport,
    prev_sha: &str,
    cur_sha: &str,
    actor: &str,
    now: Timestamp,
) -> Mutation {
    let question_id = new_id();
    let mut m = transition(
        task,
        TaskState::WaitingForInput,
        event_kind::QUESTION_RAISED,
        actor,
        now,
        json!({
            (json_field::QUESTION_ID): question_id.to_string(),
            "blocking": true,
            (json_field::GATE): "empty-rework",
        }),
    );
    m.events.push(EventDraft {
        kind: event_kind::QA_REWORK_GATE_TRIPPED,
        actor: actor.to_string(),
        payload: json!({
            (json_field::PREV_REPORT): prior.artifact_id.to_string(),
            (json_field::CURRENT_REPORT): facts.current_report.to_string(),
            (json_field::PREV_COMMIT): prev_sha,
            (json_field::CURRENT_COMMIT): cur_sha,
            (json_field::QA_RETURN_RATIONALE): facts.qa_return_rationale,
            (json_field::QUESTION_ID): question_id.to_string(),
        }),
    });
    m.questions.push(QuestionDraft {
        id: question_id,
        text: format!(
            "REWORK publication {} names commit {cur_sha} — the SAME commit as report {} ({prev_sha}): the rework captured no new changes. QA returned with: {}. If the points were genuinely already satisfied, the operator may ratify the no-op explicitly: `devai task ratify-empty-rework {} --rationale \"why no fixes were required\"` (a stored human decision — the QA transition will cite it). Otherwise run the implementation-agent with the actual fixes and publish again.",
            facts.current_report,
            prior.artifact_id,
            facts.qa_return_rationale.as_deref().unwrap_or("(rationale not stored)"),
            task.id,
        ),
        context: Some(format!("gate=empty-rework prev={} current={} commit={cur_sha}", prior.artifact_id, facts.current_report)),
        options: vec!["ratify-empty-rework (no fixes were required)".into(), "re-run implementation with fixes".into()],
        blocking: true,
    });
    m
}

pub fn accept(
    task: &Task,
    artifacts: &[Artifact],
    deps: &[TaskDependency],
    decisions: &[Decision],
    rationale: Option<&str>,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(task, &[TaskState::Qa], "accept", CoreError::hint("acceptance applies only in QA"))?;
    // REQ-03: the human gate cannot pass a task an open blocker still
    // holds; the refusal enumerates the blocking ids. The only release is
    // the human's own `dependency_override` decision (ratified OQ2) —
    // checked here against the same stored decisions the QA auditor reads.
    let blockers = effective_blockers(task, deps, decisions);
    if !blockers.is_empty() {
        return Err(CoreError::OpenBlockers { blockers });
    }
    let has_quality = artifacts
        .iter()
        .any(|a| a.kind == ArtifactKind::QualityReport && a.status == ArtifactStatus::Current);
    if !has_quality {
        return Err(CoreError::guard(
            "acceptance requires a CURRENT QUALITY_REPORT artifact: checks must be independent of the implementation",
        ));
    }
    Ok(transition(task, TaskState::Accepted, event_kind::TASK_ACCEPTED, actor, now, json!({ "rationale": rationale })))
}

pub fn return_to_rework(task: &Task, rationale: Option<&str>, actor: &str, now: Timestamp) -> CoreResult<Mutation> {
    require_state(task, &[TaskState::Qa], "return", CoreError::hint("QA results can be returned only in QA"))?;
    Ok(transition(task, TaskState::Rework, event_kind::QUALITY_RETURNED, actor, now, json!({ (json_field::RATIONALE): rationale })))
}

/// A role raises a question. Blocking questions move the task to
/// WAITING_FOR_INPUT; the stage is remembered for restoration on answer.
pub fn raise_question(
    task: &Task,
    question_id: Id,
    blocking: bool,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    let payload = json!({ "question_id": question_id.to_string(), "blocking": blocking });
    if !blocking {
        return Ok(note(task, event_kind::QUESTION_RAISED, actor, now, payload));
    }
    require_state(task, &ACTIVE_STATES, "raise-blocking-question", CoreError::hint("only an active stage can wait for input"))?;
    Ok(transition(task, TaskState::WaitingForInput, event_kind::QUESTION_RAISED, actor, now, payload))
}

pub fn answer_question(task: &Task, question_id: Id, actor: &str, now: Timestamp) -> CoreResult<Mutation> {
    require_state(task, &[TaskState::WaitingForInput], "answer", CoreError::hint("the task is not waiting for input"))?;
    let to = task.return_state.ok_or_else(|| CoreError::guard("WAITING_FOR_INPUT without a remembered stage is a state corruption"))?;
    let mut m = transition(task, to, event_kind::QUESTION_ANSWERED, actor, now, json!({ "question_id": question_id.to_string() }));
    m.task.return_state = None;
    Ok(m)
}

/// The response-form correction loop exhausted its configured attempts
/// (OQ-FG-08, ratified): the task parks in WAITING_FOR_INPUT on a blocking
/// question carrying the FULL violation list of the last attempt — a bare
/// FAILED leaves the operator no "what to fix" list. Same loudness discipline
/// as the empty-rework gate (REQ-RG-02 precedent): the parked state is a real,
/// answerable question persisted in the SAME mutation/transaction, never a
/// log-only exit. The workflow effect of a form exhaustion is exactly this
/// park — the stage itself is replayed by the next `advance` once answered.
pub fn escalate_form_rejection(
    task: &Task,
    question_id: Id,
    role_slug: &str,
    attempts: u32,
    violations: &[String],
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(task, &ACTIVE_STATES, "escalate-form-rejection", CoreError::hint("only an active stage can park on a form escalation"))?;
    let bullet_list = violations.iter().map(|v| format!("\n  - {v}")).collect::<String>();
    let mut m = transition(
        task,
        TaskState::WaitingForInput,
        event_kind::QUESTION_RAISED,
        actor,
        now,
        json!({
            (json_field::QUESTION_ID): question_id.to_string(),
            "blocking": true,
            (json_field::GATE): "response-form-loop",
            (json_field::ROLE): role_slug,
            (json_field::ATTEMPT): attempts,
        }),
    );
    m.events.push(EventDraft {
        kind: event_kind::EXECUTION_FORM_ESCALATED,
        actor: actor.to_string(),
        payload: json!({
            (json_field::ROLE): role_slug,
            (json_field::ATTEMPT): attempts,
            (json_field::VIOLATIONS): violations,
            (json_field::QUESTION_ID): question_id.to_string(),
        }),
    });
    m.questions.push(QuestionDraft {
        id: question_id,
        text: format!(
            "Role `{role_slug}` failed its reply-shape contract gate on ALL {attempts} attempt(s) of the response-form correction loop — the task is parked, nothing was published. Full violation list of the last attempt:{bullet_list}\n\nFix the FORM source (the stored contract, the role prompt, or `config/workflow/response-form-loop.yaml`) and answer this question, then re-run `devai task run`: the remembered stage replays from the answer. This is protocol noise, not a content judgement — do NOT count it as the role failing its task.",
        ),
        context: Some(format!("gate=response-form-loop role={role_slug} attempts={attempts}")),
        options: vec![
            "fix contract/prompt/config and re-run this role".into(),
            "answer with correction guidance for the next attempt".into(),
        ],
        blocking: true,
    });
    Ok(m)
}

// ---------------------------------------------------------------------------
// holds: pause / block / cancel / fail
// ---------------------------------------------------------------------------

pub fn pause(task: &Task, reason: Option<&str>, actor: &str, now: Timestamp) -> CoreResult<Mutation> {
    require_state(task, &ACTIVE_STATES, "pause", CoreError::hint("cannot pause a terminal or already-held task"))?;
    Ok(transition(task, TaskState::Paused, event_kind::TASK_PAUSED, actor, now, json!({ "reason": reason })))
}

pub fn block(task: &Task, reason: Option<&str>, actor: &str, now: Timestamp) -> CoreResult<Mutation> {
    require_state(task, &ACTIVE_STATES, "block", CoreError::hint("cannot block a terminal or already-held task"))?;
    Ok(transition(task, TaskState::Blocked, event_kind::TASK_BLOCKED, actor, now, json!({ "reason": reason })))
}

pub fn cancel(task: &Task, reason: Option<&str>, actor: &str, now: Timestamp) -> CoreResult<Mutation> {
    require_state(task, &ACTIVE_STATES, "cancel", CoreError::hint("cannot cancel a terminal or already-held task"))?;
    Ok(transition(task, TaskState::Cancelled, event_kind::TASK_CANCELLED, actor, now, json!({ "reason": reason })))
}

/// Releases a hold state (PAUSED / BLOCKED / FAILED → remembered stage).
/// REQ-03: an open `blocks` dependency forbids the release; the refusal
/// carries the blocking task ids as data. The human-escape of RATIFIED
/// DECISION OQ2 is the only exception — an explicit `dependency_override`
/// decision covering every open blocker, read from stored decisions.
/// RATIFIED DECISION OQ2 also FORBIDS auto-unpause: nothing in this
/// function (or anywhere else in the platform) releases a hold state when
/// the last blocker closes — resume stays a human act until a human issues
/// it, the platform never decides on the operator's behalf.
pub fn resume(task: &Task, deps: &[TaskDependency], decisions: &[Decision], actor: &str, now: Timestamp) -> CoreResult<Mutation> {
    require_state(
        task,
        &[TaskState::Paused, TaskState::Blocked, TaskState::Failed],
        "resume",
        CoreError::hint("nothing to resume: task is not PAUSED/BLOCKED/FAILED"),
    )?;
    let blockers = effective_blockers(task, deps, decisions);
    if !blockers.is_empty() {
        return Err(CoreError::OpenBlockers { blockers });
    }
    let to = task.return_state.ok_or_else(|| CoreError::guard("hold state without a remembered stage is a state corruption"))?;
    let mut m = transition(task, to, event_kind::TASK_RESUMED, actor, now, json!({}));
    m.task.return_state = None;
    Ok(m)
}

pub fn fail(task: &Task, reason: Option<&str>, actor: &str, now: Timestamp) -> CoreResult<Mutation> {
    require_state(task, &ACTIVE_STATES, "fail", CoreError::hint("only an active stage can fail"))?;
    Ok(transition(task, TaskState::Failed, event_kind::TASK_FAILED, actor, now, json!({ "reason": reason })))
}

// ---------------------------------------------------------------------------
// role execution guards
// ---------------------------------------------------------------------------

/// States in which a role with the given slug may be executed. Central
/// enforcement of role/stage separation: an external agent cannot run an
/// Implementation execution while the task awaits approval, etc.
pub fn role_allowed_states(slug: &str) -> &'static [TaskState] {
    match slug {
        "project-intelligence" => ACTIVE_STATES,
        "requirements-analyst" => &[TaskState::Analysis, TaskState::Review],
        "requirements-reviewer" | "architecture-reviewer" => &[TaskState::Review],
        "implementation-agent" => &[TaskState::Implementation, TaskState::Rework],
        "quality-auditor" | "quality-aggregator" => &[TaskState::Qa],
        _ => ACTIVE_STATES, // custom/extension roles default to active stages
    }
}

pub fn check_execution_allowed(slug: &str, task: &Task) -> CoreResult<()> {
    let allowed = role_allowed_states(slug);
    if allowed.contains(&task.state) {
        Ok(())
    } else {
        Err(CoreError::InvalidTransition {
            op: "run-role-execution",
            from: task.state.as_str().to_string(),
            hint: Some(format!(
                "role `{slug}` may only run in {}",
                allowed.iter().map(|s| s.as_str()).collect::<Vec<_>>().join(", ")
            )),
        })
    }
}

/// Output artifact kinds of a role and the workflow effect they trigger.
pub fn execution_output_effect(slug: &str, kind: ArtifactKind) -> Option<&'static str> {
    match (slug, kind) {
        ("requirements-analyst", ArtifactKind::RequirementsSpec) => Some("requirements_published"),
        ("requirements-reviewer", ArtifactKind::ReviewReport) => Some("review_recorded"),
        ("architecture-reviewer", ArtifactKind::ReviewReport) => Some("review_recorded"),
        ("implementation-agent", ArtifactKind::ImplementationReport) => Some("implementation_reported"),
        ("quality-auditor", ArtifactKind::QualityReport) | ("quality-aggregator", ArtifactKind::QualityReport) => Some("qa_report"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// staleness propagation over the artifact dependency graph
// ---------------------------------------------------------------------------

/// Transitive dependents of `roots` that must be marked STALE. Formal
/// artifacts (and work checkpoints) downstream of a superseded result become
/// stale; caches are recoverable and stay as-is. Nothing is deleted.
pub fn stale_closure(artifacts: &[Artifact], roots: &[Id]) -> Vec<Id> {
    let mut out = Vec::new();
    let mut frontier: Vec<Id> = roots.to_vec();
    let mut seen: Vec<Id> = roots.to_vec();
    while let Some(id) = frontier.pop() {
        for a in artifacts {
            if a.status != ArtifactStatus::Current {
                continue;
            }
            if a.kind == ArtifactKind::RequirementsSpec || a.category == DataCategory::Cache {
                continue;
            }
            if seen.contains(&a.id) {
                continue;
            }
            if a.depends_on.contains(&id) {
                seen.push(a.id);
                out.push(a.id);
                frontier.push(a.id);
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// status projection
// ---------------------------------------------------------------------------

/// The forward pipeline spine in domain order (the declared `TaskState`
/// order, types.rs): the stages a task moves through toward acceptance.
/// Owned by the engine so display layers draw position from domain data and
/// never from a CLI-local copy of the sequence (REQ-PROG-03; a step toward
/// action-discovery in the roadmap API seam). Orthogonal states are not on
/// the spine: REWORK is the QA return loop, the hold family waits on a
/// release, CANCELLED ends the cycle off-spine.
pub fn pipeline_order() -> &'static [TaskState] {
    &[
        TaskState::Draft,
        TaskState::Analysis,
        TaskState::Review,
        TaskState::Approval,
        TaskState::Implementation,
        TaskState::Qa,
        TaskState::Accepted,
    ]
}

/// The spine state the task is positioned at for display:
/// - an on-spine state is its own position (ACCEPTED = the final node);
/// - REWORK sits in the QA return loop: it re-enters QA via an
///   implementation report (`implementation_reported` accepts Rework → Qa);
/// - a hold state (WAITING_FOR_INPUT / PAUSED / BLOCKED / FAILED) is
///   positioned at the stage its `return_state` releases back to;
/// - CANCELLED ends the cycle off-spine: no position.
pub fn pipeline_position(task: &Task) -> Option<TaskState> {
    let order = pipeline_order();
    if order.contains(&task.state) {
        return Some(task.state);
    }
    match task.state {
        TaskState::Rework => Some(TaskState::Qa),
        s if s.is_hold() => task.return_state,
        _ => None,
    }
}

/// True when the task is off the spine but the workflow re-enters it
/// (REWORK loop or a hold state): the renderer marks the re-entry stage,
/// not the current state, and must say so (REQ-PROG-02).
pub fn pipeline_off_spine(task: &Task) -> bool {
    !pipeline_order().contains(&task.state) && !task.state.is_terminal()
}

/// Which operations the platform currently permits for the task — the
/// answer to `GET /tasks/{id}/status`: the next allowed domain operations.
pub fn allowed_ops(task: &Task) -> Vec<&'static str> {
    let mut v: Vec<&'static str> = match task.state {
        TaskState::Draft => vec!["start"],
        TaskState::Analysis => vec!["publish-requirements", "run-role-execution(requirements-analyst)", "raise-question"],
        TaskState::Review => vec!["record-review", "run-role-execution(*-reviewer)", "revise", "raise-question"],
        TaskState::Approval => vec!["approve", "reject", "revise"],
        TaskState::Implementation => vec!["publish-implementation-report", "run-role-execution(implementation-agent)", "revise", "raise-question"],
        TaskState::Qa => vec!["accept", "return", "run-role-execution(quality-auditor)", "run-role-execution(quality-aggregator)", "revise"],
        // REQ-RG-05: the empty-rework gate's loud primitive is the blocking
        // question, so the REWORK row of this table names it (the gate itself
        // raises it at publication; the listing is the operator's surface).
        TaskState::Rework => vec!["publish-implementation-report", "run-role-execution(implementation-agent)", "revise", "raise-question"],
        TaskState::WaitingForInput => vec!["answer"],
        TaskState::Paused | TaskState::Failed => vec!["resume"],
        TaskState::Blocked => vec!["resume"],
        // The merge gate (REQ-WT-07) is deliberately NOT a workflow
        // operation: merging the task's `rework/<task-id>` branch into the
        // baseline is a repository act performed by the human `devai task
        // merge` command — available via the CLI at any time, recorded as a
        // decision + event, and moving no task state. An ACCEPTED task
        // awaits nothing from it, and no state "passes" by merging.
        TaskState::Accepted => vec![],
        TaskState::Cancelled => vec![],
    };
    if !task.state.is_terminal() && !task.state.is_hold() {
        v.push("pause");
        v.push("block");
        v.push("cancel");
    }
    v
}
