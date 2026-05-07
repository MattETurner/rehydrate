use thiserror::Error;

#[derive(Debug, Error)]
pub enum DeviceError {
    #[error("device unreachable: {0}")]
    Unreachable(String),

    #[error("authentication failed")]
    AuthFailed,

    #[error("io: {0}")]
    Io(#[from] std::io::Error),

    #[error("protocol: {0}")]
    Protocol(String),

    #[error("not found: {0}")]
    NotFound(String),

    #[error("other: {0}")]
    Other(String),
}

pub type DeviceResult<T> = std::result::Result<T, DeviceError>;
