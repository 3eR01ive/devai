use thiserror::Error;

/// Errors produced by pure domain logic (workflow sovereignty checks).
#[derive(Debug, Error)]
pub enum CoreError {
    /// The requested operation is not allowed in the current workflow state.
    /// This is the enforcement point of "workflow belongs to the platform".
    #[error("operation `{op}` is not allowed in state {from}{}", hint.as_ref().map(|h| format!(": {h}")).unwrap_or_default())]
    InvalidTransition {
        op: &'static str,
        from: String,
        hint: Option<String>,
    },

    #[error("workflow violation: {0}")]
    Guard(String),

    #[error("unknown {} variant: `{value}`", ty.to_lowercase())]
    UnknownVariant { ty: &'static str, value: String },

    /// REQ-03: continuation (resume from a hold state) and human acceptance
    /// are refused while the task has OPEN `blocks` dependencies. The
    /// blocking task ids are carried as structured data — the refusal must
    /// enumerate them, not merely name one cause. The only release is the
    /// operator's own `dependency_override` decision (ratified OQ2), checked
    /// before this error is produced — reaching it means no ratification
    /// covers every open blocker.
    #[error("open blocking dependencies: {}", blockers.join(", "))]
    OpenBlockers { blockers: Vec<String> },
}

impl CoreError {
    pub fn guard(msg: impl Into<String>) -> Self {
        Self::Guard(msg.into())
    }

    pub fn hint(msg: impl Into<String>) -> Option<String> {
        Some(msg.into())
    }
}

pub type CoreResult<T> = Result<T, CoreError>;
