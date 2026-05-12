//! rehydrate-core: content-addressed library for reHydrate.
//!
//! The library is a single self-contained directory containing a blob store
//! (sha256 fanout), manifests (canonical JSON, stored as blobs), and a SQLite
//! version log. See `docs/architecture.md` for the design rationale.

// Implementation modules are `pub(crate)` so external callers can
// only reach the types we deliberately re-export at the crate root.
// Locking the sub-modules off this way is what an external reviewer
// will look for first — "what's the public API surface, and is it
// minimal?" — and the answer should be: "exactly what's listed
// below."
pub(crate) mod blob;
pub(crate) mod db;
pub mod error;
pub(crate) mod hash;
pub mod manifest;
pub(crate) mod paths;

mod library;

pub use blob::PutOutcome;
pub use error::{CoreError, Result};
pub use hash::Sha256Hex;
pub use library::{
    ArchiveReason, ArchivedDocument, DocumentSummary, FolderEntry, GarbageCollectReport,
    ImportKind, Library, LibraryPathKind, ReconstructOptions, RecordOutcome, Source, VerifyReport,
    VersionEntry, VersionId,
};
pub use manifest::{Manifest, ManifestFile};
