//! WHY a task stands still, in the operator's words (task 01a1ca39,
//! REQ-PK-01/02/05/06).
//!
//! Nothing here stores anything: both facts are folded out of the task's
//! append-only history at every read (REQ-PK-04 — one line of truth, no
//! counter that can drift from the events that justify it):
//!
//! * [`StopView`] — the stop that put the task where it is now: the brake or
//!   the class, the counter and the limit from the very event, when and who
//!   recorded it, the stored reason VERBATIM and what a human can do next;
//! * [`LaunchFailureView`] — a role launch that did not happen: its class,
//!   time, verbatim reason and the proposed action, whether or not it parked
//!   the task.
//!
//! The words come from the declaration (`config/workflow/park-reasons.yaml`,
//! REQ-PK-03): a token with no declared phrase is a LOUD config error naming
//! the token, never a machine name printed where a phrase belongs. A stored
//! payload that cannot be READ is a loud corruption error in the same spirit —
//! absence is stated as absence, never papered over as one.

use devai_core::types::{
    Id, Question, Task, TaskState, Timestamp, WorkflowEvent, event_kind, json_field, stop_kind,
};
use devai_core::workflow::AGENT_FAILURE_GATE;
use devai_store::StoreError;
use devai_store::config::{ParkReasons, ParkWords};
use serde::Serialize;

use crate::{Api, ApiError, ApiResult};

/// Why the task is standing still — one read of the history, human words
/// resolved from the declaration.
#[derive(Serialize, Clone, Debug)]
pub struct StopView {
    /// The closed stop-kind token (`rounds_without_shift`, `agent_failure`, …).
    pub kind: String,
    /// Why it stands, in declared words.
    pub why: String,
    /// The counter the stop event recorded; `None` for a manual stop.
    pub counter: Option<u32>,
    /// The declared threshold at that moment; `None` when none applies.
    pub limit: Option<u32>,
    /// ONE rendering of the pair — `3 из 3`, «сработало с первого раза, порог
    /// 1», or, when the event recorded none, the absence in words for THIS kind
    /// (manual stop / waiting question / a brake with no counter). Never an
    /// invented `0 из 0`.
    pub counter_text: String,
    /// When the stop was recorded.
    pub at: Timestamp,
    /// The recording actor's token as stored.
    pub actor: String,
    /// The actor in words (`platform` → «платформа», a human's own name
    /// verbatim).
    pub actor_word: String,
    /// The stored reason, VERBATIM. `None` when the event carried none (a
    /// manual hold without a written reason) — absence is never filled with
    /// an invented sentence.
    pub reason: Option<String>,
    /// What the human can do next, in declared words.
    pub action: String,
    /// The failure class behind the stop, when the history names one (an
    /// agent-failure park).
    pub failure_class: Option<String>,
    /// Its human phrase, from the same declaration.
    pub failure_class_word: Option<String>,
}

/// A role launch that did NOT happen (REQ-PK-02): the class, the time, the
/// stored reason verbatim and the proposed action — visible whether it parked
/// the task or the task kept going.
#[derive(Serialize, Clone, Debug)]
pub struct LaunchFailureView {
    pub failure_class: String,
    pub class_word: String,
    pub at: Timestamp,
    pub reason: Option<String>,
    pub action: String,
    /// The consecutive counter the event recorded; `None` when the event
    /// recorded none — absence is stated, never invented as `1`.
    pub consecutive: Option<u32>,
    /// The declared threshold at that moment; `None` when none was recorded.
    pub limit: Option<u32>,
    /// ONE rendering of the pair, by the same rule the stop block uses.
    pub counter_text: String,
    /// Did THIS failed launch park the task (as the event recorded it)?
    /// `None` when the event does not say — the reader never assumes «no».
    pub parked: Option<bool>,
    /// Is the task going RIGHT NOW (it is in no hold state and not finished)?
    /// `false` on a task held by any stop — so «continues» never claims a
    /// blocked or a cancelled task is running, while `parked` keeps saying
    /// what THIS event did.
    pub continues: bool,
    /// Is the task FINISHED (accepted / cancelled)? `continues` is false for a
    /// hold and for a finished task alike; this says which one it is.
    pub finished: bool,
}

