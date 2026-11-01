//! Devai role execution runtime.
//!
//! Turns a `RoleDefinition` + workflow inputs into a real agent call and a
//! contract-shaped output artifact. The engine advances a task through its
//! AUTONOMOUS stages by invoking the assigned role, publishing the result,
//! and committing the transition **through `devai-core`** — it never bypasses
//! the workflow. Human gates (APPROVAL, final ACCEPT) stop the engine.

mod adapters;
mod engine;
pub mod evaluation;
mod prompt;

pub use adapters::{AdapterError, AgentRuntime, ExecOutput, ExecRequest};
pub use engine::{Engine, Gate, RunReport, RuntimeError, RuntimeResult, StepOutcome};
pub use prompt::{build_system_prompt, build_user_prompt, extract_json_object};
