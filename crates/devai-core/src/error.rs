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