/// One declared phrase pair, or a loud error naming the missing value
/// (REQ-PK-03 AC3): a machine token is never printed in a phrase's place.
fn words<'a>(lookup: Option<&'a ParkWords>, what: &str, token: &str) -> ApiResult<&'a ParkWords> {
    lookup.ok_or_else(|| {
        ApiError::from(StoreError::Config(format!(
            "config/workflow/park-reasons.yaml declares no {what} for `{token}` — add it to the \
             `park_reasons:` section; a machine name is never printed in its place"
        )))
    })
}

/// A stored payload the reader cannot parse. Loud by the same contract as an
/// undeclared word: a corrupt event and an absent fact are different answers,
/// and only one of them may print «ничего не записано».
fn corrupt(ev: &WorkflowEvent, what: &str) -> ApiError {
    ApiError::from(StoreError::Corrupt(format!(
        "`{}` event {} {what} — the stored facts of this stop cannot be read (a corrupt payload \
         is not an absent one)",
        ev.kind, ev.id
    )))
}

/// The pair in words (ratified OQ-PK-7): `3 из 3`, or «сработало с первого
/// раза, порог 1» — `1 из 1` must not read like an accident. `None` when the
/// event recorded no pair: each caller words the absence for its own surface.
fn pair_text(counter: Option<u32>, limit: Option<u32>) -> Option<String> {
    match (counter, limit) {
        (Some(1), Some(1)) => Some("сработало с первого раза, порог 1".to_string()),
        (Some(n), Some(m)) => Some(format!("{n} из {m}")),
        (Some(n), None) => Some(n.to_string()),
        _ => None,
    }
}

/// The stop block's pair text: the pair itself, else WHY there is none — a
/// manual stop says it was manual (ratified OQ-PK-2), a question hold says it
/// waits for a human, and a brake whose event recorded no counter says exactly
/// that. Never an invented `0 из 0`.
fn stop_counter_text(kind: &str, counter: Option<u32>, limit: Option<u32>) -> String {
    if let Some(pair) = pair_text(counter, limit) {
        return pair;
    }
    match kind {
        stop_kind::MANUAL_BLOCK
        | stop_kind::MANUAL_PAUSE
        | stop_kind::MANUAL_CANCEL
        | stop_kind::MANUAL_FAIL => "остановлено вручную, счётчика нет",
        stop_kind::QUESTION => "счётчика нет: задача ждёт ответа человека",
        _ => "счётчик в событии не записан",
    }
    .to_string()
}

/// The failed-launch block's pair — an event that recorded no counter says so.
fn launch_counter_text(consecutive: Option<u32>, limit: Option<u32>) -> String {
    pair_text(consecutive, limit).unwrap_or_else(|| "счётчик не записан".to_string())
}

fn payload_u32(ev: &WorkflowEvent, key: &str) -> Option<u32> {
    ev.payload
        .get(key)
        .and_then(|v| v.as_u64())
        .map(|n| n as u32)
}

fn payload_str<'a>(ev: &'a WorkflowEvent, key: &str) -> Option<&'a str> {
    ev.payload.get(key).and_then(|v| v.as_str())
}

/// Does this event hold the task in its CURRENT state? Holds carry `to`;
/// a platform park (`process.parked`) carries the brake instead and is
/// accepted as-is — it is written exactly when the park happened.
fn arrived_here(ev: &WorkflowEvent, task: &Task) -> bool {
    match payload_str(ev, json_field::TO) {
        Some(to) => to == task.state.as_str(),
        None => true,
    }
}

