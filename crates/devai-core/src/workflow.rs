//! Workflow state machine and sovereignty rules.
//!
//! Pure domain logic: every function takes the current task (and whatever
//! domain data the operation inspects), validates the operation against the
//! workflow, and returns a [`Mutation`] — the next task state plus the
//! immutable events to append. No IO happens here.

use serde_json::json;

use crate::error::{CoreError, CoreResult};
use crate::process::{HoldKind, Process, ProcessTransition, Target, process_condition};
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

fn require_state(
    task: &Task,
    allowed: &[TaskState],
    op: &'static str,
    hint: Option<String>,
) -> CoreResult<()> {
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

/// The edges the platform CODE can walk — `(from, condition)` pairs, WITHOUT
/// targets: the target is the declaration's business (REQ-AR-09). This table
/// is the code's half of the contract, and `Process::validate` refuses a
/// declaration that does not cover it, so a drift between the two is an init
/// refusal rather than a silent preference for one side.
pub fn emitted_edges() -> &'static [(TaskState, &'static str)] {
    const EDGES: &[(TaskState, &'static str)] = &[
        (TaskState::Draft, process_condition::HUMAN_START),
        (TaskState::Analysis, process_condition::SPEC_PUBLISHED),
        (TaskState::Review, process_condition::SPEC_PUBLISHED),
        (TaskState::Approval, process_condition::SPEC_PUBLISHED),
        (TaskState::Implementation, process_condition::SPEC_PUBLISHED),
        (TaskState::Qa, process_condition::SPEC_PUBLISHED),
        (TaskState::Rework, process_condition::SPEC_PUBLISHED),
        (TaskState::Review, process_condition::REVIEW_FINISHED),
        (TaskState::Review, process_condition::REVIEW_REJECTED),
        (TaskState::Approval, process_condition::HUMAN_APPROVED),
        (TaskState::Approval, process_condition::HUMAN_REJECTED),
        (
            TaskState::Implementation,
            process_condition::IMPLEMENTATION_REPORTED,
        ),
        (
            TaskState::Rework,
            process_condition::IMPLEMENTATION_REPORTED,
        ),
        (TaskState::Qa, process_condition::HUMAN_ACCEPTED),
        (TaskState::Qa, process_condition::HUMAN_RETURNED),
        (TaskState::WaitingForInput, process_condition::HUMAN_RESUMED),
        (TaskState::Paused, process_condition::HUMAN_RESUMED),
        (TaskState::Blocked, process_condition::HUMAN_RESUMED),
        (TaskState::Failed, process_condition::HUMAN_RESUMED),
        (TaskState::Analysis, process_condition::LAUNCH_FAILED),
        (TaskState::Review, process_condition::LAUNCH_FAILED),
        (TaskState::Implementation, process_condition::LAUNCH_FAILED),
        (TaskState::Rework, process_condition::LAUNCH_FAILED),
        (TaskState::Qa, process_condition::LAUNCH_FAILED),
    ];
    EDGES
}

/// The declared target of `on` from the task's current state. `RETURN_STATE`
/// resolves to the remembered stage of the hold being released; a hold with no
/// remembered stage is state corruption, not a silent self-transition.
fn declared_target(process: &Process, task: &Task, on: &str) -> CoreResult<TaskState> {
    match process.target(task.state.as_str(), on)? {
        Target::State(to) => Ok(to),
        Target::ReturnState => task.return_state.ok_or_else(|| {
            CoreError::guard(format!(
                "{on}: the declaration sends `{}` back to its remembered stage, and none is recorded",
                task.state.as_str()
            ))
        }),
    }
}

/// Builds a mutation that walks the DECLARED edge `(task.state, on)` and
/// appends one event. This is the only place a process state changes: the
/// address comes from [`Process`], never from this module.
fn transition(
    process: &Process,
    task: &Task,
    on: &'static str,
    kind: &'static str,
    actor: &str,
    now: Timestamp,
    mut payload: serde_json::Value,
) -> CoreResult<Mutation> {
    let to = declared_target(process, task, on)?;
    let from = task.state;
    let mut next = task.clone();
    next.state = to;
    next.updated_at = now;
    next.lock_version += 1;
    next.return_state = to.is_hold().then_some(from);
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("from".into(), json!(from.as_str()));
        obj.insert("to".into(), json!(to.as_str()));
    }
    Ok(Mutation {
        task: next,
        events: vec![EventDraft {
            kind,
            actor: actor.to_string(),
            payload,
        }],
        artifact_status: Vec::new(),
        questions: Vec::new(),
    })
}

