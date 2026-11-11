//! The attempt limit of a role (REQ-AL-01..08): how many COMPLETED executions
//! of ONE ROLE passed WITHOUT a substantive shift, and the declared ceiling.
//!
//! Everything here is DERIVED from stored history — executions, artifacts,
//! workflow events and the SAME convergence slice the read surfaces publish.
//! No counter column exists to drift from the rows that justify it (REQ-AL-02).
//!
//! The counter lives on the (TASK, ROLE) pair (rework R1/R2). A shift of
//! ANOTHER role never zeroes it — a stage move, a verdict and a converging
//! round are read against the role whose work they judge. A round that
//! changes nothing raises it, which is the whole point: `ANALYSIS ↔ REVIEW`
//! and `QA ↔ REWORK` used to reset a task-wide counter on every transition
//! and could therefore run forever.
//!
//! What a shift of one role is (REQ-AL-03, ratified — the score must RISE,
//! «оценка растёт — сходится, не растёт — не сходится»):
//!
//! * a round the stage's own convergence series calls movement (`CONVERGING`)
//!   or a verdict change between two of its rounds — read from the series that
//!   judges THIS role's work (`spec_review` for the analyst and the reviewer,
//!   `quality` for QA and the implementation agent);
//! * a stored verdict change recorded while the task stood in a stage that
//!   series judges — the same substance, read even when no series exists yet;
//! * a stage move out of THIS role's own stage into a state the task had never
//!   reached before. That is the only stage-move form that counts: a ping-pong
//!   re-enters states it has already been in, so ten such rounds add ten
//!   attempts instead of zeroing the counter ten times.
//!
//! WHICH states each series judges is declared in `process.yaml` (`series:`),
//! not in this file (rework R2): a third series is a declaration edit.
//!
//! The floor is the END of the launch that produced the last shift, not the
//! shift's timestamp: the platform publishes the artifact (and the verdict)
//! inside a run and closes the execution afterwards, so a bare timestamp would
//! count the shifting launch itself and leave «1 из 5» right after a real
//! advance (rework R1).
//!
//! A new artifact version with another text and the same verdict is NOT a
//! shift: that is exactly how 50 spec versions and 48 review reports rode as
//! «movement» until a human stopped the task.
//!
//! Entering or leaving a CUSTODY HOLD is not a shift either: a human question
//! is not progress, and releasing the task by hand must not reset a counter
//! that only an answer to the stop question resets (REQ-AL-06).

use chrono::{DateTime, Utc};
use devai_core::types::{
    ExecutionStatus, RoleExecution, Task, TaskState, WorkflowEvent, event_kind, json_field,
};
use devai_runtime::convergence::{self, ConvergenceView, StageConvergence};
use devai_store::config::ProcessPolicy;
use serde::Serialize;

use crate::{Api, ApiError, ApiResult};

/// The measured series a declared working state is judged by, if the
/// declaration measures one (rework R2: the catalog is data, not this file).
fn series_of<'p>(policy: &'p ProcessPolicy, state: TaskState) -> Option<&'p str> {
    policy.series_of(state.as_str())
}

/// Does the declared series `stage` speak for `role` — is any state it judges
/// executed by `role`? ONE reading, shared by the series rounds and by the
/// verdicts, so the two can never attribute a shift to different roles.
fn series_judges(policy: &ProcessPolicy, stage: &str, role: &str) -> bool {
    policy.series_judges(stage, role)
}

/// The ONE triple the stop decision, the refusal, the question and the status
/// line publish: the role, the derived count and the declared limit (`None`
/// = «без предела»). `line` is the exact sentence the operator reads on every
/// surface, so the numbers can never disagree between them (REQ-AL-07 AC1).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct AttemptLimitView {
    pub role: String,
    /// Completed executions of this role after the last substantive shift
    /// (and after the last human reset).
    pub attempts: u32,
    /// The declared limit; `None` means the declaration says «без предела».
    pub limit: Option<u32>,
    /// `попыток без прогресса: N из M` or `… N — без предела`.
    pub line: String,
}

impl AttemptLimitView {
    /// The view of one role: the count, the effective limit and the ONE line.
    pub fn new(role: &str, attempts: u32, limit: Option<u32>) -> AttemptLimitView {
        let line = match limit {
            Some(limit) => format!("попыток без прогресса: {attempts} из {limit}"),
            None => format!("попыток без прогресса: {attempts} — без предела"),
        };
        AttemptLimitView {
            role: role.to_string(),
            attempts,
            limit,
            line,
        }
    }

    /// The refusal/question wording shares this prefix with `line`, so the
    /// stopped task, the question and the status speak the same numbers.
    pub fn phrase(&self) -> String {
        match self.limit {
            Some(limit) => format!(
                "роль `{}`: {} (предел {limit})",
                self.role,
                self.line.trim_start_matches("попыток без прогресса: ")
            ),
            None => format!("роль `{}`: {}", self.role, self.line),
        }
    }
}

