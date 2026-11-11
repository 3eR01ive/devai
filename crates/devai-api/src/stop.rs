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
use devai_store::config::{
    ClassWords, CounterSource, LaunchAftermathWords, ParkReasons, ParkWords,
};
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
    /// 1», or, when the event recorded none, the declared sentence for THIS
    /// kind (manual stop / waiting question / a brake with no counter). Never
    /// an invented `0 из 0`.
    pub counter_text: String,
    /// When the stop was recorded.
    pub at: Timestamp,
    /// The recording actor's token as stored.
    pub actor: String,
    /// The actor in words (the platform's declared spelling, a human's own
    /// name verbatim).
    pub actor_word: String,
    /// The stored reason, VERBATIM. `None` when the event carried none (a
    /// manual hold without a written reason) — absence is never filled with
    /// an invented sentence.
    pub reason: Option<String>,
    /// The declaration's sentence for a stop that recorded no reason. Printed
    /// ONLY when `reason` is `None`; the fact itself stays `None` above.
    pub reason_absent: String,
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
    /// What this failure left the task in, in the declaration's words — the
    /// one sentence the human output prints. The three facts above stay the
    /// machine answer; this is their declared wording, not a code literal.
    pub aftermath: String,
}

/// How one history event names the stop it records.
#[derive(Clone, Copy)]
enum KindSource {
    /// `process.parked` carries the brake token in its payload (one of
    /// `stop_kind::BRAKES`).
    Brake,
    /// A parking question: the agent-failure gate names its own kind, any
    /// other gate is a plain question.
    Gate,
    /// A human hold: the event kind itself names the kind.
    Manual(&'static str),
}

/// One row of the classification table: the event kind that can hold a task
/// and how its stop kind is named.
struct KindRow {
    event: &'static str,
    source: KindSource,
}

/// The ONE classification of history events into stop kinds (mt/change-
/// isolation): the reader no longer spreads the vocabulary over a chain of
/// matches, and a kind that no row can produce is caught by the
/// `every_stop_kind_is_produced` test against `stop_kind::ALL`.
const KIND_ROWS: &[KindRow] = &[
    KindRow {
        event: event_kind::PROCESS_PARKED,
        source: KindSource::Brake,
    },
    KindRow {
        event: event_kind::QUESTION_RAISED,
        source: KindSource::Gate,
    },
    KindRow {
        event: event_kind::TASK_BLOCKED,
        source: KindSource::Manual(stop_kind::MANUAL_BLOCK),
    },
    KindRow {
        event: event_kind::TASK_PAUSED,
        source: KindSource::Manual(stop_kind::MANUAL_PAUSE),
    },
    KindRow {
        event: event_kind::TASK_CANCELLED,
        source: KindSource::Manual(stop_kind::MANUAL_CANCEL),
    },
    KindRow {
        event: event_kind::TASK_FAILED,
        source: KindSource::Manual(stop_kind::MANUAL_FAIL),
    },
];

/// One declared phrase pair, or a loud error naming the missing value
/// (REQ-PK-03 AC3): a machine token is never printed in a phrase's place.
fn words<'a, T>(lookup: Option<&'a T>, what: &str, token: &str) -> ApiResult<&'a T> {
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

/// A payload STRING field. A missing key is absence (`Ok(None)`); a key that
/// is present with another type is a corrupt payload, never absence — folding
/// `counter: "3"` into «счётчик не записан» is how a real fact disappears.
fn opt_str<'a>(ev: &'a WorkflowEvent, key: &str) -> ApiResult<Option<&'a str>> {
    match ev.payload.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_str()
            .map(Some)
            .ok_or_else(|| corrupt(ev, &format!("carries a `{key}` that is not a string: {v}"))),
    }
}

/// A payload field the event MUST carry: a missing one is corrupt too, because
/// the stop cannot be worded without it.
fn req_str<'a>(ev: &'a WorkflowEvent, key: &str) -> ApiResult<&'a str> {
    opt_str(ev, key)?.ok_or_else(|| corrupt(ev, &format!("carries no `{key}`")))
}

/// A payload COUNT field. A negative or fractional value, or one beyond `u32`,
/// is corrupt by the same rule.
fn opt_u32(ev: &WorkflowEvent, key: &str) -> ApiResult<Option<u32>> {
    match ev.payload.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|n| u32::try_from(n).ok())
            .map(Some)
            .ok_or_else(|| {
                corrupt(
                    ev,
                    &format!("carries a `{key}` that is not a non-negative integer: {v}"),
                )
            }),
    }
}

