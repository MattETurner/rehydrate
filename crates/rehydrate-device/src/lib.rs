//! Device transport for reHydrate.
//!
//! The `Device` trait is the single seam between the rest of the app and the
//! reMarkable tablet. Phase 1 needs only enumeration and download; later
//! phases will extend the trait with upload, delete, and move.

pub mod error;
pub mod fake;
pub mod model;
pub mod trait_def;

#[cfg(feature = "ssh")]
pub mod ssh;

pub use error::{DeviceError, DeviceResult};
pub use model::{DeviceInfo, RemoteEntry, RemoteEntryKind, RemoteFile};
pub use trait_def::Device;
