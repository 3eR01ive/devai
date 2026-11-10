//! OBSERVATION of a launch: the handshake with the caller, the launch record,
//! and the run's evidence (REQ-AR-03/07/08).
//!
//! Split out of `mutations.rs` (rework R2). The mutation plane drives the
//! chain; THIS module owns what the platform KEEPS about it, so the facts a
//! later reader sees never depend on the process that started the run:
//!
//! * the handshake — the execution id is handed to the caller ONLY after the
//!   row exists (REQ-AR-08: no handle without a process);
//! * the immutable launch event — run, row, and the owner columns that make
//!   a later absence PROVABLE;
//! * the settled outcome in the run's evidence dir, which a `--wait` caller
//!   (or any later reader) reads back without re-running anything, and the
//!   row closure of a chain that stopped before a stage started.
//!
//! Best-effort writes here leave a TRACE on failure (rework R5): the role's
//! answer is never lost because a log could not be written, but a swallowed
//! error must not be invisible either.

use std::path::Path;

use chrono::Utc;

use devai_core::types::{ExecutionStatus, event_kind};
use devai_runtime::trace;
use devai_runtime::{Gate, RunReport, launch};

use crate::mutations::{ExecStarted, SupervisedRunReq};
use crate::{Api, ApiResult};

impl Api {
    /// The handshake: `started <run> <exec>`. Failing to deliver it is a
    /// launch failure — the caller must not be left waiting on a phantom.
    pub(crate) fn hand_over_handle(
        &self,
        req: &SupervisedRunReq,
        started: &ExecStarted,
    ) -> ApiResult<()> {
        launch::write_ack(
            &req.ack,
            &format!("started {} {}", req.run_id, started.execution.id),
        )
        .map_err(|e| {
            crate::ApiError::refused(
                "launch_failed",
                format!("cannot hand the execution id to the caller: {e}"),
            )
        })
    }

    /// The immutable record of the launch: which run, which row, which owner
    /// process. The owner columns are what makes the absence PROVABLE later.
    pub(crate) fn record_launch_event(
        &self,
        task: &devai_core::types::Task,
        role: &str,
        req: &SupervisedRunReq,
        started: &ExecStarted,
    ) -> ApiResult<()> {
        self.store.insert_event(
            Some(task.id),
            event_kind::EXECUTION_LAUNCHED,
            "platform",
            serde_json::json!({
                "run_id": req.run_id,
                "execution_id": started.execution.id.to_string(),
                "role": role,
                "agent": started.execution.agent_name,
                "owner_pid": started.execution.owner_pid,
                "owner_host": started.execution.owner_host,
                "owner_process_start": started.execution.owner_process_start,
            }),
        )?;
        Ok(())
    }

    /// REQ-AR-08/05: the handle row must never be left RUNNING. The engine
    /// adopts it for the FIRST stage — but a gate that stops the run BEFORE any
    /// stage (a FRESH quality report, a human gate, a hold) leaves it
    /// untouched. Close it as a completed no-work run whose summary names the
    /// gate, so the slot is free and the row explains itself.
    pub(crate) fn settle_open_row(&self, started: &ExecStarted, gate: Gate) -> ApiResult<()> {
        let row = self
            .store
            .get_execution(&started.execution.id.to_string())?;
        if row.status != ExecutionStatus::Running {
            return Ok(());
        }
        self.store.finish_execution_classified(
            &row,
            ExecutionStatus::Completed,
            Some(&format!(
                "run stopped at {gate:?} before the stage started — no role work was done"
            )),
            None,
            None,
            Utc::now(),
        )?;
        Ok(())
    }

    /// REQ-AR-03/07: the run's OUTCOME belongs to the platform and outlives the
    /// caller. Persist the same gate view a synchronous run rendered into the
    /// run's evidence dir, so `--wait` (and any later reader) sees the settled
    /// report without re-running anything.
    pub(crate) fn persist_outcome(
        &self,
        report: &RunReport,
        req: &SupervisedRunReq,
    ) -> ApiResult<()> {
        let view = self.run_view(report.clone(), Some(req.run_id.clone()))?;
        let path = launch::run_evidence_dir(&self.home, &req.task_id, &req.run_id)
            .join(launch::OUTCOME_FILE);
        let text = match serde_json::to_string_pretty(&view) {
            Ok(text) => text,
            Err(e) => return self.record_unwritten_outcome(req, &path, &e.to_string()),
        };
        match std::fs::write(&path, text) {
            Ok(()) => Ok(()),
            // R5: best effort stays — the execution rows still explain the run
            // — but the failure is a FACT with the path and the reason.
            Err(e) => self.record_unwritten_outcome(req, &path, &e.to_string()),
        }
    }

    /// The trace of an outcome that did not land (R5): the platform journal
    /// line plus a stored event, so the operator can SEE that `--wait` will
    /// fall back to the row listing instead of reading a missing file.
    fn record_unwritten_outcome(
        &self,
        req: &SupervisedRunReq,
        path: &Path,
        reason: &str,
    ) -> ApiResult<()> {
        trace::note_failure("run outcome", path, &reason);
        let task = self.store.resolve_task(&req.task_id)?;
        self.store.insert_event(
            Some(task.id),
            event_kind::EXECUTION_EVIDENCE_UNWRITTEN,
            "platform",
            serde_json::json!({
                "run_id": req.run_id,
                "what": launch::OUTCOME_FILE,
                "path": path.display().to_string(),
                "reason": reason,
            }),
        )?;
        Ok(())
    }
}
