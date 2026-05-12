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

    /// The remote server presented a host key that does not match the
    /// fingerprint pinned on the first successful connect. This means
    /// either (a) the device's host key was regenerated (firmware
    /// reflash, factory reset) or (b) something is impersonating
    /// `10.11.99.1`. We refuse the connection rather than silently
    /// accept the new key — the user has to confirm and forget the
    /// pinned fingerprint to proceed.
    #[error(
        "host key for {endpoint} has changed since the last successful connect — \
         expected {pinned_fingerprint}, got {presented_fingerprint}"
    )]
    HostKeyChanged {
        endpoint: String,
        pinned_fingerprint: String,
        presented_fingerprint: String,
    },

    /// An SFTP operation didn't complete within the per-operation
    /// budget. Surfaced as a distinct variant so the UI can offer
    /// "retry" rather than treat it as a fatal protocol error.
    #[error("operation timed out: {0}")]
    Timeout(String),
}

pub type DeviceResult<T> = std::result::Result<T, DeviceError>;
