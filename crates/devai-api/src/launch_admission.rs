//! ADMISSION of a launch (REQ-AR-06/11/13): may a run start at all?
//!
//! Split out of `mutations.rs` (rework R2). The mutation plane ORCHESTRATES a
//! launch — mint the run number, hand the request to the runner, settle the
//! answer — while THIS module answers the questions that come before any
//! process is spawned:
//!
//! * does the declaration give this state a role at all (REQ-AR-11 AC2: a
//!   human/none state launches nothing — a typed «нельзя», not a failure);
//! * is there an executable agent for the stage (REQ-AR-08: no id without a
//!   live process);
//! * is a declared brake tripped (REQ-AR-09/13: rounds without a shift park
//!   the task instead of launching again);
//! * is the task already owned (REQ-AR-06: one owner per task, the refusal
//!   names the owner and touches nothing).
//!
//! Every «no» travels through the ack file, so a caller that has already
//! returned still receives it, and none of them touches a running chain.

use devai_core::types::{ExecutionStatus, Id, Task, TaskState};
use devai_store::config::ProcessPolicy;

use devai_runtime::launch;
use devai_runtime::{liveness, reclaim};

use crate::mutations::SupervisedRunReq;
use crate::{Api, ApiResult};

/// The ONE wording of «this state is not executed by a role» (rework R3): the
/// caller-side early answer and the runner's ack say the same words, so the
/// operator reads one reason instead of two similar ones. The two PATHS stay
/// separate (an early «нельзя» before anything was tried, a refusal handed
/// back by the runner) — only the text is shared.
pub(crate) fn no_role_reason(state: &str, executor: &str) -> String {
    format!("state {state} is executed by `{executor}`, not by a role — nothing to launch")
}

impl Api {
    /// «Состояние с человеком прогона не порождает» (REQ-AR-11 AC2): the
    /// declaration says who executes the state; a human/none state is a typed
    /// «нельзя», NOT a launch failure (nothing was tried). A DRAFT is the one
    /// bookkeeping step the runner performs before the first role opens, so it
    /// is launchable.
    pub(crate) fn state_launch_refusal(&self, task: &Task) -> ApiResult<Option<String>> {
        if task.state == TaskState::Draft {
            return Ok(None);
        }
        let policy = self.process_policy()?;
        let state = task.state.as_str();
        if policy.role_for(state).is_some() {
            return Ok(None);
        }
        let executor = policy.executor_for(state).unwrap_or("none");
        Ok(Some(no_role_reason(state, executor)))
    }

    /// ADMISSION (REQ-AR-06/11/13): does this state have a role, is there an
    /// executable agent, is a brake tripped, is the task already owned? Every
    /// «no» travels through the ack, so a caller that already returned still
    /// receives it — and none of them touches a running chain.
    pub(crate) fn admit_launch(
        &self,
        task: &Task,
        policy: &ProcessPolicy,
        req: &SupervisedRunReq,
    ) -> ApiResult<Option<String>> {
        let state = task.state.as_str().to_string();
        let Some(role) = policy.role_for(&state).map(|r| r.to_string()) else {
            let executor = policy.executor_for(&state).unwrap_or("none");
            self.refuse_launch(req, &no_role_reason(&state, executor));
            return Ok(None);
        };
        if let Some(reason) = self.agent_preflight(req)? {
            self.refuse_launch(req, &reason);
            return Ok(None);
        }
        // REQ-AR-05: prove-dead rows are closed before the guard asks.
        reclaim::reclaim(&self.store, Some(task.id))?;
        let task = self.store.resolve_task(&req.task_id)?;
        if let Some(reason) = self.brake_reason(&task, policy, &role)? {
            self.refuse_launch(req, &reason);
            return Ok(None);
        }
        if let Some(owner) = self.live_owner(&task)? {
            // REQ-AR-06: the refusal NAMES the execution that holds the task.
            let _ = launch::write_ack(&req.ack, &format!("busy {} {owner}", req.run_id));
            return Ok(None);
        }
        Ok(Some(role))
    }

    /// Hand a refusal back through the handshake (best effort: the caller may
    /// have given up, and the supervisor log keeps the reason either way).
    fn refuse_launch(&self, req: &SupervisedRunReq, reason: &str) {
        let _ = launch::write_ack(&req.ack, &format!("refused {reason}"));
    }

    /// REQ-AR-08: the launch is indivisible — an execution id is handed out
    /// only when the role can actually run. An EMPTY agent means «the marked
    /// model bundle»; with none marked the platform refuses BEFORE opening a
    /// RUNNING row, not leaves a doomed row holding the slot.
    fn agent_preflight(&self, req: &SupervisedRunReq) -> ApiResult<Option<String>> {
        if !req.agent.trim().is_empty() {
            return Ok(None);
        }
        let usable = matches!(
            self.store.active_agent()?,
            Some(a) if !a.reasoning.trim().is_empty()
        );
        Ok((!usable).then(|| {
            "no active model bundle — the platform has no executable agent for this state; pick \
             one: `devai model switch <id>` (or run the role with an explicit --agent)"
                .to_string()
        }))
    }

    /// BRAKE: rounds without a shift (REQ-AR-09/13). A round is an execution of
    /// the same stage role since the task's last NEW artifact; at the declared
    /// count the task parks with the reason and counter, and nothing
    /// auto-continues from a park.
    fn brake_reason(
        &self,
        task: &Task,
        policy: &ProcessPolicy,
        role: &str,
    ) -> ApiResult<Option<String>> {
        let last_artifact = self
            .store
            .artifacts(task.id)?
            .iter()
            .map(|a| a.created_at)
            .max();
        let rounds = self
            .store
            .list_executions(Some(task.id))?
            .into_iter()
            .filter(|e| {
                e.role_slug == role && last_artifact.map(|t| e.started_at > t).unwrap_or(true)
            })
            .count() as u32;
        let limit = policy.brakes.max_rounds_without_shift;
        if rounds < limit {
            return Ok(None);
        }
        let reason =
            format!("brake: {rounds} round(s) of `{role}` without a new artifact (limit {limit})");
        self.park(task, "rounds_without_shift", rounds, limit, &reason)?;
        Ok(Some(reason))
    }

    /// REQ-AR-06: «идёт прогон» is wider than «есть строка RUNNING». Between
    /// two stages of one chain there is a moment with no RUNNING row while the
    /// runner is very much alive — the owner PROCESS is the truth, so a row
    /// whose owner is alive means the chain has not let go of the task.
    fn live_owner(&self, task: &Task) -> ApiResult<Option<Id>> {
        Ok(self
            .store
            .list_executions(Some(task.id))?
            .into_iter()
            .rev()
            .find(|e| {
                e.status == ExecutionStatus::Running
                    || liveness::check_owner(
                        e.owner_pid,
                        e.owner_host.as_deref(),
                        e.owner_process_start,
                    ) == liveness::Liveness::Alive
            })
            .map(|e| e.id))
    }
}