/// The stop kind THIS event records for the task, if any. A platform-written
/// `task.blocked` is the state change of a brake, never a manual stop — the
/// brake's own `process.parked` event is the stop fact.
fn stop_kind_of(ev: &WorkflowEvent, task: &Task) -> Option<String> {
    match ev.kind.as_str() {
        // The brake token is carried AS RECORDED: an unknown token is then a
        // LOUD declaration error naming it (REQ-PK-03 AC3), never a silently
        // hidden stop.
        event_kind::PROCESS_PARKED => Some(payload_str(ev, json_field::BRAKE)?.to_string()),
        event_kind::QUESTION_RAISED if arrived_here(ev, task) => Some(
            if payload_str(ev, json_field::GATE) == Some(AGENT_FAILURE_GATE) {
                stop_kind::AGENT_FAILURE
            } else {
                stop_kind::QUESTION
            }
            .to_string(),
        ),
        event_kind::TASK_BLOCKED => manual(ev, task, stop_kind::MANUAL_BLOCK),
        event_kind::TASK_PAUSED => manual(ev, task, stop_kind::MANUAL_PAUSE),
        event_kind::TASK_CANCELLED => manual(ev, task, stop_kind::MANUAL_CANCEL),
        event_kind::TASK_FAILED => manual(ev, task, stop_kind::MANUAL_FAIL),
        _ => None,
    }
}

/// A human hold: the platform's own park events are not a human act.
fn manual(ev: &WorkflowEvent, task: &Task, kind: &'static str) -> Option<String> {
    (ev.actor != "platform" && arrived_here(ev, task)).then(|| kind.to_string())
}

/// The stored question an event refers to (its text is the reason of a
/// question hold). A question row is never deleted, so the lookup is exact.
///
/// An event with NO `question_id` has no question to cite — that is absence.
/// A `question_id` that does not PARSE is a corrupt payload: it is reported by
/// name, never folded into «причина не записана».
fn question_text<'a>(questions: &'a [Question], ev: &WorkflowEvent) -> ApiResult<Option<&'a str>> {
    let Some(raw) = payload_str(ev, json_field::QUESTION_ID) else {
        return Ok(None);
    };
    let id: Id = raw.parse().map_err(|_| {
        corrupt(
            ev,
            &format!(
                "carries a malformed `question_id` `{raw}` (the stored question cannot be found)"
            ),
        )
    })?;
    Ok(questions
        .iter()
        .find(|q| q.id == id)
        .map(|q| q.text.as_str()))
}

/// The LATEST stop event that explains the current hold — the reason a park is
/// visible, not a `state` field copied around.
fn latest_stop<'a>(
    events: &'a [WorkflowEvent],
    task: &Task,
) -> Option<(&'a WorkflowEvent, String)> {
    events
        .iter()
        .rev()
        .find_map(|ev| stop_kind_of(ev, task).map(|kind| (ev, kind)))
}

/// The counter pair a stop event recorded: the agent-failure series for a
/// parking question, the brake's own counter and limit, or none at all (a
/// question hold and every manual stop carry no counter).
fn stop_counter(ev: &WorkflowEvent, kind: &str) -> (Option<u32>, Option<u32>) {
    match kind {
        stop_kind::AGENT_FAILURE => (
            payload_u32(ev, json_field::SERIES_COUNT),
            payload_u32(ev, json_field::THRESHOLD),
        ),
        stop_kind::QUESTION
        | stop_kind::MANUAL_BLOCK
        | stop_kind::MANUAL_PAUSE
        | stop_kind::MANUAL_CANCEL
        | stop_kind::MANUAL_FAIL => (None, None),
        _ => (
            payload_u32(ev, json_field::COUNTER),
            payload_u32(ev, json_field::LIMIT),
        ),
    }
}

/// The stored reason, VERBATIM: the text of the stored question for a
/// question hold, the event's own `reason` for a brake or a manual stop.
fn stop_reason(
    ev: &WorkflowEvent,
    kind: &str,
    questions: &[Question],
) -> ApiResult<Option<String>> {
    match kind {
        stop_kind::AGENT_FAILURE | stop_kind::QUESTION => {
            Ok(question_text(questions, ev)?.map(|t| t.to_string()))
        }
        _ => Ok(payload_str(ev, json_field::REASON).map(|r| r.to_string())),
    }
}

/// The failure class behind an agent-failure park, with the declaration's
/// phrase for it — one pair, so the token and its words never come apart.
fn stop_class(
    ev: &WorkflowEvent,
    kind: &str,
    reasons: &ParkReasons,
) -> ApiResult<Option<(String, String)>> {
    if kind != stop_kind::AGENT_FAILURE {
        return Ok(None);
    }
    let Some(class) = payload_str(ev, json_field::CLASS) else {
        return Ok(None);
    };
    let word = words(
        reasons.class_words(class),
        "phrase for failure class",
        class,
    )?
    .why
    .clone();
    Ok(Some((class.to_string(), word)))
}

