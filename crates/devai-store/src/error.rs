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

    /// REQ-SW-02: the single-writer guard fired — SQLite's partial unique
    /// index `role_executions_single_writer` rejected a second RUNNING
    /// execution for the same task.
    /// A dedicated variant, never an opaque `Sql`: the message names the
    /// task and the FULL id of the blocking RUNNING execution, because the
    /// raw unique violation does not (acceptance: «явная ошибка с id живого
    /// execution»). No silent queuing, no overwrite — the blocking row must
    /// be closed by a human path (`exec finish|fail|cancel`) first.
    #[error(
        "single-writer guard: task `{task_id}` already has a RUNNING execution `{existing_id}` \
         (started {started_at}, owner pid={owner_pid:?} host={owner_host:?}) — one writer per task, \
         no queueing and no overwrite; resolve it first: `devai exec finish|fail|cancel {existing_id}`"
    )]
    SingleWriterBusy { task_id: String, existing_id: String, started_at: String, owner_pid: Option<i64>, owner_host: Option<String> },

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