/// Whether the count reached the declared limit. `None` = no limit: the rule
/// never stops (and zero is NOT «stop at once», REQ-AL-04 AC2).
pub fn limit_reached(attempts: u32, limit: Option<u32>) -> bool {
    limit.is_some_and(|limit| attempts >= limit)
}

impl Api {
    /// The role whose counter the task carries: the executor of the current
    /// WORKING stage, else the stage remembered by the hold the task sits in.
    /// A hold keeps the number visible (a stopped task still says «5 из 5»),
    /// while a terminal task has none (REQ-AL-08 AC4).
    pub(crate) fn attempt_role(&self, task: &Task, policy: &ProcessPolicy) -> Option<String> {
        if task.state.is_terminal() {
            return None;
        }
        policy
            .role_for(task.state.as_str())
            .or_else(|| {
                task.return_state
                    .and_then(|stage| policy.role_for(stage.as_str()))
            })
            .map(|role| role.to_string())
    }

    /// The ONE calculation behind the admission check, the stop question and
    /// both read surfaces (REQ-AL-07 AC1): the derived count of THIS role, the
    /// declared limit and the shared line. `None` when the task has no role
    /// stage.
    ///
    /// This entry point loads the history itself (admission has none handy).
    /// The status/progress reads already hold part of it and call
    /// [`Api::attempt_limit_view_in`], so the counter never re-reads a slice
    /// the caller just loaded (rework R6).
    pub(crate) fn attempt_limit_view(
        &self,
        task: &Task,
        policy: &ProcessPolicy,
    ) -> ApiResult<Option<AttemptLimitView>> {
        let executions = self.store.list_executions(Some(task.id))?;
        let artifacts = self.store.artifacts(task.id)?;
        let events = self.store.events(task.id, None)?;
        let convergence = self.attempt_convergence(task, &artifacts, &executions);
        self.attempt_limit_view_in(task, policy, &executions, &events, convergence.as_ref())
    }

    /// The same view over history the caller ALREADY loaded. `convergence`
    /// comes from [`Api::attempt_convergence`] (or the progress read's own
    /// slice) — the count is derived from stored rows, never from a field.
    pub(crate) fn attempt_limit_view_in(
        &self,
        task: &Task,
        policy: &ProcessPolicy,
        executions: &[RoleExecution],
        events: &[WorkflowEvent],
        convergence: Option<&ConvergenceView>,
    ) -> ApiResult<Option<AttemptLimitView>> {
        let Some(role) = self.attempt_role(task, policy) else {
            return Ok(None);
        };
        let attempts =
            self.attempts_without_progress(&role, executions, events, convergence, policy)?;
        Ok(Some(AttemptLimitView::new(
            &role,
            attempts,
            policy.brakes.attempt_limit(),
        )))
    }

    /// The convergence slice the attempt limit needs — and `None` when the
    /// task has published no carrier at all: with no report there is no
    /// series, so the counter skips reading and parsing artifact bodies that
    /// cannot move it (rework R6).
    pub(crate) fn attempt_convergence(
        &self,
        task: &Task,
        artifacts: &[devai_core::types::Artifact],
        executions: &[RoleExecution],
    ) -> Option<ConvergenceView> {
        let measured = artifacts.iter().any(|a| {
            matches!(
                a.kind,
                devai_core::types::ArtifactKind::ReviewReport
                    | devai_core::types::ArtifactKind::QualityReport
            )
        });
        measured
            .then(|| self.task_convergence(task, artifacts, executions))
            .flatten()
    }

    /// The derived count of ONE role, from stored rows only: the last
    /// substantive shift of THIS role, the last human reset of THIS role, and
    /// its completed stage launches between them.
    ///
    /// The history is read strictly: a transition row whose `from`/`to` cannot
    /// be understood refuses loudly here instead of being skipped, because a
    /// skipped row silently changes the number (rework R1 quality return).
    pub(crate) fn attempts_without_progress(
        &self,
        role: &str,
        executions: &[RoleExecution],
        events: &[WorkflowEvent],
        convergence: Option<&ConvergenceView>,
        policy: &ProcessPolicy,
    ) -> ApiResult<u32> {
        let progress = role_progress_at(role, policy, events, convergence)?;
        let reset = last_reset_at(events, role);
        Ok(attempts_without_progress(role, executions, progress, reset))
    }
}

/// The timestamp of the last SUBSTANTIVE shift of `role` in this task, if any —
/// the per-role counter's floor. Derived from stored events (owned stage moves,
/// review verdicts) and from the SAME convergence slice the read surfaces
/// publish, so a second read of an unchanged task yields the same value
/// (REQ-AL-03 AC3).
///
/// A shift of ANOTHER role is never this role's shift (REQ-AL-02 AC5): the
/// stage move is read against the role that executed the stage it left, the
/// verdict and the converging round against the series that judges this role's
/// work. A ping-pong that only re-enters states already reached is not a shift
/// and therefore keeps raising the count (rework R1/R2).
///
/// A damaged transition row is an ERROR, not an empty history: see
/// [`ordered_edges`].
pub fn role_progress_at(
    role: &str,
    policy: &ProcessPolicy,
    events: &[WorkflowEvent],
    convergence: Option<&ConvergenceView>,
) -> ApiResult<Option<DateTime<Utc>>> {
    Ok(max_opt(
        convergence_shift_at(role, policy, convergence),
        event_shift_at(role, policy, events)?,
    ))
}