/// Is the task in motion RIGHT NOW? A hold and a finished task are both «not
/// going» — a cancelled task is never described as running.
fn is_going(state: TaskState) -> bool {
    !state.is_hold() && !state.is_terminal()
}

impl Api {
    /// The stop and launch-failure facts of one task, both folded from the
    /// append-only history by the SAME read (REQ-PK-04 AC1/AC2): two reads
    /// without new events answer with identical numbers and identical words.
    /// Nothing is written, nothing is cached.
    pub(crate) fn task_stop_facts(
        &self,
        task: &Task,
        questions: &[Question],
        reasons: &ParkReasons,
    ) -> ApiResult<(Option<StopView>, Option<LaunchFailureView>)> {
        let events = self.store.events(task.id, None)?;
        let stop = if task.state.is_hold() {
            self.stop_view(task, &events, questions, reasons)?
        } else {
            None
        };
        let launch = launch_failure_view(&events, reasons, task)?;
        Ok((stop, launch))
    }

    /// The stop that explains the current hold, worded from the declaration.
    fn stop_view(
        &self,
        task: &Task,
        events: &[WorkflowEvent],
        questions: &[Question],
        reasons: &ParkReasons,
    ) -> ApiResult<Option<StopView>> {
        let Some((ev, kind)) = latest_stop(events, task) else {
            return Ok(None);
        };
        let declared = words(reasons.stop_words(&kind), "phrase for stop kind", &kind)?;
        let (counter, limit) = stop_counter(ev, &kind);
        let (failure_class, failure_class_word) = match stop_class(ev, &kind, reasons)? {
            Some((class, word)) => (Some(class), Some(word)),
            None => (None, None),
        };
        let reason = stop_reason(ev, &kind, questions)?;
        let counter_text = stop_counter_text(&kind, counter, limit);
        Ok(Some(StopView {
            kind,
            why: declared.why.clone(),
            counter,
            limit,
            counter_text,
            at: ev.created_at,
            actor: ev.actor.clone(),
            actor_word: reasons.actor_word(&ev.actor),
            reason,
            action: declared.action.clone(),
            failure_class,
            failure_class_word,
        }))
    }
}

/// The latest launch that did not happen, unless a later SUCCESSFUL launch or
/// execution start already superseded it: an old failed launch is not the
/// state of a task that has been running since (REQ-PK-02 AC1/AC3).
fn latest_failed_launch(events: &[WorkflowEvent]) -> Option<&WorkflowEvent> {
    for ev in events.iter().rev() {
        match ev.kind.as_str() {
            event_kind::EXECUTION_LAUNCH_FAILED => return Some(ev),
            event_kind::EXECUTION_LAUNCHED | event_kind::EXECUTION_STARTED => return None,
            _ => {}
        }
    }
    None
}

/// The failed-launch fact of the current history, worded from the declaration.
fn launch_failure_view(
    events: &[WorkflowEvent],
    reasons: &ParkReasons,
    task: &Task,
) -> ApiResult<Option<LaunchFailureView>> {
    let Some(ev) = latest_failed_launch(events) else {
        return Ok(None);
    };
    let Some(class) = payload_str(ev, json_field::FAILURE_CLASS) else {
        return Err(corrupt(ev, "carries no `failure_class`"));
    };
    let declared = words(
        reasons.class_words(class),
        "phrase for failure class",
        class,
    )?;
    let consecutive = payload_u32(ev, json_field::CONSECUTIVE);
    let limit = payload_u32(ev, json_field::LIMIT);
    Ok(Some(LaunchFailureView {
        failure_class: class.to_string(),
        class_word: declared.why.clone(),
        at: ev.created_at,
        reason: payload_str(ev, json_field::REASON).map(|r| r.to_string()),
        action: declared.action.clone(),
        consecutive,
        limit,
        counter_text: launch_counter_text(consecutive, limit),
        parked: ev.payload.get(json_field::PARKED).and_then(|v| v.as_bool()),
        continues: is_going(task.state),
        finished: task.state.is_terminal(),
    }))
}