/// Builds a mutation that parks the task in the DECLARED address of a custody
/// hold (`process.holds`). Same body shape as a transition — the address is
/// data here too, so no command assigns a hold state from code.
fn hold(
    process: &Process,
    task: &Task,
    kind: HoldKind,
    event_kind: &'static str,
    actor: &str,
    now: Timestamp,
    mut payload: serde_json::Value,
) -> CoreResult<Mutation> {
    let to = process.hold_target(kind)?;
    let from = task.state;
    let mut next = task.clone();
    next.state = to;
    next.updated_at = now;
    next.lock_version += 1;
    next.return_state = to.is_hold().then_some(from);
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("from".into(), json!(from.as_str()));
        obj.insert("to".into(), json!(to.as_str()));
    }
    Ok(Mutation {
        task: next,
        events: vec![EventDraft {
            kind: event_kind,
            actor: actor.to_string(),
            payload,
        }],
        artifact_status: Vec::new(),
        questions: Vec::new(),
    })
}

/// The declared edge list of one process, as `from → on → to` triples — the
/// shape the init report and the declaration tests read.
pub fn declared_edges(process: &Process) -> Vec<ProcessTransition> {
    process.transitions.clone()
}

/// Builds a mutation for an event that does not change the workflow state
/// (the lock version still advances: something happened to the aggregate).
fn note(
    task: &Task,
    kind: &'static str,
    actor: &str,
    now: Timestamp,
    payload: serde_json::Value,
) -> Mutation {
    let mut next = task.clone();
    next.updated_at = now;
    next.lock_version += 1;
    Mutation {
        task: next,
        events: vec![EventDraft {
            kind,
            actor: actor.to_string(),
            payload,
        }],
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

/// Typed outcome of reading the signboard field out of a stored CURRENT
/// spec body (QA R1, rel/error-context): «does not read as JSON» and «no
/// name in the body» are DIFFERENT facts. The second is the legal absence
/// a @1..@3 legacy body means; the first is a store-integrity failure that
/// no door may fold into the absence branch — collapsing them is what
/// let a swallowed read error keep the previous signboard silently.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SpecNameRead {
    /// The field is present and non-blank — the name this publication
    /// stamps.
    Named(String),
    /// The body parses as JSON but names no usable signboard (missing
    /// field, non-string, blank): the publication says NOTHING about the
    /// signboard, the stored one is kept (REQ-TN-02 AC3).
    Absent,
    /// The bytes are not parseable JSON — corruption, not absence.
    Corrupt,
}

/// The typed half: a door that RE-READS a stored body uses this so an
/// integrity failure stays distinguishable from the legal absence
/// (QA R1); doors handed bytes that already passed the contract gate use
/// the [`human_name_from_spec`] wrapper below.
pub fn read_human_name_from_spec(content: &[u8], field: &str) -> SpecNameRead {
    let Ok(value) = serde_json::from_slice::<serde_json::Value>(content) else {
        return SpecNameRead::Corrupt;
    };
    let Some(raw) = value.get(field).and_then(|v| v.as_str()) else {
        return SpecNameRead::Absent;
    };
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return SpecNameRead::Absent;
    }
    SpecNameRead::Named(trimmed.to_string())
}

