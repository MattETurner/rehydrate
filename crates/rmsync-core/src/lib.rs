//! rmsync-core: content-addressed library for Marginalia.
//!
//! The library is a single self-contained directory containing a blob store
//! (sha256 fanout), manifests (canonical JSON, stored as blobs), and a SQLite
//! version log. See `remarkable-sync-implementation-plan.md` for the design.

pub mod blob;
pub mod db;
pub mod error;
pub mod hash;
pub mod manifest;
pub mod paths;

mod library;

pub use error::{Error, Result};
pub use hash::Sha256Hex;
pub use library::{
    DocumentSummary, FolderEntry, GarbageCollectReport, ImportKind, Library, RecordOutcome, Source,
    VerifyReport, VersionEntry, VersionId,
};
pub use manifest::{Manifest, ManifestFile};
