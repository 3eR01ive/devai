//! BRAKES and PARKING of the declared process (REQ-AR-05/09/13).
//!
//! Split out of `mutations.rs` (rework R2): a launch either happens or the
//! task is PARKED with the reason and the counter — the whole «что делать,
//! когда прогон не состоялся» policy lives here, and the mutation plane only
//! calls it.
//!
//! Two exits of a run are brakes' business:
//! * the ENGINE failed mid-chain — the row is closed FAILED(platform) and a
//!   class declared «паркует сразу» parks the task at its first occurrence;
//! * the LAUNCH did not happen — an event with the class and the CONSECUTIVE
//!   counter (derived from the append-only log, so a restart cannot silently
//!   reset it), and the declared threshold parks the task.
//!
//! Nothing is retried automatically, and nothing auto-continues from a park:
//! the declaration says where a park goes, a human releases it.

use chrono::Utc;

use devai_core::types::{ExecutionStatus, Id, Task, event_kind, failure_class};
use devai_core::workflow::{self, Mutation};
use devai_runtime::RuntimeError;
use devai_store::config::ProcessPolicy;

use crate::mutations::ExecStarted;
use crate::{Api, ApiResult};

/// The stored failure CLASS of a launch that did not happen (REQ-AR-13 AC1):
/// the distinction the class vocabulary exists for, kept at the one door that
/// records it. Nothing was spawned when no runner could be resolved or
/// `spawn` itself failed (`spawn`); a runner that timed out, died without a
/// handshake, refused, or could not be placed outside the caller's cgroup is
/// a PLATFORM-side failure (`platform`).
pub(crate) fn launch_failure_class(e: &devai_runtime::LaunchError) -> &'static str {
    use devai_runtime::LaunchError as L;
    match e {
        L::NoRunner(_) | L::Spawn(_) => failure_class::SPAWN,
        L::Timeout(_) | L::Runner(_) | L::NoScope(_) => failure_class::PLATFORM,
    }
}

impl Api {
    /// Close a failed engine run as FAILED(platform) and honour the declared
    /// «паркующие сразу» classes. The row closure is reported, not swallowed:
    /// a row that cannot be closed is an error the caller must see, since a
    /// RUNNING row is exactly the incident this task exists to end.
    pub(crate) fn close_failed_run(
        &self,
        started: &ExecStarted,
        policy: &ProcessPolicy,
        e: &RuntimeError,
    ) -> ApiResult<()> {
        let id = started.execution.id.to_string();
        let row = self.store.get_execution(&id)?;
        if row.status == ExecutionStatus::Running {
            self.store.finish_execution_classified(
                &row,
                ExecutionStatus::Failed,
                None,
                Some(&format!("engine failed: {e}")),
                Some(failure_class::PLATFORM),
                Utc::now(),
            )?;
        }
        let class = self.store.get_execution(&id)?.failure_class;
        let Some(class) = class.filter(|c| policy.parks_immediately(c)) else {
            return Ok(());
        };
        let cur = self.store.get_task(&started.task.id.to_string())?;
        let reason = format!(
            "failure class `{class}` is declared to park immediately — another round cannot fix \
             it; engine error: {e}"
        );
        // A park-immediately class fires on its FIRST occurrence, so the
        // counter and the limit are both 1 — named so the pair reads as
        // «сработало с первого раза при пороге 1», not as bare literals.
        let occurrence: u32 = 1;
        let limit: u32 = 1;
        self.park_launch_failure(&cur, "park_immediately", occurrence, limit, &reason)
    }

    /// Park the task on a tripped brake: the state change, the reason with
    /// the counter, and the dedicated event. `workflow::block` keeps every
    /// workflow rule (a terminal task cannot be parked, the remembered stage
    /// is kept for a human release) — a park is not a private store write.
    pub(crate) fn park(
        &self,
        task: &Task,
        brake: &str,
        counter: u32,
        limit: u32,
        reason: &str,
    ) -> ApiResult<()> {
        let process = self.process_policy()?;
        let m = workflow::park(&process, task, Some(reason), "platform", Utc::now())?;
        self.commit_park(task, &m, brake, counter, limit, reason)
    }

    /// The same park for a FAILED LAUNCH: the declaration says where a launch
    /// failure sends each role state (`launch_failed`), so the brake address is
    /// only the fallback for a state the process declares no such outcome for.
    fn park_launch_failure(
        &self,
        task: &Task,
        brake: &str,
        counter: u32,
        limit: u32,
        reason: &str,
    ) -> ApiResult<()> {
        let process = self.process_policy()?;
        let m =
            workflow::park_on_launch_failure(&process, task, Some(reason), "platform", Utc::now())?;
        self.commit_park(task, &m, brake, counter, limit, reason)
    }

    /// The stored park plus its dedicated event (brake, counter, limit,
    /// reason) — one body for both park kinds.
    fn commit_park(
        &self,
        task: &Task,
        m: &Mutation,
        brake: &str,
        counter: u32,
        limit: u32,
        reason: &str,
    ) -> ApiResult<()> {
        self.store.commit(task.lock_version, m)?;
        self.store.insert_event(
            Some(task.id),
            event_kind::PROCESS_PARKED,
            "platform",
            serde_json::json!({
                "brake": brake,
                "counter": counter,
                "limit": limit,
                "reason": reason,
            }),
        )?;
        Ok(())
    }

    /// REQ-AR-13: a launch that did not happen is recorded as an event with
    /// its class and the CONSECUTIVE counter (derived from the append-only
    /// log, so a restart cannot silently reset it), and parks the task at the
    /// declared threshold. Nothing is retried automatically.
    pub(crate) fn record_launch_failure(
        &self,
        task: &Task,
        class: &'static str,
        reason_text: &str,
    ) -> ApiResult<()> {
        let policy = self.process_policy()?;
        let consecutive = self.consecutive_launch_failures(task.id)? + 1;
        let limit = policy.brakes.max_consecutive_failures;
        let mut parked = false;
        if consecutive >= limit {
            let reason = format!(
                "launch failed {consecutive} time(s) in a row (limit {limit}): {reason_text}"
            );
            self.park_launch_failure(
                task,
                "consecutive_launch_failures",
                consecutive,
                limit,
                &reason,
            )?;
            parked = true;
        }
        self.store.insert_event(
            Some(task.id),
            event_kind::EXECUTION_LAUNCH_FAILED,
            "platform",
            serde_json::json!({
                "failure_class": class,
                "consecutive": consecutive,
                "limit": limit,
                "parked": parked,
                "reason": reason_text,
            }),
        )?;
        Ok(())
    }

    /// The tail of the append-only log: how many launch failures sit in a row
    /// right now. Derived, never stored separately — a counter field could
    /// drift from the events that justify it.
    fn consecutive_launch_failures(&self, task_id: Id) -> ApiResult<u32> {
        let events = self.store.events(task_id, None)?;
        let n = events
            .iter()
            .rev()
            .take_while(|ev| ev.kind == event_kind::EXECUTION_LAUNCH_FAILED)
            .count();
        Ok(n as u32)
    }
}
