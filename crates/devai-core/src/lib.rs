//! Devai domain core: entities, workflow state machine and sovereignty
//! rules. This crate performs no IO: persistence lives in `devai-store`,
//! interfaces (CLI / REST / MCP) only translate user intent into commands
//! and render domain results.

pub mod error;
pub mod types;
pub mod workflow;

pub use error::{CoreError, CoreResult};
pub use types::{
    AgentInstance, Artifact, ArtifactKind, ArtifactStatus, ContractRef, DataCategory, Decision,
    DependencyKind, DependencyNode, EventDraft, ExecutionStatus, Id, Project, Question,
    QuestionStatus, RoleDefinition, RoleExecution, Task, TaskDependency, TaskState, Timestamp,
    Verdict, WorkflowEvent, json_array, json_field, json_parse, json_string, new_id,
};
pub use workflow::{Mutation, allowed_ops, check_execution_allowed, open_blockers, role_allowed_states};
