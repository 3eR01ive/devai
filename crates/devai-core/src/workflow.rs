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
    }
}

fn current_spec(artifacts: &[Artifact]) -> Option<&Artifact> {
    artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::RequirementsSpec && a.status == ArtifactStatus::Current)
        .max_by_key(|a| a.seq)
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
    Ok(transition(task, to, event_kind::REVIEW_RECORDED, actor, now, json!({ "verdict": verdict.as_str(), "summary": summary })))
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
    Ok(transition(task, TaskState::Analysis, event_kind::REQUIREMENTS_REJECTED, actor, now, json!({ "rationale": rationale })))
}

pub fn implementation_reported(task: &Task, summary: Option<&str>, actor: &str, now: Timestamp) -> CoreResult<Mutation> {
    require_state(task, &[TaskState::Implementation, TaskState::Rework], "publish-implementation-report", CoreError::hint(
        "implementation reports are recorded in IMPLEMENTATION or REWORK",
    ))?;
    Ok(transition(task, TaskState::Qa, event_kind::IMPLEMENTATION_REPORTED, actor, now, json!({ "summary": summary })))
}

pub fn accept(
    task: &Task,
    artifacts: &[Artifact],
    rationale: Option<&str>,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(task, &[TaskState::Qa], "accept", CoreError::hint("acceptance applies only in QA"))?;
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
    Ok(transition(task, TaskState::Rework, event_kind::QUALITY_RETURNED, actor, now, json!({ "rationale": rationale })))
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
pub fn resume(task: &Task, actor: &str, now: Timestamp) -> CoreResult<Mutation> {
    require_state(
        task,
        &[TaskState::Paused, TaskState::Blocked, TaskState::Failed],
        "resume",
        CoreError::hint("nothing to resume: task is not PAUSED/BLOCKED/FAILED"),
    )?;
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
        TaskState::Rework => vec!["publish-implementation-report", "run-role-execution(implementation-agent)", "revise"],
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