/// REQ-TN-02: read the human name out of a published CURRENT spec BODY.
/// The body arrives already contract-gated (`task_name` is REQUIRED by the
/// @4 schema, the mechanical window detector already rejected anything the
/// project template disallows), so this half is a pure read: the field
/// named by the loaded `config/workflow/task-name.yaml` policy, trimmed,
/// returned when non-blank. A body with no such field returns `None` — the
/// obligation is NOT retroactive (REQ-TN-02 AC3), absence means "this
/// publication says nothing about the signboard", never "clear it". The
/// `Corrupt` outcome maps to `None` HERE because every caller of this
/// wrapper holds bytes that just passed the JSON-Schema gate; a door that
/// reads the body back from the store uses [`read_human_name_from_spec`]
/// instead and refuses on `Corrupt` (QA R1).
pub fn human_name_from_spec(content: &[u8], field: &str) -> Option<String> {
    match read_human_name_from_spec(content, field) {
        SpecNameRead::Named(v) => Some(v),
        SpecNameRead::Absent | SpecNameRead::Corrupt => None,
    }
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
        .filter(|d| {
            d.task_id == task.id
                && d.kind == DependencyKind::Blocks
                && d.depends_on_state != TaskState::Accepted
        })
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
pub fn effective_blockers(
    task: &Task,
    deps: &[TaskDependency],
    decisions: &[Decision],
) -> Vec<String> {
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

pub fn start(process: &Process, task: &Task, actor: &str, now: Timestamp) -> CoreResult<Mutation> {
    require_state(
        task,
        &[TaskState::Draft],
        "start",
        CoreError::hint("task is created but its workflow not started"),
    )?;
    transition(
        process,
        task,
        process_condition::HUMAN_START,
        event_kind::WORKFLOW_STARTED,
        actor,
        now,
        json!({}),
    )
}

/// A new Requirements Specification version was published (by the Analyst
/// role or by the user via `revise`). The spec artifact must already exist in
/// `artifacts` with `status = CURRENT`. Supersedes previous versions, marks
/// dependent results stale and returns the task to REVIEW.
///
/// `human_name` is the signboard read out of the published CURRENT spec BODY
/// (REQ-TN-02: the analyst produces it as a required contract field, every
/// publication path — engine run, manual `task revise`, `artifact advance` —
/// extracts it with the ONE helper `human_name_from_spec` and hands it HERE;
/// this command is the single writer of the field on publication). `Some(v)`
/// REPLACES the stored name wholesale — a rework spec re-signs the task from
/// its own body, never a delta (REQ-TN-04, ratified OQ-HP-4 idiom); `None`
/// (a spec published under @1..@3, no name in the body) keeps the previous
/// name untouched — the obligation is not retroactive (REQ-TN-02 AC3). The
/// title is never touched (REQ-TN-01 AC2). The name lands on the task row
/// inside the SAME commit as the transition and its event.
pub fn requirements_published(
    process: &Process,
    task: &Task,
    artifacts: &[Artifact],
    reason: Option<&str>,
    human_name: Option<&str>,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(
        task,
        &[
            TaskState::Analysis,
            TaskState::Review,
            TaskState::Approval,
            TaskState::Implementation,
            TaskState::Qa,
            TaskState::Rework,
        ],
        if task.requirements_seq == 0 {
            "publish-requirements"
        } else {
            "revise"
        },
        CoreError::hint(
            "run `devai task start` first; a specification can only be published while the task is in ANALYSIS/REVIEW/APPROVAL/IMPLEMENTATION/QA/REWORK",
        ),
    )?;
    let Some(spec) = current_spec(artifacts) else {
        return Err(CoreError::guard(
            "no CURRENT REQUIREMENTS_SPEC artifact found",
        ));
    };

    let is_revise = task.requirements_seq > 0;
    // REQ-TN-02/04: the name this publication writes. Absence keeps the
    // previous stored value (a legacy-body publication does not clear a
    // signboard); presence replaces it whole.
    let name_now: Option<&str> = human_name.or(task.human_name.as_deref());
    let mut m = transition(
        process,
        task,
        process_condition::SPEC_PUBLISHED,
        if is_revise {
            event_kind::REQUIREMENTS_REVISED
        } else {
            event_kind::REQUIREMENTS_PUBLISHED
        },
        actor,
        now,
        json!({
            (json_field::REQUIREMENTS_SEQ): spec.seq,
            (json_field::SPEC_ID): spec.id.to_string(),
            (json_field::REASON): reason,
            (json_field::HUMAN_NAME): name_now,
        }),
    )?;
    m.task.requirements_seq = spec.seq;
    m.task.human_name = name_now.map(|s| s.to_string());

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
    for a in predecessors
        .iter()
        .filter(|a| a.status != ArtifactStatus::Superseded)
    {
        m.artifact_status.push((a.id, ArtifactStatus::Superseded));
    }

    // Downstream results of the superseded specification become STALE.
    if is_revise {
        let staled = stale_closure(artifacts, &superseded);
        if !staled.is_empty() {
            m.artifact_status
                .extend(staled.iter().map(|id| (*id, ArtifactStatus::Stale)));
            m.events.push(EventDraft {
                kind: event_kind::ARTIFACTS_STALED,
                actor: actor.to_string(),
                payload: json!({ (json_field::STALED): staled.iter().map(|i| i.to_string()).collect::<Vec<_>>(), (json_field::CAUSE): stale_cause::REQUIREMENTS_REVISION }),
            });
        }
    }
    Ok(m)
}

pub fn review_recorded(
    process: &Process,
    task: &Task,
    verdict: Verdict,
    summary: Option<&str>,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(
        task,
        &[TaskState::Review],
        "record-review",
        CoreError::hint("an independent review applies only in REVIEW"),
    )?;
    // The verdict names the declared edge: a go verdict finishes the review,
    // anything else is a rejection — «куда» is the declaration's answer.
    let condition = match verdict {
        Verdict::Approved => process_condition::REVIEW_FINISHED,
        _ => process_condition::REVIEW_REJECTED,
    };
    transition(
        process,
        task,
        condition,
        event_kind::REVIEW_RECORDED,
        actor,
        now,
        json!({ (json_field::VERDICT): verdict.as_str(), (json_field::SUMMARY): summary }),
    )
}

pub fn approve_requirements(
    process: &Process,
    task: &Task,
    rationale: Option<&str>,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(
        task,
        &[TaskState::Approval],
        "approve",
        CoreError::hint(
            "implementation may only start after requirements were independently reviewed and approved here",
        ),
    )?;
    if task.requirements_seq == 0 {
        return Err(CoreError::guard("no requirements version to approve"));
    }
    transition(
        process,
        task,
        process_condition::HUMAN_APPROVED,
        event_kind::REQUIREMENTS_APPROVED,
        actor,
        now,
        json!({
            "requirements_seq": task.requirements_seq,
            "rationale": rationale,
        }),
    )
}

pub fn reject_requirements(
    process: &Process,
    task: &Task,
    rationale: Option<&str>,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(
        task,
        &[TaskState::Approval],
        "reject",
        CoreError::hint("requirements can be rejected only while in APPROVAL"),
    )?;
    transition(
        process,
        task,
        process_condition::HUMAN_REJECTED,
        event_kind::REQUIREMENTS_REJECTED,
        actor,
        now,
        json!({ (json_field::RATIONALE): rationale }),
    )
}

/// R5 QA (rl/error-handling/rel/error-context): the TYPED outcome of
/// reading one stored report body — the honest vocabulary the gate records
/// when a side cannot be compared. A missing blob and a corrupt (unparse-
/// able) body were previously collapsed into the same silent `None` as a
/// readable body without a `commit` field; the gate's SEMANTICS stay
/// unchanged (REQ-RG-04: never guess, proceed incomparable), only the
/// record becomes distinguishable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReportBodyRead {
    /// The body read and parsed; the optional commit is what it names.
    Readable,
    /// The blob bytes are absent/unreadable from the store.
    Missing,
    /// The blob bytes exist but are not parseable JSON — corruption.
    Corrupt,
}

/// One previous IMPLEMENTATION_REPORT as the producer read it from stored
/// values: identity + version order + the platform-stamped `commit` the body
/// carries (engine-side stamp of implementation-report@2). `None` commit means
/// "this report names no comparable commit" (read-only producer, pre-@2 body,
/// manual publication without the field) — a fact the gate must SEE, not guess.
/// `body` says WHY the commit could be absent (R5): readable-but-unnamed,
/// missing, or corrupt.
#[derive(Clone, Debug)]
pub struct PriorImplementationReport {
    pub artifact_id: Id,
    pub seq: u32,
    pub commit: Option<String>,
    pub body: ReportBodyRead,
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
    /// R5 QA (rl/error-handling): WHY the current commit could be absent —
    /// the typed read outcome of the same body read that produced
    /// `current_commit` (readable-but-unnamed / missing / corrupt).
    pub current_body: ReportBodyRead,
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
        Self {
            summary: None,
            current_report,
            current_commit: None,
            current_body: ReportBodyRead::Readable,
            priors: Vec::new(),
            ratifications: Vec::new(),
            qa_return_rationale: None,
        }
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
    process: &Process,
    task: &Task,
    facts: &ImplementationReportFacts,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(
        task,
        &[TaskState::Implementation, TaskState::Rework],
        "publish-implementation-report",
        CoreError::hint("implementation reports are recorded in IMPLEMENTATION or REWORK"),
    )?;

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
                                && d.refs
                                    .iter()
                                    .any(|r| *r == facts.current_report.to_string())
                        });
                        match ratified {
                            None => {
                                return empty_rework_trip(
                                    process, task, facts, prior, p, c, actor, now,
                                );
                            }
                            Some(d) => {
                                payload[json_field::GATE] = json!("empty-rework");
                                payload[json_field::VERDICT] = json!("IDENTICAL_RATIFIED");
                                payload[json_field::PREV_REPORT] =
                                    json!(prior.artifact_id.to_string());
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
                        // R5 QA (rl/error-handling/rel/error-context): the
                        // record now NAMES WHY the commit could not be read —
                        // a corrupt body and an absent blob are distinct
                        // facts, no longer collapsed into the silent same
                        // phrase as a readable body that names no commit.
                        // Semantics unchanged: never guess, proceed
                        // incomparable.
                        let side: &'static str = match (prior.body, facts.current_body) {
                            (ReportBodyRead::Corrupt, ReportBodyRead::Corrupt) => {
                                "both report bodies are corrupt (unreadable as JSON)"
                            }
                            (ReportBodyRead::Corrupt, _) => {
                                "the previous report body is corrupt (unreadable as JSON)"
                            }
                            (_, ReportBodyRead::Corrupt) => {
                                "the published report body is corrupt (unreadable as JSON)"
                            }
                            (ReportBodyRead::Missing, ReportBodyRead::Missing) => {
                                "neither report body is present in the store"
                            }
                            (ReportBodyRead::Missing, _) => {
                                "the previous report body is absent from the store"
                            }
                            (_, ReportBodyRead::Missing) => {
                                "the published report body is absent from the store"
                            }
                            (ReportBodyRead::Readable, ReportBodyRead::Readable) => {
                                if prev_sha.is_none() && cur_sha.is_none() {
                                    "neither report names a stamped commit"
                                } else if prev_sha.is_none() {
                                    "the previous report names no stamped commit"
                                } else {
                                    "the published report names no stamped commit"
                                }
                            }
                        };
                        incomparable = Some((side, Some(prior.artifact_id.to_string())));
                    }
                }
            }
        }
    }

    let mut m = transition(
        process,
        task,
        process_condition::IMPLEMENTATION_REPORTED,
        event_kind::IMPLEMENTATION_REPORTED,
        actor,
        now,
        payload,
    )?;
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
    process: &Process,
    task: &Task,
    facts: &ImplementationReportFacts,
    prior: &PriorImplementationReport,
    prev_sha: &str,
    cur_sha: &str,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    let question_id = new_id();
    let mut m = hold(
        process,
        task,
        HoldKind::Question,
        event_kind::QUESTION_RAISED,
        actor,
        now,
        json!({
            (json_field::QUESTION_ID): question_id.to_string(),
            "blocking": true,
            (json_field::GATE): "empty-rework",
        }),
    )?;
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
    m.questions.push(empty_rework_question(
        task,
        facts,
        prior,
        prev_sha,
        cur_sha,
        question_id,
    ));
    Ok(m)
}