/// A payload BOOLEAN field; a non-boolean value is corrupt, not «not said».
fn opt_bool(ev: &WorkflowEvent, key: &str) -> ApiResult<Option<bool>> {
    match ev.payload.get(key) {
        None => Ok(None),
        Some(v) => v
            .as_bool()
            .map(Some)
            .ok_or_else(|| corrupt(ev, &format!("carries a `{key}` that is not a boolean: {v}"))),
    }
}

/// The pair in words (ratified OQ-PK-7): `3 из 3`, or «сработало с первого
/// раза, порог 1» — `1 из 1` must not read like an accident. `None` when the
/// event recorded no pair: each caller words the absence from its declaration.
fn pair_text(counter: Option<u32>, limit: Option<u32>) -> Option<String> {
    match (counter, limit) {
        (Some(1), Some(1)) => Some("сработало с первого раза, порог 1".to_string()),
        (Some(n), Some(m)) => Some(format!("{n} из {m}")),
        (Some(n), None) => Some(n.to_string()),
        _ => None,
    }
}

/// The stop block's pair text: the pair itself, else the declared sentence for
/// WHY there is none — a kind without a counter words its own absence, a brake
/// whose event recorded none uses the declaration's generic one. Never an
/// invented `0 из 0`.
fn stop_counter_text(words: &ParkWords, absence: &str, pair: (Option<u32>, Option<u32>)) -> String {
    pair_text(pair.0, pair.1).unwrap_or_else(|| {
        if words.counter_absent.is_empty() {
            absence.to_string()
        } else {
            words.counter_absent.clone()
        }
    })
}

/// Does this event hold the task in its CURRENT state? Holds carry `to`;
/// a platform park (`process.parked`) carries the brake instead and is
/// accepted as-is — it is written exactly when the park happened.
fn arrived_here(ev: &WorkflowEvent, task: &Task) -> ApiResult<bool> {
    match opt_str(ev, json_field::TO)? {
        Some(to) => Ok(to == task.state.as_str()),
        None => Ok(true),
    }
}

/// The stop kind THIS event records for the task, if any. A platform-written
/// `task.blocked` is the state change of a brake, never a manual stop — the
/// brake's own `process.parked` event is the stop fact.
fn stop_kind_of(
    ev: &WorkflowEvent,
    task: &Task,
    reasons: &ParkReasons,
) -> ApiResult<Option<String>> {
    let Some(row) = KIND_ROWS.iter().find(|r| r.event == ev.kind) else {
        return Ok(None);
    };
    match row.source {
        // The brake token is carried AS RECORDED: an unknown token is then a
        // LOUD declaration error naming it (REQ-PK-03 AC3), never a silently
        // hidden stop — and a `process.parked` that cannot name its brake is
        // corrupt, never «not a stop».
        KindSource::Brake => Ok(Some(req_str(ev, json_field::BRAKE)?.to_string())),
        KindSource::Gate => {
            if !arrived_here(ev, task)? {
                return Ok(None);
            }
            let kind = if opt_str(ev, json_field::GATE)? == Some(AGENT_FAILURE_GATE) {
                stop_kind::AGENT_FAILURE
            } else {
                stop_kind::QUESTION
            };
            Ok(Some(kind.to_string()))
        }
        KindSource::Manual(kind) => {
            // The platform's own park events are not a human act; which actor
            // token IS the platform comes from the declaration.
            if reasons.is_automatic(&ev.actor) || !arrived_here(ev, task)? {
                return Ok(None);
            }
            Ok(Some(kind.to_string()))
        }
    }
}

/// The stored question an event refers to (its text is the reason of a
/// question hold). A question row is never deleted, so the lookup is exact.
///
/// An event with NO `question_id` has no question to cite — that is absence.
/// A `question_id` that does not PARSE is a corrupt payload: it is reported by
/// name, never folded into «причина не записана».
fn question_text<'a>(questions: &'a [Question], ev: &WorkflowEvent) -> ApiResult<Option<&'a str>> {
    let Some(raw) = opt_str(ev, json_field::QUESTION_ID)? else {
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
    reasons: &ParkReasons,
) -> ApiResult<Option<(&'a WorkflowEvent, String)>> {
    for ev in events.iter().rev() {
        if let Some(kind) = stop_kind_of(ev, task, reasons)? {
            return Ok(Some((ev, kind)));
        }
    }
    Ok(None)
}

