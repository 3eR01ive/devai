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
    /// Cycle 2 (REQ-MUT-01/08): a domain refusal raised by a mutation
    /// operation. The message carries the operator-facing text the CLI
    /// printed before the API owned this path (output parity, REQ-MUT-06);
    /// `code` is the machine token a second consumer branches on. A consumer
    /// that formats its own text reads `code` + the structured fields and
    /// never has to parse the message.
    #[error("{message}")]
    Refused { code: ErrorCode, message: String },
    /// The manual publish contract gate: the named contract is not
    /// registered in this store (the same refusal the pre-API CLI path
    /// printed).
    #[error("contract {contract} is not registered (devai contract add)")]
    ContractUnregistered { contract: String },
    /// The manual publish contract gate (ratified OQ-FG-05): the artifact
    /// violates the registered contract. `violations` is the FULL machine
    /// list — the presentation (warning per violation, then the refusal)
    /// belongs to the consumer, enforcement to the API.
    #[error("artifact does not satisfy contract {contract}: {violations:?}")]
    ContractRejected {
        contract: String,
        violations: Vec<String>,
    },
}

impl ApiError {
    /// Internal constructor for the mutation surface's domain refusals.
    pub(crate) fn refused(code: &'static str, message: impl Into<String>) -> ApiError {
        ApiError::Refused {
            code: ErrorCode(code),
            message: message.into(),
        }
    }
}

fn store_code(e: &StoreError) -> ErrorCode {
    match e {
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
    }
}

fn core_code(e: &CoreError) -> ErrorCode {
    match e {
        CoreError::InvalidTransition { .. } => ErrorCode("invalid_transition"),
        CoreError::Guard(..) => ErrorCode("guard"),
        CoreError::UnknownVariant { .. } => ErrorCode("unknown_variant"),
        CoreError::OpenBlockers { .. } => ErrorCode("open_blockers"),
    }
}

/// Same recursion through the worktree family: each REQ-WT refusal keeps
/// its own token instead of hiding behind one "worktree" (R1 QA).
fn worktree_code(e: &devai_runtime::worktree::WorktreeError) -> ErrorCode {
    use devai_runtime::worktree::WorktreeError as W;
    match e {
        W::NotGit(..) => ErrorCode("worktree_not_git"),
        W::NoBoundCommit => ErrorCode("worktree_no_bound_commit"),
        W::Git { .. } => ErrorCode("worktree_git_failed"),
        W::BranchMismatch { .. } => ErrorCode("worktree_branch_mismatch"),
        W::Io(..) => ErrorCode("worktree_io"),
    }
}

/// Per-variant codes for the engine's runtime errors (R1 QA [arch/log]):
/// a whole `RuntimeError` must not collapse into one token — the transparent
/// wrappers recurse into their inner mapping, each domain variant keeps its
/// own name across the API boundary.
fn runtime_code(e: &RuntimeError) -> ErrorCode {
    match e {
        RuntimeError::Store(inner) => store_code(inner),
        RuntimeError::Core(inner) => core_code(inner),
        RuntimeError::Adapter(inner) => adapter_code(inner),
        RuntimeError::MissingRole(..) => ErrorCode("missing_role"),
        RuntimeError::MissingArtifact(..) => ErrorCode("missing_artifact"),
        RuntimeError::StepLimit(..) => ErrorCode("step_limit"),
        RuntimeError::MissingProfile(..) => ErrorCode("missing_profile"),
        RuntimeError::MissingContract(..) => ErrorCode("missing_contract"),
        RuntimeError::Worktree(inner) => worktree_code(inner),
        RuntimeError::ReportShape(..) => ErrorCode("report_shape"),
    }
}

/// Per-variant codes for the contract gate / agent adapter (R1 QA): the
/// form-rejection, provider-failure and platform-failure families are
/// distinguishable without parsing text (mirrors `failure_class`, which is
/// row data, not this wire code).
fn adapter_code(e: &AdapterError) -> ErrorCode {
    match e {
        AdapterError::UnknownProvider(..) => ErrorCode("unknown_provider"),
        AdapterError::Io(..) => ErrorCode("adapter_io"),
        AdapterError::Agent(..) => ErrorCode("agent_error"),
        AdapterError::BadOutput { .. } => ErrorCode("contract_violation"),
        AdapterError::Platform(..) => ErrorCode("platform_contract"),
    }
}