/// The answerable half of the trip (REQ-RG-02): the operator is told WHAT was
/// skipped, both commits, the QA return that opened the cycle and the one
/// command that can legitimately ratify a no-op.
fn empty_rework_question(
    task: &Task,
    facts: &ImplementationReportFacts,
    prior: &PriorImplementationReport,
    prev_sha: &str,
    cur_sha: &str,
    question_id: Id,
) -> QuestionDraft {
    let rationale = facts
        .qa_return_rationale
        .as_deref()
        .unwrap_or("(rationale not stored)");
    QuestionDraft {
        id: question_id,
        text: format!(
            "REWORK publication {} names commit {cur_sha} — the SAME commit as report {} \
             ({prev_sha}): the rework captured no new changes. QA returned with: {rationale}. If \
             the points were genuinely already satisfied, the operator may ratify the no-op \
             explicitly: `devai task ratify-empty-rework {} --rationale \"why no fixes were \
             required\"` (a stored human decision — the QA transition will cite it). Otherwise \
             run the implementation-agent with the actual fixes and publish again.",
            facts.current_report, prior.artifact_id, task.id,
        ),
        context: Some(format!(
            "gate=empty-rework prev={} current={} commit={cur_sha}",
            prior.artifact_id, facts.current_report
        )),
        options: vec![
            "ratify-empty-rework (no fixes were required)".into(),
            "re-run implementation with fixes".into(),
        ],
        blocking: true,
    }
}

