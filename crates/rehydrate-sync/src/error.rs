use thiserror::Error;

#[derive(Debug, Error)]
pub enum SyncError {
    #[error("device: {0}")]
    Device(#[from] rehydrate_device::DeviceError),

    #[error("library: {0}")]
    Library(#[from] rehydrate_core::Error),

    #[error("cancelled")]
    Cancelled,

    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

pub type SyncResult<T> = std::result::Result<T, SyncError>;