/// The latest shift the measured series records for `role`: a CONVERGING round
/// or a verdict change between two of its rounds. Only the series the
/// DECLARATION says judges this role's work speaks here (rework R2).
fn convergence_shift_at(
    role: &str,
    policy: &ProcessPolicy,
    convergence: Option<&ConvergenceView>,
) -> Option<DateTime<Utc>> {
    let mut mark = None;
    for stage in &convergence?.stages {
        if series_judges(policy, stage.stage, role) {
            mark = max_opt(mark, series_shift_at(stage));
        }
    }
    mark
}

/// One series' own latest shift mark: a CONVERGING round, or a verdict that
/// changed between two rounds that both carry one.
fn series_shift_at(stage: &StageConvergence) -> Option<DateTime<Utc>> {
    let mut mark = None;
    let mut previous: Option<&str> = None;
    for point in &stage.series {
        let verdict_moved = previous.is_some_and(|prev| {
            point
                .verdict
                .as_deref()
                .is_some_and(|verdict| verdict != prev)
        });
        if point.mode == convergence::mode::CONVERGING || verdict_moved {
            mark = max_opt(mark, Some(point.created_at));
        }
        if point.verdict.is_some() {
            previous = point.verdict.as_deref();
        }
    }
    mark
}

/// The latest shift the stored EVENTS record for `role`: a verdict change the
/// series judges and a move of THIS role's stage into a state it had never
/// reached before. A repeat is a round, not a shift.
fn event_shift_at(
    role: &str,
    policy: &ProcessPolicy,
    events: &[WorkflowEvent],
) -> ApiResult<Option<DateTime<Utc>>> {
    let edges = ordered_edges(events)?;
    Ok(max_opt(
        verdict_shift_at(role, policy, &edges),
        stage_move_shift_at(role, policy, &edges),
    ))
}

/// The working edges of the stored events in append order: a real
/// `from → to` move, not a hold and not a same-state write. A hold is skipped
/// because a human question is not progress (see the module docs).
///
/// A row that CARRIES `from`/`to` but whose values cannot be read as states is
/// refused, never skipped (rework R1 quality return: the old `.ok()?` dropped
/// such a row without a trace, so the counter could print a number the stored
/// history does not support — the dropped row may be the very move that
/// shifted the series, or a repeat that should raise it). The refusal names the
/// event and the value, so a damaged row is repaired instead of swallowed.
fn ordered_edges(
    events: &[WorkflowEvent],
) -> ApiResult<Vec<(&WorkflowEvent, TaskState, TaskState)>> {
    let mut ordered: Vec<&WorkflowEvent> = events.iter().collect();
    ordered.sort_by(|a, b| a.created_at.cmp(&b.created_at).then(a.id.cmp(&b.id)));
    let mut edges = Vec::new();
    for ev in ordered {
        let Some(payload) = ev.payload.as_object() else {
            continue;
        };
        let (Some(from), Some(to)) = (payload.get(json_field::FROM), payload.get(json_field::TO))
        else {
            continue;
        };
        let from = transition_state(ev, json_field::FROM, from)?;
        let to = transition_state(ev, json_field::TO, to)?;
        if from != to && !from.is_hold() && !to.is_hold() {
            edges.push((ev, from, to));
        }
    }
    Ok(edges)
}

/// One side of a transition row, read strictly: a value that is not a state
/// name is a refusal with the event id and the offending value, because the
/// count is derived from these rows and a silently missing row would lie.
fn transition_state(
    ev: &WorkflowEvent,
    side: &'static str,
    value: &serde_json::Value,
) -> ApiResult<TaskState> {
    value
        .as_str()
        .and_then(|state| TaskState::parse(state).ok())
        .ok_or_else(|| {
            ApiError::refused(
                "corrupt_transition",
                format!(
                    "workflow event {} carries an unreadable `{side}` (`{value}`) — the attempt \
                     counter cannot be derived from a history it cannot read",
                    ev.id
                ),
            )
        })
}

/// A verdict change the judge recorded while the task stood in a stage the
/// declared series judges — the same substance the series records, read even
/// when no series exists yet.
fn verdict_shift_at(
    role: &str,
    policy: &ProcessPolicy,
    edges: &[(&WorkflowEvent, TaskState, TaskState)],
) -> Option<DateTime<Utc>> {
    let mut mark = None;
    let mut previous: Option<String> = None;
    for (ev, from, _) in edges {
        if ev.kind != event_kind::REVIEW_RECORDED {
            continue;
        }
        let Some(verdict) = ev.payload.get("verdict").and_then(|v| v.as_str()) else {
            continue;
        };
        let changed = previous.as_deref().is_some_and(|prev| prev != verdict);
        if changed && series_of(policy, *from).is_some_and(|s| series_judges(policy, s, role)) {
            mark = max_opt(mark, Some(ev.created_at));
        }
        previous = Some(verdict.to_string());
    }
    mark
}