pub fn accept(
    process: &Process,
    task: &Task,
    artifacts: &[Artifact],
    deps: &[TaskDependency],
    decisions: &[Decision],
    rationale: Option<&str>,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(
        task,
        &[TaskState::Qa],
        "accept",
        CoreError::hint("acceptance applies only in QA"),
    )?;
    // REQ-03: the human gate cannot pass a task an open blocker still
    // holds; the refusal enumerates the blocking ids. The only release is
    // the human's own `dependency_override` decision (ratified OQ2) —
    // checked here against the same stored decisions the QA auditor reads.
    let blockers = effective_blockers(task, deps, decisions);
    if !blockers.is_empty() {
        return Err(CoreError::OpenBlockers { blockers });
    }
    require_current_quality_report(artifacts)?;
    Ok(transition(
        process,
        task,
        process_condition::HUMAN_ACCEPTED,
        event_kind::TASK_ACCEPTED,
        actor,
        now,
        json!({ "rationale": rationale }),
    )?)
}

/// Acceptance is not a formality: an independent, CURRENT quality report is
/// the platform's evidence that somebody else checked the work.
fn require_current_quality_report(artifacts: &[Artifact]) -> CoreResult<()> {
    if artifacts
        .iter()
        .any(|a| a.kind == ArtifactKind::QualityReport && a.status == ArtifactStatus::Current)
    {
        return Ok(());
    }
    Err(CoreError::guard(
        "acceptance requires a CURRENT QUALITY_REPORT artifact: checks must be independent of the implementation",
    ))
}

