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
//! the token, never a machine name printed where a phrase belongs.

use devai_core::types::{
    Id, Question, Task, Timestamp, WorkflowEvent, event_kind, json_field, stop_kind,
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
    /// ONE rendering of the pair: `3 из 3`, or
    /// «сработало с первого раза, порог 1», or
    /// «остановлено вручную, счётчика нет» — never an invented `0 из 0`.
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
    /// The consecutive counter the event recorded.
    pub consecutive: u32,
    /// The declared threshold at that moment.
    pub limit: u32,
    /// Did THIS failed launch park the task (as the event recorded it)?
    pub parked: bool,
    /// Is the task going RIGHT NOW (it is in no hold state)? `false` on a
    /// task held by any stop — so «continues» never claims a blocked task is
    /// running, while `parked` keeps saying what THIS event did.
    pub continues: bool,
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

/// The ONE rendering of a counter pair (ratified OQ-PK-2 and OQ-PK-7): a
/// manual stop says so instead of an invented `0 из 0`, and a park on the very
/// first occurrence says «с первого раза», not a bare `1 из 1`.
fn counter_text(counter: Option<u32>, limit: Option<u32>) -> String {
    match (counter, limit) {
        (Some(1), Some(1)) => "сработало с первого раза, порог 1".to_string(),
        (Some(n), Some(m)) => format!("{n} из {m}"),
        (Some(n), None) => n.to_string(),
        _ => "остановлено вручную, счётчика нет".to_string(),
    }
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
    match payload_str(ev, "to") {
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
        event_kind::PROCESS_PARKED => Some(payload_str(ev, "brake")?.to_string()),
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
fn question_text<'a>(questions: &'a [Question], ev: &WorkflowEvent) -> Option<&'a str> {
    let id: Id = payload_str(ev, json_field::QUESTION_ID)?.parse().ok()?;
    questions
        .iter()
        .find(|q| q.id == id)
        .map(|q| q.text.as_str())
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
        let launch = launch_failure_view(&events, reasons, task.state.is_hold())?;
        Ok((stop, launch))
    }

    /// The LATEST stop event that explains the current hold — the reason a
    /// park is visible, not a `state` field copied around.
    fn stop_view(
        &self,
        task: &Task,
        events: &[WorkflowEvent],
        questions: &[Question],
        reasons: &ParkReasons,
    ) -> ApiResult<Option<StopView>> {
        let Some((ev, kind)) = events
            .iter()
            .rev()
            .find_map(|ev| stop_kind_of(ev, task).map(|kind| (ev, kind)))
        else {
            return Ok(None);
        };
        let declared = words(reasons.stop_words(&kind), "phrase for stop kind", &kind)?;
        let (counter, limit) = match kind.as_str() {
            stop_kind::AGENT_FAILURE => (
                payload_u32(ev, json_field::SERIES_COUNT),
                payload_u32(ev, json_field::THRESHOLD),
            ),
            stop_kind::QUESTION
            | stop_kind::MANUAL_BLOCK
            | stop_kind::MANUAL_PAUSE
            | stop_kind::MANUAL_CANCEL
            | stop_kind::MANUAL_FAIL => (None, None),
            // Every brake records its counter and the limit it tripped.
            _ => (payload_u32(ev, "counter"), payload_u32(ev, "limit")),
        };
        // A question hold's reason is the stored question text — verbatim.
        let reason = match kind.as_str() {
            stop_kind::AGENT_FAILURE | stop_kind::QUESTION => {
                question_text(questions, ev).map(|t| t.to_string())
            }
            _ => payload_str(ev, json_field::REASON).map(|r| r.to_string()),
        };
        let failure_class = (kind.as_str() == stop_kind::AGENT_FAILURE)
            .then(|| payload_str(ev, json_field::CLASS).map(|c| c.to_string()))
            .flatten();
        let failure_class_word = match failure_class.as_deref() {
            Some(class) => Some(
                words(
                    reasons.class_words(class),
                    "phrase for failure class",
                    class,
                )?
                .why
                .clone(),
            ),
            None => None,
        };
        Ok(Some(StopView {
            kind: kind.to_string(),
            why: declared.why.clone(),
            counter,
            limit,
            counter_text: counter_text(counter, limit),
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
fn launch_failure_view(
    events: &[WorkflowEvent],
    reasons: &ParkReasons,
    held: bool,
) -> ApiResult<Option<LaunchFailureView>> {
    let mut latest: Option<&WorkflowEvent> = None;
    for ev in events.iter().rev() {
        match ev.kind.as_str() {
            event_kind::EXECUTION_LAUNCH_FAILED => {
                latest = Some(ev);
                break;
            }
            // A launch that DID happen (or an execution that started) closes
            // the story of the failures before it.
            event_kind::EXECUTION_LAUNCHED | event_kind::EXECUTION_STARTED => return Ok(None),
            _ => {}
        }
    }
    let Some(ev) = latest else {
        return Ok(None);
    };
    let class = payload_str(ev, "failure_class").unwrap_or("unclassified");
    let declared = words(
        reasons.class_words(class),
        "phrase for failure class",
        class,
    )?;
    let parked = ev
        .payload
        .get("parked")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok(Some(LaunchFailureView {
        failure_class: class.to_string(),
        class_word: declared.why.clone(),
        at: ev.created_at,
        reason: payload_str(ev, json_field::REASON).map(|r| r.to_string()),
        action: declared.action.clone(),
        consecutive: payload_u32(ev, "consecutive").unwrap_or(1),
        limit: payload_u32(ev, "limit").unwrap_or(0),
        parked,
        continues: !held,
    }))
}