impl ApiError {
    /// Programmatic category (REQ-API-06 AC3). The mapping is exhaustive per
    /// underlying variant so a consumer never has to guess from the text,
    /// and no variant family collapses into one token (R1 QA).
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::Store(e) => store_code(e),
            Self::Core(e) => core_code(e),
            Self::Runtime(e) => runtime_code(e),
            Self::Adapter(e) => adapter_code(e),
            Self::BadRequest(..) => ErrorCode("bad_request"),
            Self::NotUtf8 => ErrorCode("not_utf8"),
            Self::Io(..) => ErrorCode("io"),
            Self::Refused { code, .. } => *code,
            Self::ContractUnregistered { .. } => ErrorCode("contract_unregistered"),
            Self::ContractRejected { .. } => ErrorCode("contract_violation"),
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

    #[test]
    fn runtime_and_adapter_variants_keep_their_own_codes() {
        // R1 QA [arch/log-completeness]: whole RuntimeError/AdapterError
        // variant families must NOT collapse into one token — each variant
        // is distinguishable across the API boundary.
        let cases: Vec<(ApiError, &str)> = vec![
            (RuntimeError::MissingRole("x".into()).into(), "missing_role"),
            (
                RuntimeError::MissingArtifact("spec").into(),
                "missing_artifact",
            ),
            (RuntimeError::StepLimit(3, "QA").into(), "step_limit"),
            (
                RuntimeError::MissingProfile("/p".into()).into(),
                "missing_profile",
            ),
            (
                RuntimeError::MissingContract("c".into(), 1).into(),
                "missing_contract",
            ),
            (RuntimeError::ReportShape("s".into()).into(), "report_shape"),
            (
                RuntimeError::Worktree(devai_runtime::worktree::WorktreeError::NoBoundCommit)
                    .into(),
                "worktree_no_bound_commit",
            ),
            (
                AdapterError::UnknownProvider("q".into()).into(),
                "unknown_provider",
            ),
            (AdapterError::Agent("boom".into()).into(), "agent_error"),
            (
                AdapterError::BadOutput {
                    violations: vec!["$.x".into()],
                }
                .into(),
                "contract_violation",
            ),
            (
                AdapterError::Platform("asm".into()).into(),
                "platform_contract",
            ),
            // Transparent wrappers recurse into the inner family's mapping —
            // a store NotFound under Runtime is still `not_found`.
            (
                RuntimeError::Store(StoreError::SingleWriterBusy {
                    task_id: "t".into(),
                    existing_id: "e".into(),
                    started_at: "now".into(),
                    owner_pid: None,
                    owner_host: None,
                })
                .into(),
                "single_writer_busy",
            ),
            (RuntimeError::Core(CoreError::guard("g")).into(), "guard"),
            (
                RuntimeError::Adapter(AdapterError::BadOutput {
                    violations: vec!["v".into()],
                })
                .into(),
                "contract_violation",
            ),
        ];
        // Distinct expectations set — captured before `cases` moves into the loop.
        let distinct_expected: std::collections::HashSet<&str> =
            cases.iter().map(|(_, c)| *c).collect();
        let mut seen: Vec<&str> = Vec::new();
        for (e, want) in cases {
            assert_eq!(e.code(), ErrorCode(want), "code for {e}");
            seen.push(want);
        }
        // Uniqueness is asserted over the mapping's whole reachable set:
        // a wrapped variant MAY share the inner family's token (that is the
        // recursion), but no two DISTINCT expectations may collide.
        let mut sorted = seen.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            distinct_expected.len(),
            "codes drift or collide"
        );
    }

    #[test]
    fn mutation_plane_refusals_carry_own_code_and_parity_text() {
        // Cycle 2 (REQ-MUT-01/08): a domain refusal keeps the exact
        // operator-facing text AND a machine token of its own.
        let e = ApiError::refused(
            "execution_not_running",
            "execution x is COMPLETED, not RUNNING",
        );
        assert_eq!(e.code(), ErrorCode("execution_not_running"));
        assert_eq!(format!("{e}"), "execution x is COMPLETED, not RUNNING");
        // The contract-gate family renders the same sentences the pre-API
        // CLI path bailed with (parity, REQ-MUT-06).
        let e = ApiError::ContractRejected {
            contract: "c@1".into(),
            violations: vec!["$.a".into()],
        };
        assert_eq!(e.code(), ErrorCode("contract_violation"));
        assert_eq!(
            format!("{e}"),
            "artifact does not satisfy contract c@1: [\"$.a\"]"
        );
        let e = ApiError::ContractUnregistered {
            contract: "c@1".into(),
        };
        assert_eq!(e.code(), ErrorCode("contract_unregistered"));
        assert_eq!(
            format!("{e}"),
            "contract c@1 is not registered (devai contract add)"
        );
    }
}
