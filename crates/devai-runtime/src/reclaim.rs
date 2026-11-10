//! Closing executions whose owner process is PROVABLY gone (REQ-AR-05).
//!
//! The 2026-11-07 incidents left RUNNING rows behind a dead child: the row
//! held the single-writer slot and every later launch was refused by a corpse.
//! The owner columns (pid + process start + host, REQ-SW-03) make the absence
//! PROVABLE, and this module acts on that proof:
//!
//! * pid gone on THIS host, row started after this boot → `process_vanished`;
//! * row started BEFORE this boot → `host_restarted`;
//! * RUNNING row with no owner recorded at all (pre-v7) → the ratified OQ-4
//!   answer, `lost_before_ownership`.
//!
//! Anything unprovable — another host, a missing `/proc`, an uncomparable
//! clock — is left UNTOUCHED: «недоказуемое не выдаётся ни за смерть, ни за
//! жизнь», the slot stays held and the refusal names the execution for a
//! human to close. Nothing here kills a process or sends a signal
//! (REQ-SW-06 preserved: the check is an observation, the write is a row
//! closure).
//!
//! Where it runs: the launch path (before a new run starts, so a corpse never
//! blocks it), `devai exec reap`, and the active-work snapshot. It is not a
//! background daemon — the platform has no daemon (ASM-5).

use chrono::Utc;
use devai_core::types::{ExecutionStatus, RoleExecution, event_kind, failure_class};
use devai_store::{Store, StoreResult};

use crate::liveness::{Liveness, boot_time_epoch, check_owner};

/// One execution the platform closed, with the class it recorded.
#[derive(Debug, Clone)]
pub struct Reclaimed {
    pub execution: RoleExecution,
    pub class: &'static str,
    /// The observation that justified the closure, as text (evidence payload).
    pub evidence: String,
}

/// Close every provably dead RUNNING execution (optionally of one task).
/// Returns the rows actually closed — an empty result means «nothing was
/// provable», never «nothing was wrong».
pub fn reclaim(store: &Store, task: Option<devai_core::types::Id>) -> StoreResult<Vec<Reclaimed>> {
    let running = match task {
        Some(t) => store
            .list_executions(Some(t))?
            .into_iter()
            .filter(|e| e.status == ExecutionStatus::Running)
            .collect::<Vec<_>>(),
        None => store.running_executions()?,
    };
    let boot = boot_time_epoch();
    let mut closed = Vec::new();
    for e in running {
        let (class, evidence) = match (e.owner_pid, e.owner_host.as_deref(), e.owner_process_start)
        {
            (None, None, _) => (
                failure_class::LOST_BEFORE_OWNERSHIP,
                "RUNNING row with no owner recorded (written before the platform owned \
                 processes) — closed by the ratified «потеряно до введения владения» rule"
                    .to_string(),
            ),
            (pid, host, start) => match check_owner(pid, host, start) {
                Liveness::Dead => {
                    let started_epoch = e.started_at.timestamp();
                    let restarted = boot.map(|b| started_epoch < b).unwrap_or(false);
                    if restarted {
                        (
                            failure_class::HOST_RESTARTED,
                            format!(
                                "owner pid={pid:?} started {} — BEFORE this boot ({}); the host \
                                 restarted, nothing of this row can still run",
                                e.started_at.to_rfc3339(),
                                boot.map(|b| b.to_string()).unwrap_or_else(|| "?".into())
                            ),
                        )
                    } else {
                        (
                            failure_class::PROCESS_VANISHED,
                            format!(
                                "owner pid={pid:?} host={host:?} start={start:?} is gone on this \
                                 host (no /proc entry or a recycled pid) and the host did not \
                                 reboot since the row started"
                            ),
                        )
                    }
                }
                // Alive, or NOT provable (another host / no /proc). Never
                // guessed dead — the human closes an unprovable row.
                Liveness::Alive | Liveness::Unknown => continue,
            },
        };
        store.finish_execution_classified(
            &e,
            ExecutionStatus::Failed,
            None,
            Some(&format!(
                "platform closed this execution: {class} — {evidence}. The slot is free; the \
                 role is not re-run automatically (no automatic retries)."
            )),
            Some(class),
            Utc::now(),
        )?;
        store.insert_event(
            Some(e.task_id),
            event_kind::EXECUTION_LOST,
            "platform",
            serde_json::json!({
                "execution_id": e.id.to_string(),
                "role": e.role_slug,
                "failure_class": class,
                "owner_pid": e.owner_pid,
                "owner_host": e.owner_host,
                "owner_process_start": e.owner_process_start,
                "evidence": evidence,
            }),
        )?;
        closed.push(Reclaimed {
            execution: e,
            class,
            evidence,
        });
    }
    Ok(closed)
}
