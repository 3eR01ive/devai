//! Devai role execution runtime.
//!
//! Turns a `RoleDefinition` + workflow inputs into a real agent call and a
//! contract-shaped output artifact. The engine advances a task through its
//! AUTONOMOUS stages by invoking the assigned role, publishing the result,
//! and committing the transition **through `devai-core`** — it never bypasses
//! the workflow. Human gates (APPROVAL, final ACCEPT) stop the engine.

mod adapters;
pub mod context;
mod engine;
pub mod evaluation;
pub mod gate;
pub mod liveness;
mod prompt;
pub mod revision;
pub mod worktree;

pub use adapters::{
    AdapterError, AgentRuntime, ExecOutput, ExecRequest, FormLoopHost, RunOutcome,
    contract_violations, form_repair_section, validate_against_contract,
};
pub use context::{REWORK_CONTEXT_LABEL, rework_context};
pub use engine::{Engine, Gate, RunReport, RuntimeError, RuntimeResult, StepOutcome};
pub use liveness::{Liveness, OwnerInfo, check_owner, current_host, current_owner};
pub use prompt::{build_system_prompt, build_user_prompt, extract_json_object};
