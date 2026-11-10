//! Devai role execution runtime.
//!
//! Turns a `RoleDefinition` + workflow inputs into a real agent call and a
//! contract-shaped output artifact. The engine advances a task through its
//! AUTONOMOUS stages by invoking the assigned role, publishing the result,
//! and committing the transition **through `devai-core`** — it never bypasses
//! the workflow. Human gates (APPROVAL, final ACCEPT) stop the engine.

mod adapters;
pub mod context;
pub mod convergence;
mod engine;
pub mod evaluation;
pub mod gate;
pub mod human_view;
pub mod humanize;
pub mod launch;
pub mod liveness;
pub mod probe;
mod prompt;
pub mod reclaim;
pub mod revision;
pub mod tail;
pub mod taskname;
pub mod trace;
pub mod worktree;

pub use adapters::{
    AdapterError, AgentRuntime, ExecOutput, ExecRequest, FormLoopHost, LIVE_STDERR_FILE,
    LIVE_STDOUT_FILE, RunOutcome, artifact_violations, contract_violations, form_repair_section,
    validate_against_contract, validate_reply,
};
pub use context::{REWORK_CONTEXT_LABEL, rework_context};
pub use engine::{Engine, Gate, RunReport, RuntimeError, RuntimeResult, StepOutcome};
pub use launch::{
    ACK_TIMEOUT, LaunchError, LaunchOutcome, LaunchRequest, RUNNER_SUBCOMMAND, run_evidence_dir,
    runner_exe, write_ack,
};
pub use liveness::{
    Liveness, OwnerInfo, boot_time_epoch, check_owner, current_host, current_owner,
};
pub use prompt::{build_system_prompt, build_user_prompt, extract_json_object};
pub use tail::{TAIL_LINES, TailError, read_tail, tail_of, tail_of_bytes, trim_to_tail};
