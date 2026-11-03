use thiserror::Error;

#[derive(Debug, Error)]
pub enum StoreError {
    #[error(transparent)]
    Sql(#[from] rusqlite::Error),

    #[error(transparent)]
    Io(#[from] std::io::Error),

    #[error("{0} `{1}` not found")]
    NotFound(String, String),

    #[error("concurrent modification of {what}: expected version {expected}, found {found}")]
    Conflict { what: String, expected: u64, found: u64 },

    #[error("ambiguous identifier: {0}")]
    Ambiguous(String),

    /// REQ-07 AC2: a dependency reference is rejected AT WRITE — never
    /// stored and silently ignored at read. (Dangling endpoints surface as
    /// `NotFound`; this variant names the shape violations: self-dependency
    /// and a duplicate edge.)
    #[error("invalid task dependency: {0}")]
    InvalidDependency(String),

    #[error("database schema version {found}, but this build expects {expected}")]
    SchemaVersion { found: i64, expected: i64 },

    #[error("corrupt data: {0}")]
    Corrupt(String),

    #[error(
        "bundle import collides with the single-CURRENT invariant: task `{task_id}` kind `{kind}` \
         already has CURRENT artifact `{existing_id}` (seq {existing_seq}), the bundle carries \
         `{seq}` as CURRENT too — re-export the task from a migrated (v3) store instead of \
         importing a pre-v3 bundle whose multi-CURRENT defect would silently drop a row"
    )]
    SingleCurrentCollision { task_id: String, kind: String, seq: u32, existing_id: String, existing_seq: u32 },

    #[error("config error: {0}")]
    Config(String),
}

pub type StoreResult<T> = Result<T, StoreError>;
