use thiserror::Error;

#[derive(Debug, Error)]
pub enum Error {
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
}

pub type Result<T> = std::result::Result<T, Error>;