pub fn return_to_rework(
    process: &Process,
    task: &Task,
    artifacts: &[Artifact],
    rationale: Option<&str>,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(
        task,
        &[TaskState::Qa],
        "return",
        CoreError::hint("QA results can be returned only in QA"),
    )?;
    let mut m = transition(
        process,
        task,
        process_condition::HUMAN_RETURNED,
        event_kind::QUALITY_RETURNED,
        actor,
        now,
        json!({ (json_field::RATIONALE): rationale }),
    )?;

    stale_rejected_report(&mut m, artifacts, actor);
    Ok(m)
}

/// Ratified OQ-QH-3: rework is a DATA change, not only a presentation one.
/// The quality report the return just rejected stops being CURRENT — a stale
/// report that still reads as CURRENT is «старьё выглядит решением» at the
/// store level, which no presentation layer could honestly fix. Nothing is
/// deleted: the row keeps its content and moves to STALE until a new audit
/// publishes its successor.
fn stale_rejected_report(m: &mut Mutation, artifacts: &[Artifact], actor: &str) {
    let rejected: Vec<Id> = artifacts
        .iter()
        .filter(|a| a.kind == ArtifactKind::QualityReport && a.status == ArtifactStatus::Current)
        .map(|a| a.id)
        .collect();
    if rejected.is_empty() {
        return;
    }
    m.artifact_status
        .extend(rejected.iter().map(|id| (*id, ArtifactStatus::Stale)));
    m.events.push(EventDraft {
        kind: event_kind::ARTIFACTS_STALED,
        actor: actor.to_string(),
        payload: json!({
            (json_field::STALED): rejected.iter().map(|i| i.to_string()).collect::<Vec<_>>(),
            (json_field::CAUSE): stale_cause::REWORK_RETURN,
        }),
    });
}

/// A role raises a question. Blocking questions move the task to
/// WAITING_FOR_INPUT; the stage is remembered for restoration on answer.
pub fn raise_question(
    process: &Process,
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
    require_state(
        task,
        &ACTIVE_STATES,
        "raise-blocking-question",
        CoreError::hint("only an active stage can wait for input"),
    )?;
    hold(
        process,
        task,
        HoldKind::Question,
        event_kind::QUESTION_RAISED,
        actor,
        now,
        payload,
    )
}

