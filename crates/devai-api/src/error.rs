//! Programmatic error surface of the platform API (REQ-API-06 AC3).
//!
//! Every API failure carries a machine-readable `code()` beside its human
//! `Display` text, so a non-CLI consumer can branch on the category without
//! parsing messages. The CLI keeps rendering the text (parity with the
//! pre-API exit path, REQ-API-04 AC2: explicit message, nonzero exit).

use devai_core::CoreError;
use devai_runtime::{AdapterError, RuntimeError};
use devai_store::StoreError;

/// The category an error belongs to — a stable machine token, not the Rust
/// variant name. Consumers may match on it across API versions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ErrorCode(pub &'static str);

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ApiError {
    #[error(transparent)]
    Store(#[from] StoreError),
    #[error(transparent)]
    Core(#[from] CoreError),
    #[error(transparent)]
    Runtime(#[from] RuntimeError),
    #[error(transparent)]
    Adapter(#[from] AdapterError),
    /// A caller-supplied argument the API rejected before touching the store.
    #[error("bad request: {0}")]
    BadRequest(String),
    /// Artifact content that cannot be presented to a human as text.
    #[error("artifact content is not valid UTF-8")]
    NotUtf8,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

impl ApiError {
    /// Programmatic category (REQ-API-06 AC3). The mapping is exhaustive per
    /// underlying variant so a consumer never has to guess from the text.
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Store(e) => match e {
                StoreError::NotFound(..) => ErrorCode("not_found"),
                StoreError::Ambiguous(..) => ErrorCode("ambiguous"),
                StoreError::Conflict { .. } => ErrorCode("conflict"),
                StoreError::SingleWriterBusy { .. } => ErrorCode("single_writer_busy"),
                StoreError::SingleCurrentCollision { .. } => ErrorCode("single_current_collision"),
                StoreError::InvalidDependency(..) => ErrorCode("invalid_dependency"),
                StoreError::SchemaVersion { .. } => ErrorCode("schema_version"),
                StoreError::Corrupt(..) => ErrorCode("corrupt"),
                StoreError::Config(..) => ErrorCode("config"),
                StoreError::Io(..) => ErrorCode("io"),
                StoreError::Sql(..) => ErrorCode("store"),
            },
            Self::Core(e) => match e {
                CoreError::InvalidTransition { .. } => ErrorCode("invalid_transition"),
                CoreError::Guard(..) => ErrorCode("guard"),
                CoreError::UnknownVariant { .. } => ErrorCode("unknown_variant"),
                CoreError::OpenBlockers { .. } => ErrorCode("open_blockers"),
            },
            Self::Runtime(..) => ErrorCode("runtime"),
            Self::Adapter(..) => ErrorCode("contract_gate"),
            Self::BadRequest(..) => ErrorCode("bad_request"),
            Self::NotUtf8 => ErrorCode("not_utf8"),
            Self::Io(..) => ErrorCode("io"),
        }
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_are_machine_readable_and_distinct() {
        // REQ-API-06 AC3: a consumer branches on code(), not on text.
        let e: ApiError = StoreError::NotFound("task".into(), "deadbeef".into()).into();
        assert_eq!(e.code(), ErrorCode("not_found"));
        let e2: ApiError = devai_core::CoreError::guard("gate").into();
        assert_eq!(e2.code(), ErrorCode("guard"));
        let e3 = ApiError::BadRequest("x".into());
        assert_eq!(e3.code(), ErrorCode("bad_request"));
        // Display stays human-readable and carries the inner text (CLI parity).
        assert!(format!("{e}").contains("task `deadbeef` not found"));
    }
}
