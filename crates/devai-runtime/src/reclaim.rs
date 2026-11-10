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
//! Where it runs (grep `reclaim::reclaim`): both launch paths — `task_run`
//! (`mutations.rs`) and the supervised admission (`launch_admission.rs`), so a
//! corpse never blocks a new launch — and `devai exec reap` (`Api::exec_reap`,
//! `lib.rs`). It is not a background daemon — the platform has no daemon
//! (ASM-5).

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
    let boot = boot_time_epoch();
    let mut closed = Vec::new();
    for e in running_rows(store, task)? {
        let Some((class, evidence)) = classify(&e, boot) else {
            continue;
        };
        close_row(store, &e, class, &evidence)?;
        closed.push(Reclaimed {
            execution: e,
            class,
            evidence,
        });
    }
    Ok(closed)
}

/// The RUNNING rows in scope — one task's, or every home's.
fn running_rows(
    store: &Store,
    task: Option<devai_core::types::Id>,
) -> StoreResult<Vec<RoleExecution>> {
    Ok(match task {
        Some(t) => store
            .list_executions(Some(t))?
            .into_iter()
            .filter(|e| e.status == ExecutionStatus::Running)
            .collect(),
        None => store.running_executions()?,
    })
}

/// WHY this row is provably dead, or `None` when its fate is not provable.
/// The three classes are the ratified OQ-4 answers; everything else — another
/// host, a missing `/proc`, an uncomparable clock — is left to a human.
fn classify(e: &RoleExecution, boot: Option<i64>) -> Option<(&'static str, String)> {
    let (pid, host, start) = (e.owner_pid, e.owner_host.as_deref(), e.owner_process_start);
    if pid.is_none() && host.is_none() {
        return Some((
            failure_class::LOST_BEFORE_OWNERSHIP,
            "RUNNING row with no owner recorded (written before the platform owned \
             processes) — closed by the ratified «потеряно до введения владения» rule"
                .to_string(),
        ));
    }
    match check_owner(pid, host, start) {
        Liveness::Alive | Liveness::Unknown => None,
        Liveness::Dead => Some(dead_class(e, pid, host, start, boot)),
    }
}

/// The class of a row whose owner is gone on THIS host: a reboot since the row
/// started is a different fact from a vanished process.
fn dead_class(
    e: &RoleExecution,
    pid: Option<i64>,
    host: Option<&str>,
    start: Option<i64>,
    boot: Option<i64>,
) -> (&'static str, String) {
    let restarted = boot.map(|b| e.started_at.timestamp() < b).unwrap_or(false);
    if restarted {
        return (
            failure_class::HOST_RESTARTED,
            format!(
                "owner pid={pid:?} started {} — BEFORE this boot ({}); the host restarted, \
                 nothing of this row can still run",
                e.started_at.to_rfc3339(),
                boot.map(|b| b.to_string()).unwrap_or_else(|| "?".into())
            ),
        );
    }
    (
        failure_class::PROCESS_VANISHED,
        format!(
            "owner pid={pid:?} host={host:?} start={start:?} is gone on this host (no /proc entry \
             or a recycled pid) and the host did not reboot since the row started"
        ),
    )
}

/// Close the row as FAILED with the class and say so in the event log. The
/// slot is freed; the role is not re-run (no automatic retries).
fn close_row(
    store: &Store,
    e: &RoleExecution,
    class: &'static str,
    evidence: &str,
) -> StoreResult<()> {
    store.finish_execution_classified(
        e,
        ExecutionStatus::Failed,
        None,
        Some(&format!(
            "platform closed this execution: {class} — {evidence}. The slot is free; the role is \
             not re-run automatically (no automatic retries)."
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
    )
}