/// The counter pair a stop event recorded, taken from the pair the DECLARATION
/// assigns to the kind — never from a match over kind names.
fn stop_counter(
    ev: &WorkflowEvent,
    source: CounterSource,
) -> ApiResult<(Option<u32>, Option<u32>)> {
    match source {
        CounterSource::Brake => Ok((
            opt_u32(ev, json_field::COUNTER)?,
            opt_u32(ev, json_field::LIMIT)?,
        )),
        CounterSource::Series => Ok((
            opt_u32(ev, json_field::SERIES_COUNT)?,
            opt_u32(ev, json_field::THRESHOLD)?,
        )),
        CounterSource::None => Ok((None, None)),
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
        _ => Ok(opt_str(ev, json_field::REASON)?.map(|r| r.to_string())),
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
    let Some(class) = opt_str(ev, json_field::CLASS)? else {
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
        let Some((ev, kind)) = latest_stop(events, task, reasons)? else {
            return Ok(None);
        };
        let declared = words(reasons.stop_words(&kind), "phrase for stop kind", &kind)?;
        let pair = stop_counter(ev, declared.counter)?;
        let (failure_class, failure_class_word) = match stop_class(ev, &kind, reasons)? {
            Some((class, word)) => (Some(class), Some(word)),
            None => (None, None),
        };
        let reason = stop_reason(ev, &kind, questions)?;
        let counter_text = stop_counter_text(declared, &reasons.absence.counter, pair);
        Ok(Some(StopView {
            kind,
            why: declared.why.clone(),
            counter: pair.0,
            limit: pair.1,
            counter_text,
            at: ev.created_at,
            actor: ev.actor.clone(),
            actor_word: reasons.actor_word(&ev.actor),
            reason,
            reason_absent: reasons.absence.reason.clone(),
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

/// The declared sentence for one (parked, continues, finished) combination —
/// the facts stay separate fields; only their wording is data.
fn aftermath_text(
    parked: Option<bool>,
    continues: bool,
    finished: bool,
    words: &LaunchAftermathWords,
) -> String {
    match (parked, continues, finished) {
        (Some(true), false, _) => words.parked.clone(),
        (Some(true), true, _) => words.parked_then_going.clone(),
        (Some(false), true, _) => words.going.clone(),
        (Some(false), false, true) => words.finished.clone(),
        (Some(false), false, false) => words.held_elsewhere.clone(),
        (None, ..) => words.unknown.clone(),
    }
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
    let class = req_str(ev, json_field::FAILURE_CLASS)?;
    let declared: &ClassWords = words(
        reasons.class_words(class),
        "phrase for failure class",
        class,
    )?;
    let consecutive = opt_u32(ev, json_field::CONSECUTIVE)?;
    let limit = opt_u32(ev, json_field::LIMIT)?;
    let parked = opt_bool(ev, json_field::PARKED)?;
    let continues = is_going(task.state);
    let finished = task.state.is_terminal();
    Ok(Some(LaunchFailureView {
        failure_class: class.to_string(),
        class_word: declared.why.clone(),
        at: ev.created_at,
        reason: opt_str(ev, json_field::REASON)?.map(|r| r.to_string()),
        action: declared.action.clone(),
        consecutive,
        limit,
        counter_text: pair_text(consecutive, limit)
            .unwrap_or_else(|| reasons.absence.launch_counter.clone()),
        parked,
        continues,
        finished,
        aftermath: aftermath_text(parked, continues, finished, &reasons.launch_aftermath),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// REQ-PK-06 AC3 (mt/change-isolation): the reader's kind vocabulary is
    /// exactly `stop_kind::ALL`. A brake token needs no row (the payload names
    /// it), every other kind needs one — a kind added to `ALL` alone, or a row
    /// nobody declared, fails HERE instead of silently producing no block.
    #[test]
    fn every_stop_kind_is_produced_by_a_row_or_a_brake_payload() {
        let mut produced: Vec<&str> = stop_kind::BRAKES.to_vec();
        for row in KIND_ROWS {
            match row.source {
                KindSource::Brake => {}
                KindSource::Gate => {
                    produced.push(stop_kind::AGENT_FAILURE);
                    produced.push(stop_kind::QUESTION);
                }
                KindSource::Manual(kind) => produced.push(kind),
            }
        }
        produced.sort_unstable();
        produced.dedup();
        let mut declared: Vec<&str> = stop_kind::ALL.to_vec();
        declared.sort_unstable();
        assert_eq!(
            produced, declared,
            "the event classification and the stop vocabulary must name the same kinds"
        );
    }
}
