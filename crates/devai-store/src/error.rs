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

    #[error("database schema version {found}, but this build expects {expected}")]
    SchemaVersion { found: i64, expected: i64 },

    #[error("corrupt data: {0}")]
    Corrupt(String),

    #[error("config error: {0}")]
    Config(String),
}

pub type StoreResult<T> = Result<T, StoreError>;
