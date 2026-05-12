use thiserror::Error;

/// The crate's error type. Named `CoreError` (rather than bare
/// `Error`) so the prefix matches the workspace convention used by
/// every other crate (`SyncError`, `DeviceError`, `OcrError`,
/// `PublishError`, `HttpError`, `ParseError`). Call sites that
/// import a list of error types from across the workspace can then
/// see at a glance which crate each came from.
#[derive(Debug, Error)]
pub enum CoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("sqlite: {0}")]
    Sqlite(#[from] rusqlite::Error),

    #[error("serde_json: {0}")]
    Json(#[from] serde_json::Error),

    #[error("library at {path} is corrupt: {reason}")]
    Corrupt { path: String, reason: String },

    #[error("manifest references unknown blob {0}")]
    MissingBlob(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("invalid library path: {0}")]
    InvalidPath(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    #[error("library at {0} is already open by another process")]
    AlreadyOpen(String),

    /// A reconstruction/export was asked to write to a path that
    /// already exists, with `allow_overwrite = false`. The library
    /// returns this rather than silently clobbering the file.
    #[error("destination already exists: {0}")]
    AlreadyExists(String),
}

pub type Result<T> = std::result::Result<T, CoreError>;