pub fn answer_question(
    process: &Process,
    task: &Task,
    question_id: Id,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(
        task,
        &[TaskState::WaitingForInput],
        "answer",
        CoreError::hint("the task is not waiting for input"),
    )?;
    let mut m = transition(
        process,
        task,
        process_condition::HUMAN_RESUMED,
        event_kind::QUESTION_ANSWERED,
        actor,
        now,
        json!({ "question_id": question_id.to_string() }),
    )?;
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
    process: &Process,
    task: &Task,
    question_id: Id,
    role_slug: &str,
    attempts: u32,
    violations: &[String],
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(
        task,
        &ACTIVE_STATES,
        "escalate-form-rejection",
        CoreError::hint("only an active stage can park on a form escalation"),
    )?;
    let bullet_list = violations
        .iter()
        .map(|v| format!("\n  - {v}"))
        .collect::<String>();
    // Ratified OQ-HP-5: the escalation is the MOST human place in the loop —
    // it must carry a plain-words retelling of WHY the platform refused, not
    // only the formal violation paths. Deterministic template over the
    // machine list (no model): the shape of the failure is classified from
    // the violation texts themselves, which are platform-owned literals.
    let retelling = form_escalation_retelling(role_slug, attempts, violations);
    let mut m = hold(
        process,
        task,
        HoldKind::Question,
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
    )?;
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
            "{retelling}\nRole `{role_slug}` failed its reply-shape contract gate on ALL {attempts} attempt(s) of the response-form correction loop — the task is parked, nothing was published. Full violation list of the last attempt:{bullet_list}\n\nFix the FORM source (the stored contract, the role prompt, or `config/workflow/response-form-loop.yaml`) and answer this question, then re-run `devai task run`: the remembered stage replays from the answer. This is protocol noise, not a content judgement — do NOT count it as the role failing its task.",
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

/// The human retelling of a form exhaustion (ratified OQ-HP-5): simple
/// words, what happened and what changes, BEFORE the formal violation list.
/// Deterministic by construction — categories are classified from the
/// platform's OWN violation literals (adapters/humanize texts), never from
/// a model and never from free-form role output. The bullet list stays
/// below: the paragraph adds the readable half, it hides no machine fact.
fn form_escalation_retelling(role_slug: &str, attempts: u32, violations: &[String]) -> String {
    let mut missing: Vec<&str> = Vec::new();
    let mut paragraph = false;
    let mut mistyped = false;
    let mut not_json = false;
    for v in violations {
        if let Some(path) = v
            .strip_prefix("$.")
            .and_then(|s| s.strip_suffix(": missing required key"))
        {
            missing.push(path);
        } else if v.starts_with("$.")
            && (v.contains("paragraph is")
                || v.contains("explanation slot")
                || v.contains("repeat the formal body")
                || v.contains("formal-protocol marker"))
        {
            paragraph = true;
        } else if v.contains("expected object, got")
            || v.contains("expected string, got")
            || v.contains("expected array, got")
            || v.contains("expected integer, got")
            || v.contains("expected number, got")
            || v.contains("not in enum")
            || v.contains(", minimum")
        {
            // A declared field carries the wrong shape (type/enum/cardinality).
            mistyped = true;
        } else {
            // The whole-reply literals of the gate: not JSON at all, or
            // JSON but not one object (OQ-FG-02).
            not_json = true;
        }
    }
    let mut causes: Vec<String> = Vec::new();
    if !missing.is_empty() {
        causes.push(format!(
            "в ответе не было обязательных полей ({})",
            missing.join(", ")
        ));
    }
    if paragraph {
        causes.push(
            "человеческий абзац не прошёл проверку читаемости (пустая болтовня или пересказ формализма)"
                .into(),
        );
    }
    if mistyped {
        causes.push("часть полей была заполнена не того типа или вне разрешённых значений".into());
    }
    if not_json {
        causes.push("ответ вообще не читался как один JSON-объект".into());
    }
    if causes.is_empty() {
        causes.push("форма ответа не совпала с хранящимся контрактом".into());
    }
    format!(
        "Простыми словами: роль «{role_slug}» {attempts} раз подряд не смогла отдать ответ нужной формы — платформа не сохранила ни одного артефакта и ждёт твоего решения. Отклонена ФОРМА ответа, а не содержание: {}. Полный список формальных причин — ниже. Что изменится после твоего ответа: та же стадия будет сыграна заново с учётом ответа; чинить нужно источник формы (хранящийся контракт, промпт роли или config/workflow) — не работу роли по существу.",
        causes.join("; ")
    )
}

// ---------------------------------------------------------------------------
// holds: pause / block / cancel / fail
// ---------------------------------------------------------------------------

pub fn pause(
    process: &Process,
    task: &Task,
    reason: Option<&str>,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(
        task,
        &ACTIVE_STATES,
        "pause",
        CoreError::hint("cannot pause a terminal or already-held task"),
    )?;
    hold(
        process,
        task,
        HoldKind::Pause,
        event_kind::TASK_PAUSED,
        actor,
        now,
        json!({ "reason": reason }),
    )
}

pub fn block(
    process: &Process,
    task: &Task,
    reason: Option<&str>,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(
        task,
        &ACTIVE_STATES,
        "block",
        CoreError::hint("cannot block a terminal or already-held task"),
    )?;
    hold(
        process,
        task,
        HoldKind::Block,
        event_kind::TASK_BLOCKED,
        actor,
        now,
        json!({ "reason": reason }),
    )
}

pub fn cancel(
    process: &Process,
    task: &Task,
    reason: Option<&str>,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(
        task,
        &ACTIVE_STATES,
        "cancel",
        CoreError::hint("cannot cancel a terminal or already-held task"),
    )?;
    hold(
        process,
        task,
        HoldKind::Cancel,
        event_kind::TASK_CANCELLED,
        actor,
        now,
        json!({ "reason": reason }),
    )
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
pub fn resume(
    process: &Process,
    task: &Task,
    deps: &[TaskDependency],
    decisions: &[Decision],
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
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
    let mut m = transition(
        process,
        task,
        process_condition::HUMAN_RESUMED,
        event_kind::TASK_RESUMED,
        actor,
        now,
        json!({}),
    )?;
    m.task.return_state = None;
    Ok(m)
}

pub fn fail(
    process: &Process,
    task: &Task,
    reason: Option<&str>,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    require_state(
        task,
        &ACTIVE_STATES,
        "fail",
        CoreError::hint("only an active stage can fail"),
    )?;
    hold(
        process,
        task,
        HoldKind::Fail,
        event_kind::TASK_FAILED,
        actor,
        now,
        json!({ "reason": reason }),
    )
}

/// Park a task on a PLATFORM-side stop: a failed launch walks the declared
/// `launch_failed` edge of its state (the declaration says where a failed
/// launch sends each role state); a brake that is not an outcome parks in the
/// declared `brakes.park_state`. Both addresses are data.
pub fn park_on_launch_failure(
    process: &Process,
    task: &Task,
    reason: Option<&str>,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    let declared = process
        .target(task.state.as_str(), process_condition::LAUNCH_FAILED)
        .ok();
    match declared {
        Some(Target::State(_)) => transition(
            process,
            task,
            process_condition::LAUNCH_FAILED,
            event_kind::TASK_BLOCKED,
            actor,
            now,
            json!({ "reason": reason }),
        ),
        _ => {
            let to = process.park_state()?;
            park_at(
                process,
                task,
                to,
                event_kind::TASK_BLOCKED,
                actor,
                now,
                json!({ "reason": reason }),
            )
        }
    }
}

/// Park in the declared brake address regardless of the current state — the
/// shape a tripped brake uses (rounds without a shift, consecutive failures).
pub fn park(
    process: &Process,
    task: &Task,
    reason: Option<&str>,
    actor: &str,
    now: Timestamp,
) -> CoreResult<Mutation> {
    let to = process.park_state()?;
    park_at(
        process,
        task,
        to,
        event_kind::TASK_BLOCKED,
        actor,
        now,
        json!({ "reason": reason }),
    )
}

/// One explicit address for a platform park (already resolved by the caller,
/// so a brake that names its own park state and a declared outcome edge share
/// the same body).
fn park_at(
    _process: &Process,
    task: &Task,
    to: TaskState,
    kind: &'static str,
    actor: &str,
    now: Timestamp,
    mut payload: serde_json::Value,
) -> CoreResult<Mutation> {
    if !ACTIVE_STATES.contains(&task.state) {
        return Err(CoreError::InvalidTransition {
            op: "park",
            from: task.state.as_str().to_string(),
            hint: CoreError::hint("only an active stage can be parked"),
        });
    }
    let from = task.state;
    let mut next = task.clone();
    next.state = to;
    next.updated_at = now;
    next.lock_version += 1;
    next.return_state = Some(from);
    if let Some(obj) = payload.as_object_mut() {
        obj.insert("from".into(), json!(from.as_str()));
        obj.insert("to".into(), json!(to.as_str()));
    }
    Ok(Mutation {
        task: next,
        events: vec![EventDraft {
            kind,
            actor: actor.to_string(),
            payload,
        }],
        artifact_status: Vec::new(),
        questions: Vec::new(),
    })
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
                allowed
                    .iter()
                    .map(|s| s.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
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
        ("implementation-agent", ArtifactKind::ImplementationReport) => {
            Some("implementation_reported")
        }
        ("quality-auditor", ArtifactKind::QualityReport)
        | ("quality-aggregator", ArtifactKind::QualityReport) => Some("qa_report"),
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
        TaskState::Analysis => vec![
            "publish-requirements",
            "run-role-execution(requirements-analyst)",
            "raise-question",
        ],
        TaskState::Review => vec![
            "record-review",
            "run-role-execution(*-reviewer)",
            "revise",
            "raise-question",
        ],
        TaskState::Approval => vec!["approve", "reject", "revise"],
        TaskState::Implementation => vec![
            "publish-implementation-report",
            "run-role-execution(implementation-agent)",
            "revise",
            "raise-question",
        ],
        TaskState::Qa => vec![
            "accept",
            "return",
            "run-role-execution(quality-auditor)",
            "run-role-execution(quality-aggregator)",
            "revise",
        ],
        // REQ-RG-05: the empty-rework gate's loud primitive is the blocking
        // question, so the REWORK row of this table names it (the gate itself
        // raises it at publication; the listing is the operator's surface).
        TaskState::Rework => vec![
            "publish-implementation-report",
            "run-role-execution(implementation-agent)",
            "revise",
            "raise-question",
        ],
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