/// A move out of THIS role's stage into a state the task had never reached
/// before: the only stage-move form that counts. A ping-pong re-enters states
/// it has already been in, so its rounds keep raising the count.
fn stage_move_shift_at(
    role: &str,
    policy: &ProcessPolicy,
    edges: &[(&WorkflowEvent, TaskState, TaskState)],
) -> Option<DateTime<Utc>> {
    let mut mark = None;
    let mut visited: Vec<TaskState> = Vec::new();
    for (ev, from, to) in edges {
        let first_visit = !visited.contains(to);
        if first_visit {
            visited.push(*to);
        }
        if first_visit && policy.role_for(from.as_str()) == Some(role) {
            mark = max_opt(mark, Some(ev.created_at));
        }
    }
    mark
}

/// The later of two optional marks; `None` loses to `Some`.
fn max_opt(a: Option<DateTime<Utc>>, b: Option<DateTime<Utc>>) -> Option<DateTime<Utc>> {
    match (a, b) {
        (Some(a), Some(b)) => Some(a.max(b)),
        (a, b) => a.or(b),
    }
}

/// The timestamp of the last HUMAN reset of THIS role's counter — the stored
/// answer «продолжить». It is the only reset boundary of the counter
/// (REQ-AL-06): editing the declaration, a re-run and a restart write no such
/// event, so none of them zeroes the number.
pub fn last_reset_at(events: &[WorkflowEvent], role: &str) -> Option<DateTime<Utc>> {
    events
        .iter()
        .filter(|ev| ev.kind == event_kind::ATTEMPT_LIMIT_RESET)
        .filter(|ev| ev.payload.get("role").and_then(|v| v.as_str()) == Some(role))
        .map(|ev| ev.created_at)
        .max()
}

/// Completed stage launches of `role` after the last shift / human reset.
///
/// A stage launch is a row WITHOUT a `parent_execution`: the response-form
/// correction loop writes one row per FORM attempt, and the form retries of
/// one launch are the same attempt (REQ-AL-02 AC1). Failed, cancelled and
/// running rows are not attempts: a launch that did not happen (provider or
/// platform refusal, no agent, admission refusal) spends no attempt and
/// resets none.
///
/// The floor is the END of the launch that produced the last shift (see
/// [`shift_floor`]), not the shift's own timestamp: on a live task the artifact
/// (and the verdict event) is published INSIDE the run while `finished_at` is
/// written after it, so a bare timestamp would count the shifting launch
/// itself — «1 из 5» straight after a real advance (rework R1).
pub fn attempts_without_progress(
    role: &str,
    executions: &[RoleExecution],
    progress_at: Option<DateTime<Utc>>,
    reset_at: Option<DateTime<Utc>>,
) -> u32 {
    let floor = max_opt(shift_floor(role, executions, progress_at), reset_at);
    executions
        .iter()
        .filter(|e| e.role_slug == role)
        .filter(|e| e.parent_execution.is_none())
        .filter(|e| e.status == ExecutionStatus::Completed)
        .filter(|e| {
            let finished = e.finished_at.unwrap_or(e.started_at);
            floor.is_none_or(|floor| finished > floor)
        })
        .count() as u32
}

