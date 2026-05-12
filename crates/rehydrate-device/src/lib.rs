//! Device transport for reHydrate.
//!
//! The `Device` trait is the single seam between the rest of the app and the
//! reMarkable tablet. Phase 1 needs only enumeration and download; later
//! phases will extend the trait with upload, delete, and move.

pub mod error;
pub mod model;
pub mod trait_def;

// `fake` is only used by the sync crate's tests against a
// directory-backed `Device` impl, plus a small internal smoke test.
// Gating it behind `cfg(any(test, feature = "fake"))` keeps the
// production build's public surface focused on the SSH path while
// still letting downstream tests reach for `FakeDevice` by enabling
// the feature.
#[cfg(any(test, feature = "fake"))]
pub mod fake;

#[cfg(feature = "ssh")]
pub mod known_hosts;
#[cfg(feature = "ssh")]
pub mod ssh;

pub use error::{DeviceError, DeviceResult};
pub use model::{DeviceInfo, RemoteEntry, RemoteEntryKind, RemoteFile};
pub use trait_def::Device;
