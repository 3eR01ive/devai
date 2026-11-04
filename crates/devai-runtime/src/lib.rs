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
    contract_violations, form_repair_section, validate_against_contract, AdapterError, AgentRuntime, ExecOutput, ExecRequest, FormLoopHost, RunOutcome,
};
pub use context::{rework_context, REWORK_CONTEXT_LABEL};
pub use engine::{Engine, Gate, RunReport, RuntimeError, RuntimeResult, StepOutcome};
pub use liveness::{check_owner, current_host, current_owner, Liveness, OwnerInfo};
pub use prompt::{build_system_prompt, build_user_prompt, extract_json_object};