/// The boundary of the LAST shift: the completion of the completed stage
/// launch whose interval [started_at, finished_at] contains the shift mark.
///
/// A shift mark is the artifact/event time, and the platform publishes it
/// BEFORE the engine closes the execution (engine.rs: the agent publishes, the
/// run finishes after). Counting `finished_at > mark` would therefore count
/// the very launch that advanced the task. When no launch of this role
/// contains the mark (e.g. the judge's verdict belongs to another role's run),
/// the mark itself is the boundary — no launch can straddle it.
fn shift_floor(
    role: &str,
    executions: &[RoleExecution],
    progress_at: Option<DateTime<Utc>>,
) -> Option<DateTime<Utc>> {
    let at = progress_at?;
    executions
        .iter()
        .filter(|e| e.role_slug == role)
        .filter(|e| e.parent_execution.is_none())
        .filter(|e| e.status == ExecutionStatus::Completed)
        .filter_map(|e| {
            let finished = e.finished_at.unwrap_or(e.started_at);
            (e.started_at <= at && at <= finished).then_some(finished)
        })
        .max()
        .or(Some(at))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use devai_core::process::Process;
    use devai_core::types::new_id;
    use devai_runtime::convergence::{ConvergenceCounters, ConvergencePoint, StageConvergence};

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(1_700_000_000 + secs, 0).unwrap()
    }

    fn event(kind: &'static str, at: i64, payload: serde_json::Value) -> WorkflowEvent {
        WorkflowEvent {
            id: new_id(),
            task_id: None,
            kind: kind.to_string(),
            actor: "tester".into(),
            payload,
            created_at: ts(at),
        }
    }

    /// `role_progress_at` on a HEALTHY stored history. The shifts asserted
    /// below are the subject of these cases; the strictness refusal has its own
    /// case ([`a_damaged_transition_row_is_refused_not_silently_dropped`]).
    fn progress_at(
        role: &str,
        policy: &ProcessPolicy,
        events: &[WorkflowEvent],
        convergence: Option<&ConvergenceView>,
    ) -> Option<DateTime<Utc>> {
        role_progress_at(role, policy, events, convergence)
            .expect("a test history is readable: no transition row is damaged")
    }

    fn exec(
        role: &str,
        started: i64,
        finished: Option<i64>,
        status: ExecutionStatus,
    ) -> RoleExecution {
        RoleExecution {
            id: new_id(),
            task_id: new_id(),
            role_slug: role.into(),
            role_version: 1,
            agent_name: Some("mock".into()),
            status,
            inputs: Vec::new(),
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
            bundle: None,
            run_id: None,
        }
    }

    /// One round of the burning loop: the analyst hands the spec over, the
    /// reviewer bounces it back with the SAME verdict. Nothing shifts.
    fn loop_events(rounds: usize) -> Vec<WorkflowEvent> {
        let mut events = Vec::new();
        for i in 0..rounds {
            let at = 10 + i as i64 * 4;
            events.push(event(
                event_kind::REQUIREMENTS_PUBLISHED,
                at,
                serde_json::json!({"from": "ANALYSIS", "to": "REVIEW"}),
            ));
            events.push(event(
                event_kind::REVIEW_RECORDED,
                at + 2,
                serde_json::json!({
                    "verdict": "NEEDS_REVISION",
                    "from": "REVIEW",
                    "to": "ANALYSIS",
                }),
            ));
        }
        events
    }

    /// The invariant fields of a point; the two that vary per case are set by
    /// [`point_in`]. Split out so no test helper is a wall of `None`s.
    fn base_point(stage: &'static str, round: u32, at: i64) -> ConvergencePoint {
        ConvergencePoint {
            round,
            artifact_id: "a".into(),
            created_at: ts(at),
            stage,
            status: "CURRENT".into(),
            measured: true,
            comparable: true,
            missing: Vec::new(),
            score_milli: None,
            theta_milli: None,
            low_count: None,
            unscored_count: None,
            invalid_count: None,
            pending_count: None,
            closed_count: None,
            delta_score_milli: None,
            verdict: None,
            requirements_count: Some(3),
            oq_count: Some(0),
            delta_requirements: Some(0),
            delta_oq: Some(0),
            volume_direction: Some("flat"),
            time_seconds: Some(10),
            profile: None,
            mode: convergence::mode::PLATEAU,
            mode_basis: convergence::ModeBasis {
                rule: "plateau",
                reason: None,
                rounds: vec![round],
                check_id: None,
            },
            read_error: None,
        }
    }

    fn point_in(
        stage: &'static str,
        round: u32,
        at: i64,
        mode: &'static str,
        verdict: Option<&str>,
    ) -> ConvergencePoint {
        ConvergencePoint {
            mode,
            verdict: verdict.map(|v| v.to_string()),
            ..base_point(stage, round, at)
        }
    }

    fn view_in(stage: &'static str, points: Vec<ConvergencePoint>) -> ConvergenceView {
        ConvergenceView {
            gap_milli: 10,
            stages: vec![StageConvergence {
                stage,
                series: points,
                counters: ConvergenceCounters::default(),
                stage_total_seconds: None,
                stage_current_seconds: None,
                missing: Vec::new(),
            }],
        }
    }

    fn view(points: Vec<ConvergencePoint>) -> ConvergenceView {
        view_in(convergence::STAGE_SPEC_REVIEW, points)
    }

    #[test]
    fn the_line_speaks_the_same_numbers_with_or_without_a_limit() {
        let v = AttemptLimitView::new("requirements-analyst", 2, Some(5));
        assert_eq!(v.line, "попыток без прогресса: 2 из 5");
        assert!(v.phrase().contains("2 из 5"));
        let v = AttemptLimitView::new("requirements-analyst", 7, None);
        assert_eq!(v.line, "попыток без прогресса: 7 — без предела");
        assert!(!limit_reached(7, None), "no limit never stops");
        assert!(!limit_reached(4, Some(5)));
        assert!(limit_reached(5, Some(5)));
    }

    #[test]
    fn a_new_spec_version_with_the_same_verdict_is_not_progress() {
        // Five points, flat volume, the same NEEDS_REVISION verdict: the
        // gutted case («50 версий, 48 отчётов») must count as zero progress.
        let points: Vec<ConvergencePoint> = (1..=5)
            .map(|i| {
                point_in(
                    convergence::STAGE_SPEC_REVIEW,
                    i,
                    i as i64 * 10,
                    convergence::mode::PLATEAU,
                    Some("NEEDS_REVISION"),
                )
            })
            .collect();
        let v = view(points);
        let policy = Process::factory();
        assert_eq!(
            progress_at("requirements-analyst", policy, &[], Some(&v)),
            None
        );
        assert_eq!(
            progress_at("requirements-reviewer", policy, &[], Some(&v)),
            None
        );
    }

    #[test]
    fn a_verdict_change_and_a_converging_round_are_progress() {
        let policy = Process::factory();
        let v = view(vec![
            point_in(
                convergence::STAGE_SPEC_REVIEW,
                1,
                10,
                convergence::mode::PLATEAU,
                Some("NEEDS_REVISION"),
            ),
            point_in(
                convergence::STAGE_SPEC_REVIEW,
                2,
                20,
                convergence::mode::PLATEAU,
                Some("NEEDS_REVISION"),
            ),
            point_in(
                convergence::STAGE_SPEC_REVIEW,
                3,
                30,
                convergence::mode::PLATEAU,
                Some("APPROVED"),
            ),
        ]);
        assert_eq!(
            progress_at("requirements-analyst", policy, &[], Some(&v)),
            Some(ts(30))
        );

        let v = view(vec![
            point_in(
                convergence::STAGE_SPEC_REVIEW,
                1,
                10,
                convergence::mode::PLATEAU,
                None,
            ),
            point_in(
                convergence::STAGE_SPEC_REVIEW,
                2,
                20,
                convergence::mode::CONVERGING,
                None,
            ),
        ]);
        assert_eq!(
            progress_at("requirements-reviewer", policy, &[], Some(&v)),
            Some(ts(20))
        );
    }

    #[test]
    fn a_hold_is_not_progress_but_a_first_move_to_a_working_stage_is() {
        let events = vec![
            event(
                event_kind::STATE_CHANGED,
                10,
                serde_json::json!({"from": "ANALYSIS", "to": "REVIEW"}),
            ),
            event(
                event_kind::QUESTION_RAISED,
                20,
                serde_json::json!({"from": "REVIEW", "to": "WAITING_FOR_INPUT"}),
            ),
            event(
                event_kind::QUESTION_ANSWERED,
                30,
                serde_json::json!({"from": "WAITING_FOR_INPUT", "to": "REVIEW"}),
            ),
        ];
        let policy = Process::factory();
        assert_eq!(
            progress_at("requirements-analyst", policy, &events, None),
            Some(ts(10)),
            "only the analyst's own move into a NEW state counts; holds never do"
        );
        assert_eq!(
            progress_at("requirements-reviewer", policy, &events, None),
            None,
            "the task was never moved out of REVIEW by the reviewer"
        );
    }

    #[test]
    fn ten_rounds_of_analysis_review_without_a_shift_raise_the_count_past_the_limit() {
        // The burning case (rework R1): ANALYSIS ↔ REVIEW ten times, the same
        // NEEDS_REVISION every round. The counter must CLEAR the limit, not be
        // zeroed by each transition.
        let policy = Process::factory();
        let events = loop_events(10);
        let progress = progress_at("requirements-analyst", policy, &events, None);
        assert_eq!(
            progress,
            Some(ts(10)),
            "the first handoff into REVIEW is the last shift; the other nine rounds are repeats"
        );
        let executions: Vec<RoleExecution> = (0..10)
            .map(|i| {
                exec(
                    "requirements-analyst",
                    9 + i as i64 * 4,
                    Some(9 + i as i64 * 4),
                    ExecutionStatus::Completed,
                )
            })
            .collect();
        let attempts =
            attempts_without_progress("requirements-analyst", &executions, progress, None);
        assert_eq!(attempts, 9, "every round after the first shift counts");
        assert!(
            limit_reached(attempts, Some(5)),
            "ten rounds without a shift stop the task, they do not ride as movement"
        );

        // The reviewer is counted apart: its own bounce move is its first
        // shift, then its rounds add up as well — neither role can hide behind
        // the other's counter.
        let reviewer = progress_at("requirements-reviewer", policy, &events, None);
        assert_eq!(reviewer, Some(ts(12)));
    }

    #[test]
    fn a_move_or_a_series_of_another_role_never_resets_this_roles_counter() {
        let policy = Process::factory();
        let mut events = loop_events(5);
        // The implementation agent moves the task into a stage it had never
        // reached: a genuine advance of ANOTHER role.
        events.push(event(
            event_kind::IMPLEMENTATION_REPORTED,
            100,
            serde_json::json!({"from": "REWORK", "to": "QA"}),
        ));
        // A converging QUALITY round is movement of the QA/implementation
        // series, not of the spec-review one.
        let quality = view_in(
            convergence::STAGE_QUALITY,
            vec![point_in(
                convergence::STAGE_QUALITY,
                2,
                120,
                convergence::mode::CONVERGING,
                None,
            )],
        );
        assert_eq!(
            progress_at("requirements-analyst", policy, &events, Some(&quality)),
            Some(ts(10)),
            "another role's move and another role's series leave the analyst's series alone"
        );
        assert_eq!(
            progress_at("implementation-agent", policy, &events, Some(&quality)),
            Some(ts(120))
        );
        let executions: Vec<RoleExecution> = (0..5)
            .map(|i| {
                exec(
                    "requirements-analyst",
                    9 + i as i64 * 4,
                    Some(9 + i as i64 * 4),
                    ExecutionStatus::Completed,
                )
            })
            .collect();
        assert_eq!(
            attempts_without_progress("requirements-analyst", &executions, Some(ts(10)), None),
            4,
            "the count is not zeroed by somebody else's progress"
        );
    }

    #[test]
    fn a_genuine_verdict_change_resets_the_count_of_both_sides_of_the_series() {
        let policy = Process::factory();
        let events = vec![
            event(
                event_kind::REQUIREMENTS_PUBLISHED,
                10,
                serde_json::json!({"from": "ANALYSIS", "to": "REVIEW"}),
            ),
            event(
                event_kind::REVIEW_RECORDED,
                12,
                serde_json::json!({
                    "verdict": "NEEDS_REVISION",
                    "from": "REVIEW",
                    "to": "ANALYSIS",
                }),
            ),
            event(
                event_kind::REQUIREMENTS_PUBLISHED,
                14,
                serde_json::json!({"from": "ANALYSIS", "to": "REVIEW"}),
            ),
            event(
                event_kind::REVIEW_RECORDED,
                16,
                serde_json::json!({
                    "verdict": "APPROVED",
                    "from": "REVIEW",
                    "to": "APPROVAL",
                }),
            ),
        ];
        let progress = progress_at("requirements-analyst", policy, &events, None);
        assert_eq!(
            progress,
            Some(ts(16)),
            "the verdict moved: the spec judged better"
        );
        let executions = vec![
            exec(
                "requirements-analyst",
                5,
                Some(9),
                ExecutionStatus::Completed,
            ),
            exec(
                "requirements-analyst",
                11,
                Some(13),
                ExecutionStatus::Completed,
            ),
        ];
        assert_eq!(
            attempts_without_progress("requirements-analyst", &executions, progress, None),
            0,
            "a real shift of the role's own work zeroes its series"
        );
    }

    #[test]
    fn the_qa_rework_loop_raises_the_count_of_both_roles() {
        // The second burning loop of rework R1: QA sends the result back, the
        // implementation agent reports again, QA sends it back — nothing
        // shifts. Both roles must accumulate, not reset each other.
        let mut events = vec![event(
            event_kind::IMPLEMENTATION_REPORTED,
            4,
            serde_json::json!({"from": "IMPLEMENTATION", "to": "QA"}),
        )];
        for i in 0..5 {
            let at = 10 + i as i64 * 10;
            events.push(event(
                event_kind::QUALITY_RETURNED,
                at,
                serde_json::json!({"from": "QA", "to": "REWORK"}),
            ));
            events.push(event(
                event_kind::IMPLEMENTATION_REPORTED,
                at + 2,
                serde_json::json!({"from": "REWORK", "to": "QA"}),
            ));
        }
        let policy = Process::factory();
        let qa = progress_at("quality-auditor", policy, &events, None);
        assert_eq!(qa, Some(ts(10)), "the first return is the last QA shift");
        let implementer = progress_at("implementation-agent", policy, &events, None);
        assert_eq!(
            implementer,
            Some(ts(4)),
            "REWORK → QA repeats the state QA already saw: no shift"
        );

        let qa_runs: Vec<RoleExecution> = (0..5)
            .map(|i| {
                exec(
                    "quality-auditor",
                    11 + i as i64 * 10,
                    Some(11 + i as i64 * 10),
                    ExecutionStatus::Completed,
                )
            })
            .collect();
        assert_eq!(
            attempts_without_progress("quality-auditor", &qa_runs, qa, None),
            5
        );
        assert!(limit_reached(5, Some(5)));
    }

    #[test]
    fn only_completed_stage_launches_count_and_form_retries_are_the_same_attempt() {
        let mut failed = exec("analyst", 1, Some(2), ExecutionStatus::Failed);
        failed.failure_class = Some("provider".into());
        let mut retry = exec("analyst", 3, Some(4), ExecutionStatus::Completed);
        retry.attempt = 2;
        retry.parent_execution = Some(new_id());
        let executions = vec![
            exec("analyst", 1, Some(2), ExecutionStatus::Completed),
            exec("analyst", 3, Some(4), ExecutionStatus::Completed),
            failed,
            retry,
            exec("reviewer", 5, Some(6), ExecutionStatus::Completed),
            exec("analyst", 7, None, ExecutionStatus::Running),
        ];
        assert_eq!(
            attempts_without_progress("analyst", &executions, None, None),
            2,
            "completed stage launches only: other roles, form retries, failures and the running row are out"
        );
        // A shift after the first two leaves the third (if any) alone; a reset
        // at time 3 counts only completions FINISHED after it.
        assert_eq!(
            attempts_without_progress("analyst", &executions, None, Some(ts(3))),
            1
        );
    }

    #[test]
    fn a_reset_is_scoped_to_its_role() {
        let events = vec![
            event(
                event_kind::ATTEMPT_LIMIT_RESET,
                100,
                serde_json::json!({"role": "requirements-reviewer"}),
            ),
            event(
                event_kind::ATTEMPT_LIMIT_RESET,
                30,
                serde_json::json!({"role": "requirements-analyst"}),
            ),
        ];
        assert_eq!(
            last_reset_at(&events, "requirements-analyst"),
            Some(ts(30)),
            "only this role's reset is a boundary"
        );
        assert_eq!(
            last_reset_at(&events, "requirements-reviewer"),
            Some(ts(100))
        );
    }

    /// The event stream of the live order: the verdicts are published INSIDE
    /// the run's lifetime, as the engine does (see the test below).
    fn live_order_events() -> Vec<WorkflowEvent> {
        vec![
            event(
                event_kind::REQUIREMENTS_PUBLISHED,
                11,
                serde_json::json!({"from": "ANALYSIS", "to": "REVIEW"}),
            ),
            event(
                event_kind::REVIEW_RECORDED,
                12,
                serde_json::json!({"verdict": "NEEDS_REVISION", "from": "REVIEW", "to": "ANALYSIS"}),
            ),
            event(
                event_kind::REQUIREMENTS_PUBLISHED,
                19,
                serde_json::json!({"from": "ANALYSIS", "to": "REVIEW"}),
            ),
            event(
                event_kind::REVIEW_RECORDED,
                20,
                serde_json::json!({"verdict": "APPROVED", "from": "REVIEW", "to": "APPROVAL"}),
            ),
        ]
    }

    #[test]
    fn a_shift_published_inside_a_run_zeroes_the_counter_on_the_live_order() {
        // Live order (rework R1/R5): the role starts a run, publishes the
        // artifact and records the verdict INSIDE it, and the engine closes the
        // execution afterwards. The shifting launch must not count itself.
        let policy = Process::factory();
        let progress = progress_at("requirements-analyst", policy, &live_order_events(), None);
        assert_eq!(progress, Some(ts(20)), "the approved verdict is the shift");

        // The run that produced it started at 10 and finished at 30 — the
        // shift at 20 lies INSIDE it, exactly as on a live task.
        let shifting = exec(
            "requirements-analyst",
            10,
            Some(30),
            ExecutionStatus::Completed,
        );
        assert_eq!(
            attempts_without_progress("requirements-analyst", &[shifting.clone()], progress, None),
            0,
            "the launch that DID the shift is not an attempt after it"
        );

        // Only launches completed after that one count, one per launch.
        let next = exec(
            "requirements-analyst",
            40,
            Some(50),
            ExecutionStatus::Completed,
        );
        assert_eq!(
            attempts_without_progress("requirements-analyst", &[shifting, next], progress, None),
            1,
            "a later immobile launch counts once"
        );
    }

    #[test]
    fn a_damaged_transition_row_is_refused_not_silently_dropped() {
        // Rework R1 quality return: `ordered_edges` parsed the state names with
        // `.ok()?`, so a row carrying an unreadable `to` vanished from the
        // history the count is derived from — and the counter could then print
        // a number the stored rows do not support (the dropped row may be the
        // very move that shifted the series, or a repeat that should raise it).
        let policy = Process::factory();
        let mut events = loop_events(2);
        let damaged = event(
            event_kind::STATE_CHANGED,
            40,
            serde_json::json!({"from": "ANALYSIS", "to": "REVIEWED"}),
        );
        let damaged_id = damaged.id.to_string();
        events.push(damaged);

        let err = role_progress_at("requirements-analyst", policy, &events, None).unwrap_err();
        assert_eq!(err.code().0, "corrupt_transition", "{err}");
        assert!(
            err.to_string().contains(&damaged_id),
            "the refusal names the event: {err}"
        );
        assert!(
            err.to_string().contains("REVIEWED"),
            "the refusal names the value: {err}"
        );
    }

    #[test]
    fn the_series_catalog_is_read_from_the_declaration() {
        // Rework R2: the stage→states table is data. The shipped declaration
        // maps both measured series; clearing it makes progress fall back to
        // moves and verdicts instead of a code-side catalog.
        let policy = Process::factory();
        assert_eq!(
            policy.series_states(convergence::STAGE_SPEC_REVIEW),
            ["ANALYSIS".to_string(), "REVIEW".to_string()]
        );
        assert_eq!(policy.series_of("QA"), Some(convergence::STAGE_QUALITY));
        assert!(policy.series_judges(convergence::STAGE_QUALITY, "quality-auditor"));

        let mut without = policy.clone();
        without.series.clear();
        let converging = view(vec![point_in(
            convergence::STAGE_SPEC_REVIEW,
            2,
            20,
            convergence::mode::CONVERGING,
            None,
        )]);
        assert_eq!(
            progress_at("requirements-analyst", &without, &[], Some(&converging)),
            None,
            "no declared series means no measured movement"
        );
    }
}
