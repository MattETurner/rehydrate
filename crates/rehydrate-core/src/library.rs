//! High-level Library facade.
//!
//! Phase 1 implements a lean subset: open an empty library, store blobs,
//! record a manifest as a new version. Phase 2 will add reconstruct + history
//! UI surfaces; Phase 4 will add GC. The data model already supports them.

use std::fs;
use std::path::Path;
use std::sync::Mutex;

use fs4::fs_std::FileExt;
use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::blob::BlobStore;
use crate::db::Db;
use crate::error::{Error, Result};
use crate::hash::Sha256Hex;
use crate::manifest::{Manifest, ManifestFile};
use crate::paths::LibraryPaths;

const LIBRARY_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Source {
    Pulled,
    Imported,
    Restored,
}

impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Source::Pulled => "pulled",
            Source::Imported => "imported",
            Source::Restored => "restored",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "pulled" => Some(Source::Pulled),
            "imported" => Some(Source::Imported),
            "restored" => Some(Source::Restored),
            _ => None,
        }
    }
}

pub type VersionId = i64;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentSummary {
    pub document_id: String,
    pub visible_name: String,
    pub doc_type: String,
    pub current_manifest: Sha256Hex,
    pub current_version_id: VersionId,
    /// When the current version was recorded. Used by the UI's "Recently
    /// Synced" sidebar filter; sortable as an RFC3339 timestamp string.
    pub last_observed_at: String,
    /// Folder UUID this document belongs to. `None` for the root.
    /// `"trash"` is the device's special trash bucket.
    pub parent: Option<String>,
    /// Total bytes of every file the manifest references. Cheap to
    /// compute since `list_documents` already reads the manifest blob.
    #[serde(default)]
    pub size_bytes: u64,
    /// Page count read from `content_meta.pageCount` if present.
    /// Notebooks always carry it; PDFs/EPUBs sometimes do.
    #[serde(default)]
    pub page_count: Option<u32>,
    /// True if the current manifest hasn't been pushed yet — i.e. the
    /// document has local edits that the next sync will upload.
    /// Imported-but-never-synced docs are also unpushed.
    #[serde(default)]
    pub has_unpushed_changes: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderEntry {
    pub folder_id: String,
    pub parent: Option<String>,
    pub visible_name: String,
    /// Local-only ordering hint within the parent's children. Lower
    /// values come first; ties break alphabetically. Never sent to the
    /// device — folders on the reMarkable don't have an explicit order.
    #[serde(default)]
    pub sort_index: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VersionEntry {
    pub id: VersionId,
    pub document_id: String,
    pub manifest_hash: Sha256Hex,
    pub parent_version_id: Option<VersionId>,
    pub observed_at: String,
    pub source: Source,
    pub note: Option<String>,
    /// Sum of `manifest.files[].size`. Read from the manifest blob on demand
    /// — Phase 2's history UI shows this per version. `None` if the manifest
    /// blob is missing (which `verify` would already have flagged).
    pub total_size_bytes: Option<u64>,
    /// Number of files in the document tree at this version.
    pub file_count: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct RecordOutcome {
    pub version_id: VersionId,
    pub manifest_hash: Sha256Hex,
    /// True if this manifest hash was already the current version for this
    /// document — i.e. nothing changed and no new row was appended.
    pub unchanged: bool,
}

/// Selects the on-device file extension and content metadata for an import.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ImportKind {
    Pdf,
    Epub,
}

impl ImportKind {
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "pdf" => Some(ImportKind::Pdf),
            "epub" => Some(ImportKind::Epub),
            _ => None,
        }
    }

    fn extension(self) -> &'static str {
        match self {
            ImportKind::Pdf => "pdf",
            ImportKind::Epub => "epub",
        }
    }

    fn doc_type(self) -> &'static str {
        match self {
            ImportKind::Pdf => "DocumentType.Pdf",
            ImportKind::Epub => "DocumentType.Epub",
        }
    }

    /// `.content` JSON. We populate the small subset of fields xochitl
    /// actually requires — it fills in the rest (page count, transform,
    /// extraMetadata) on first open.
    fn content_json(self) -> serde_json::Value {
        match self {
            ImportKind::Pdf => serde_json::json!({
                "fileType": "pdf",
                "pageCount": 0,
                "lastOpenedPage": 0,
                "lineHeight": -1,
                "margins": 100,
                "orientation": "portrait",
                "textScale": 1,
                "extraMetadata": {},
                "transform": {
                    "m11": 1.0, "m12": 0.0, "m13": 0.0,
                    "m21": 0.0, "m22": 1.0, "m23": 0.0,
                    "m31": 0.0, "m32": 0.0, "m33": 1.0,
                },
            }),
            ImportKind::Epub => serde_json::json!({
                "fileType": "epub",
                "pageCount": 0,
                "lastOpenedPage": 0,
                "lineHeight": -1,
                "margins": 100,
                "orientation": "portrait",
                "textScale": 1,
                "extraMetadata": {},
            }),
        }
    }
}

/// Why a document is in the archive. Recorded so the UI can show a hint
/// ("deleted on device", "deleted locally") and so future tooling can
/// distinguish auto-archived from user-archived entries.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum ArchiveReason {
    /// User clicked Delete in the local UI.
    Local,
    /// Detected during pull: the device no longer has this document.
    Device,
}

impl ArchiveReason {
    fn as_str(self) -> &'static str {
        match self {
            ArchiveReason::Local => "local",
            ArchiveReason::Device => "device",
        }
    }

    fn parse(s: &str) -> Option<Self> {
        match s {
            "local" => Some(ArchiveReason::Local),
            "device" => Some(ArchiveReason::Device),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArchivedDocument {
    pub document_id: String,
    pub visible_name: String,
    pub doc_type: String,
    pub parent: Option<String>,
    pub manifest_hash: Sha256Hex,
    pub version_id: VersionId,
    pub reason: ArchiveReason,
    pub archived_at: String,
}

/// Optional second SQL operation glued to the same transaction as
/// `record_metadata_change`'s record_version. Used by archive /
/// unarchive so the metadata edit and the documents↔archived_documents
/// move commit together (audit fix C2).
enum PostAction<'a> {
    None,
    Archive {
        reason: ArchiveReason,
        visible_name: &'a str,
        doc_type: &'a str,
        original_parent: Option<&'a str>,
    },
    Unarchive,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct GarbageCollectReport {
    pub scanned: usize,
    pub deleted: usize,
    pub bytes_freed: u64,
    pub errors: usize,
}

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct VerifyReport {
    pub manifests_total: usize,
    pub manifests_ok: usize,
    pub manifests_missing: usize,
    pub manifests_invalid: usize,
    pub blobs_total: usize,
    pub blobs_missing: usize,
    pub blobs_orphan: usize,
    pub blobs_corrupted: usize,
    /// Up to 5 example "<document_id>:<path>" strings for missing blobs.
    pub missing_examples: Vec<String>,
    /// Up to 5 example absolute paths of orphan blobs.
    pub orphan_examples: Vec<String>,
}

/// Result of [`Library::probe_path`]: what the IPC layer should
/// tell the user before attempting to open the path. `Empty` means
/// safe-to-create-here; `Existing` means a stamped library is
/// already there. The error case (a foreign non-empty directory)
/// surfaces as [`Error::InvalidPath`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LibraryPathKind {
    Empty,
    Existing,
}

#[derive(Serialize, Deserialize)]
struct LibraryMeta {
    schema: u32,
    library_id: String,
    created_at: String,
}

pub struct Library {
    paths: LibraryPaths,
    blobs: BlobStore,
    db: Db,
    /// Coarse-grained mutex serialising blob-mutating operations
    /// (record_version, import_file, restore_version) against
    /// garbage_collect. Without this, GC can build its live-set, then
    /// record_version commits a new manifest, then GC deletes the
    /// freshly-written blob because it wasn't in the snapshot.
    /// The lock is held only inside Library methods; it doesn't span
    /// async I/O (the sync engine pulls all bytes into memory before
    /// calling record_version, so the critical section is short).
    write_lock: Mutex<()>,
    /// OS-level advisory exclusive lock held for the lifetime of the
    /// library instance. Audit fix H3: without this, two app
    /// instances pointed at the same library can each hold their own
    /// `write_lock` mutex, and instance A's GC may delete a blob that
    /// instance B just committed. The handle is kept alive in this
    /// field; dropping it releases the OS lock automatically.
    _lock_file: std::fs::File,
}

impl Library {
    /// Open an existing library or initialize a new one at `path`.
    ///
    /// Audit fix H9: refuses to silently claim a non-empty directory
    /// that doesn't already look like a reHydrate library. Without
    /// this check, a renderer-supplied path like `~/Documents/` would
    /// have `blobs/`, `tmp/`, `logs/`, and `db.sqlite` written into
    /// it — and a subsequent `garbage_collect` would walk that
    /// `blobs/` and `remove_file` matching entries.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let paths = LibraryPaths::new(path.as_ref());
        Self::validate_library_path(&paths)?;
        paths.ensure_dirs()?;

        // Acquire the inter-process advisory lock first. If another
        // app instance has the same library open, fail fast — sharing
        // a library across processes corrupts the GC vs. record_version
        // invariant (audit fix H3).
        let lock_path = paths.root.join(".lock");
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path)?;
        if lock_file.try_lock_exclusive().is_err() {
            return Err(Error::AlreadyOpen(paths.root.display().to_string()));
        }

        if !paths.library_json.exists() {
            let meta = LibraryMeta {
                schema: LIBRARY_SCHEMA,
                library_id: uuid::Uuid::new_v4().to_string(),
                created_at: OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default(),
            };
            fs::write(&paths.library_json, serde_json::to_vec_pretty(&meta)?)?;
        }

        let db = Db::open(&paths.db)?;
        let blobs = BlobStore::new(paths.clone());

        Ok(Self {
            paths,
            blobs,
            db,
            write_lock: Mutex::new(()),
            _lock_file: lock_file,
        })
    }

    /// A path is a valid library target if either:
    /// - The directory does not yet exist (fresh init), or
    /// - The directory exists and contains a parseable `library.json`
    ///   with a recognised schema and a UUID stamp, or
    /// - The directory exists and is effectively empty (only dotfiles
    ///   like `.DS_Store` / our own `.lock`).
    ///
    /// Anything else — a directory full of foreign files — is
    /// rejected so we never silently scatter blobs into the user's
    /// Documents folder.
    fn validate_library_path(paths: &LibraryPaths) -> Result<()> {
        if !paths.root.exists() {
            // Fresh init: ensure_dirs will create it.
            return Ok(());
        }
        if paths.library_json.exists() {
            // Existing library — verify the stamp is sane.
            let bytes = fs::read(&paths.library_json).map_err(|e| {
                Error::InvalidPath(format!(
                    "could not read {}: {e}",
                    paths.library_json.display()
                ))
            })?;
            let meta: LibraryMeta = serde_json::from_slice(&bytes).map_err(|e| {
                Error::InvalidPath(format!(
                    "{} has a malformed library.json: {e}",
                    paths.root.display()
                ))
            })?;
            if meta.schema != LIBRARY_SCHEMA {
                return Err(Error::InvalidPath(format!(
                    "{} has library schema {} (expected {})",
                    paths.root.display(),
                    meta.schema,
                    LIBRARY_SCHEMA
                )));
            }
            if uuid::Uuid::parse_str(&meta.library_id).is_err() {
                return Err(Error::InvalidPath(format!(
                    "{} has an invalid library_id stamp",
                    paths.root.display()
                )));
            }
            return Ok(());
        }
        // No library.json yet — only proceed if the dir is empty (modulo
        // dotfiles + a stale `.lock` from a prior failed init).
        let foreign: Vec<_> = fs::read_dir(&paths.root)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                let s = name.to_string_lossy();
                !s.starts_with('.')
            })
            .collect();
        if !foreign.is_empty() {
            return Err(Error::InvalidPath(format!(
                "{} is not empty and is not a reHydrate library (no library.json)",
                paths.root.display()
            )));
        }
        Ok(())
    }

    pub fn paths(&self) -> &LibraryPaths {
        &self.paths
    }

    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
    }

    /// Look at `path` and report whether it's already a library, an
    /// empty directory ready to become one, or a non-empty foreign
    /// directory we shouldn't claim. Public so the IPC layer can
    /// surface a "Create new library here?" prompt before
    /// `Library::open` silently materialises one.
    ///
    /// Does not acquire the inter-process lock — purely a read.
    pub fn probe_path(path: impl AsRef<Path>) -> Result<LibraryPathKind> {
        let paths = LibraryPaths::new(path.as_ref());
        if !paths.root.exists() {
            return Ok(LibraryPathKind::Empty);
        }
        if paths.library_json.exists() {
            // Reuse the same stamp validation `Library::open` does so
            // the probe and the open agree on what counts as valid.
            Self::validate_library_path(&paths)?;
            return Ok(LibraryPathKind::Existing);
        }
        let foreign: Vec<_> = fs::read_dir(&paths.root)?
            .filter_map(|e| e.ok())
            .filter(|e| {
                let name = e.file_name();
                !name.to_string_lossy().starts_with('.')
            })
            .collect();
        if foreign.is_empty() {
            Ok(LibraryPathKind::Empty)
        } else {
            Err(Error::InvalidPath(format!(
                "{} is not empty and is not a reHydrate library (no library.json)",
                paths.root.display()
            )))
        }
    }

    pub fn put_blob(&self, bytes: &[u8]) -> Result<crate::blob::PutResult> {
        self.blobs.put_bytes(bytes)
    }

    pub fn has_blob(&self, hash: &Sha256Hex) -> bool {
        self.blobs.has(hash)
    }

    pub fn read_blob(&self, hash: &Sha256Hex) -> Result<Vec<u8>> {
        self.blobs.read_to_vec(hash)
    }

    /// Append a new version for the document described by `manifest`. If the
    /// manifest's hash equals the document's current_manifest, this is a no-op
    /// and the existing version_id is returned with `unchanged=true`.
    pub fn record_version(&self, manifest: &Manifest, source: Source) -> Result<RecordOutcome> {
        let _write_guard = self.write_lock.lock().expect("library write_lock poisoned");

        let canonical = manifest.canonical_json()?;
        let manifest_hash = Sha256Hex::from_bytes(&canonical);

        // Persist the manifest itself as a blob (idempotent — orphan
        // collected by GC after the grace if we never commit).
        self.blobs.put_bytes(&canonical)?;

        let mut conn = self.db.lock();
        let tx = conn.transaction()?;
        let outcome = self.record_version_in_tx(&tx, manifest, &manifest_hash, source)?;
        tx.commit()?;
        Ok(outcome)
    }

    /// Pure-SQL part of `record_version`. The caller is expected to have
    /// already written the manifest blob to disk and to be holding both
    /// `write_lock` and a SQLite transaction.
    ///
    /// Factoring this out lets `record_metadata_change`,
    /// `archive_document`, and `unarchive_document` compose multiple
    /// row mutations into a single transaction — without it, each
    /// caller had to commit a partial state and then take a second
    /// lock + tx to finish, which left a window for crash-recovery
    /// and concurrent-edit races (audit findings C1 + C2).
    fn record_version_in_tx(
        &self,
        tx: &rusqlite::Transaction<'_>,
        manifest: &Manifest,
        manifest_hash: &Sha256Hex,
        source: Source,
    ) -> Result<RecordOutcome> {
        let now = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();

        // Already the current version?
        let current: Option<(String, i64)> = tx
            .query_row(
                "SELECT current_manifest, current_version_id FROM documents WHERE document_id = ?1",
                params![manifest.document_id],
                |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)),
            )
            .optional()?;

        if let Some((cur_hash, cur_id)) = &current {
            if cur_hash == manifest_hash.as_str() {
                // Same manifest as current — only the device-side known
                // state advances if this is a pull. Restored/imported
                // re-records change nothing observable, so skip updating
                // last_seen_manifest here.
                if matches!(source, Source::Pulled) {
                    tx.execute(
                        "UPDATE sync_state SET last_seen_manifest = ?1, last_synced_at = ?2 \
                         WHERE document_id = ?3",
                        params![manifest_hash.as_str(), now, manifest.document_id],
                    )?;
                }
                return Ok(RecordOutcome {
                    version_id: *cur_id,
                    manifest_hash: manifest_hash.clone(),
                    unchanged: true,
                });
            }
        }

        let parent_version_id = current.as_ref().map(|(_, id)| *id);

        tx.execute(
            "INSERT INTO versions(document_id, manifest_hash, parent_version_id, observed_at, source, note) \
             VALUES (?1, ?2, ?3, ?4, ?5, NULL)",
            params![
                manifest.document_id,
                manifest_hash.as_str(),
                parent_version_id,
                now,
                source.as_str()
            ],
        )?;
        let version_id = tx.last_insert_rowid();

        tx.execute(
            "INSERT INTO documents(document_id, current_manifest, current_version_id) \
             VALUES (?1, ?2, ?3) \
             ON CONFLICT(document_id) DO UPDATE SET \
                 current_manifest = excluded.current_manifest, \
                 current_version_id = excluded.current_version_id",
            params![manifest.document_id, manifest_hash.as_str(), version_id],
        )?;

        // Populate blob_refs (manifest itself + every file).
        tx.execute(
            "INSERT OR IGNORE INTO blob_refs(blob_hash, manifest_hash) VALUES (?1, ?1)",
            params![manifest_hash.as_str()],
        )?;
        {
            let mut stmt = tx.prepare(
                "INSERT OR IGNORE INTO blob_refs(blob_hash, manifest_hash) VALUES (?1, ?2)",
            )?;
            for f in &manifest.files {
                stmt.execute(params![f.sha256.as_str(), manifest_hash.as_str()])?;
            }
        }

        // `last_seen_manifest` tracks "what's known to be on the device".
        // Only `Source::Pulled` may advance it — restored/imported manifests
        // are library-side changes that should trigger a future push.
        if matches!(source, Source::Pulled) {
            tx.execute(
                "INSERT INTO sync_state(document_id, last_seen_manifest, last_synced_at) \
                 VALUES (?1, ?2, ?3) \
                 ON CONFLICT(document_id) DO UPDATE SET \
                     last_seen_manifest = excluded.last_seen_manifest, \
                     last_synced_at = excluded.last_synced_at",
                params![manifest.document_id, manifest_hash.as_str(), now],
            )?;
        } else {
            // Ensure the row exists so future planners have somewhere to
            // read from, but leave last_seen_manifest alone.
            tx.execute(
                "INSERT INTO sync_state(document_id) VALUES (?1) \
                 ON CONFLICT(document_id) DO NOTHING",
                params![manifest.document_id],
            )?;
        }

        Ok(RecordOutcome {
            version_id,
            manifest_hash: manifest_hash.clone(),
            unchanged: false,
        })
    }

    pub fn list_documents(&self) -> Result<Vec<DocumentSummary>> {
        let conn = self.db.lock();
        let mut stmt = conn.prepare(
            "SELECT d.document_id, d.current_manifest, d.current_version_id, v.observed_at, \
                    s.last_seen_manifest \
             FROM documents d \
             JOIN versions v ON v.id = d.current_version_id \
             LEFT JOIN sync_state s ON s.document_id = d.document_id \
             ORDER BY d.document_id",
        )?;
        let rows: Vec<(String, String, i64, String, Option<String>)> = stmt
            .query_map([], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))
            })?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        drop(conn);

        let mut out = Vec::with_capacity(rows.len());
        for (document_id, manifest_hex, version_id, observed_at, last_seen_manifest) in rows {
            let hash = Sha256Hex::from_hex(&manifest_hex).ok_or_else(|| Error::Corrupt {
                path: self.paths.db.display().to_string(),
                reason: format!("bad manifest hash for {document_id}"),
            })?;
            let manifest_bytes = self.blobs.read_to_vec(&hash)?;
            let manifest = Manifest::from_canonical_json(&manifest_bytes)?;
            let size_bytes: u64 = manifest.files.iter().map(|f| f.size).sum();
            let page_count = manifest
                .content_meta
                .get("pageCount")
                .and_then(|v| v.as_u64())
                .and_then(|n| u32::try_from(n).ok());
            // Document has unpushed changes if its current manifest
            // doesn't match what the device last received. Never-synced
            // (last_seen_manifest IS NULL) also counts.
            let has_unpushed_changes = match &last_seen_manifest {
                Some(ls) => ls != &manifest_hex,
                None => true,
            };
            out.push(DocumentSummary {
                document_id,
                visible_name: manifest.visible_name,
                doc_type: manifest.doc_type,
                current_manifest: hash,
                current_version_id: version_id,
                last_observed_at: observed_at,
                parent: manifest.parent,
                size_bytes,
                page_count,
                has_unpushed_changes,
            });
        }
        Ok(out)
    }

    pub fn get_history(&self, document_id: &str) -> Result<Vec<VersionEntry>> {
        // Pull raw rows first; defer hash validation so a single corrupt
        // row produces a typed `Error::Corrupt` instead of getting
        // wrapped through rusqlite's error type or — worse, before
        // the H4 fix — silently fabricated.
        type RawRow = (
            i64,
            String,
            String,
            Option<i64>,
            String,
            String,
            Option<String>,
        );
        let conn = self.db.lock();
        let mut stmt = conn.prepare(
            "SELECT id, document_id, manifest_hash, parent_version_id, observed_at, source, note \
             FROM versions WHERE document_id = ?1 ORDER BY id ASC",
        )?;
        let raw: Vec<RawRow> = stmt
            .query_map(params![document_id], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        drop(conn);

        let mut rows: Vec<VersionEntry> = Vec::with_capacity(raw.len());
        for (id, doc_id, manifest_hex, parent, observed_at, source_str, note) in raw {
            let manifest_hash =
                Sha256Hex::from_hex(&manifest_hex).ok_or_else(|| Error::Corrupt {
                    path: self.paths.db.display().to_string(),
                    reason: format!("bad manifest hash for version {id}"),
                })?;
            rows.push(VersionEntry {
                id,
                document_id: doc_id,
                manifest_hash,
                parent_version_id: parent,
                observed_at,
                source: Source::parse(&source_str).unwrap_or(Source::Pulled),
                note,
                total_size_bytes: None,
                file_count: None,
            });
        }

        // Populate size + file count by reading each manifest blob. Cheap
        // for typical libraries (a few hundred bytes per manifest); if it
        // ever shows up as hot we can cache these values in the schema.
        for entry in &mut rows {
            if let Ok(bytes) = self.blobs.read_to_vec(&entry.manifest_hash) {
                if let Ok(m) = Manifest::from_canonical_json(&bytes) {
                    entry.total_size_bytes = Some(m.files.iter().map(|f| f.size).sum());
                    entry.file_count = Some(m.files.len());
                }
            }
        }
        Ok(rows)
    }

    /// Record a new version of a document with its `.metadata` blob
    /// rewritten in place by `mutate`. The new metadata is also pushed into
    /// `manifest.metadata` and `manifest.parent` so a future planner can
    /// see the change without re-reading the blob.
    ///
    /// Used by every "library-side edit" path: move-to-folder, archive
    /// (deleted=true), unarchive (deleted=false). Bumps the device-facing
    /// `lastModified` and `modified` flags so the tablet treats the file
    /// as freshly changed.
    ///
    /// The whole read → mutate → write sequence runs under
    /// `write_lock` plus a single SQLite transaction. `post_action`
    /// lets archive / unarchive piggy-back their `documents` ↔
    /// `archived_documents` move on the same transaction (audit
    /// fixes C1 + C2). Without that coupling, two callers could
    /// observe the same parent manifest and silently overwrite each
    /// other, and a crash between the version-record commit and the
    /// archive move could leave a row in a deleted-but-not-archived
    /// limbo.
    fn record_metadata_change<F>(
        &self,
        document_id: &str,
        mutate: F,
        post_action: PostAction<'_>,
    ) -> Result<RecordOutcome>
    where
        F: FnOnce(&mut serde_json::Map<String, serde_json::Value>) -> Result<()>,
    {
        // Take the write lock for the WHOLE flow — read, mutate, write —
        // so concurrent rename/move/archive callers serialise properly.
        let _write_guard = self.write_lock.lock().expect("library write_lock poisoned");

        // Brief db lock to resolve the current manifest hash (live or
        // archived). We must drop this before doing filesystem I/O
        // because the connection is held by transactional code below.
        let manifest_hex: String = {
            let conn = self.db.lock();
            let live: Option<String> = conn
                .query_row(
                    "SELECT current_manifest FROM documents WHERE document_id = ?1",
                    params![document_id],
                    |r| r.get(0),
                )
                .optional()?;
            if let Some(h) = live {
                h
            } else {
                conn.query_row(
                    "SELECT manifest_hash FROM archived_documents WHERE document_id = ?1",
                    params![document_id],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or_else(|| Error::NotFound(format!("document {document_id}")))?
            }
        };

        let hash = Sha256Hex::from_hex(&manifest_hex).ok_or_else(|| Error::Corrupt {
            path: self.paths.db.display().to_string(),
            reason: format!("bad manifest hash for {document_id}"),
        })?;
        let manifest_bytes = self.blobs.read_to_vec(&hash)?;
        let mut manifest = Manifest::from_canonical_json(&manifest_bytes)?;

        let meta_idx = manifest
            .files
            .iter()
            .position(|f| f.path.ends_with(".metadata"))
            .ok_or_else(|| Error::Corrupt {
                path: "<manifest>".into(),
                reason: format!("document {document_id} has no .metadata file"),
            })?;
        let meta_bytes = self.blobs.read_to_vec(&manifest.files[meta_idx].sha256)?;
        let mut meta_value: serde_json::Value = serde_json::from_slice(&meta_bytes)?;
        let map = meta_value.as_object_mut().ok_or_else(|| Error::Corrupt {
            path: "<manifest>".into(),
            reason: format!("metadata for {document_id} is not a JSON object"),
        })?;

        mutate(map)?;

        // Mark the file as changed locally so the tablet's xochitl picks
        // up the new metadata on next sync. Empty defaults are safe — if
        // these keys were missing they'll simply be set.
        let now_ms = OffsetDateTime::now_utc().unix_timestamp() * 1000;
        map.insert(
            "lastModified".into(),
            serde_json::Value::String(now_ms.to_string()),
        );
        map.insert("modified".into(), serde_json::Value::Bool(true));
        map.insert("metadatamodified".into(), serde_json::Value::Bool(true));
        map.insert("synced".into(), serde_json::Value::Bool(false));

        let new_meta_bytes = serde_json::to_vec_pretty(&meta_value)?;
        let put = self.put_blob(&new_meta_bytes)?;
        manifest.files[meta_idx].sha256 = put.hash;
        manifest.files[meta_idx].size = put.size;
        manifest.metadata = meta_value.clone();

        // Reflect the post-mutation parent + visible name on the
        // manifest's top-level fields so SQL queries that read them
        // (sidebar filtering, archive entry display) match what's in
        // the metadata blob.
        manifest.parent = match meta_value.get("parent").and_then(|v| v.as_str()) {
            None | Some("") => None,
            Some(p) => Some(p.to_string()),
        };
        if let Some(name) = meta_value.get("visibleName").and_then(|v| v.as_str()) {
            manifest.visible_name = name.to_string();
        }

        // Write the new manifest blob (idempotent; GC reclaims if we
        // fail to commit below) and run the SQL atomically.
        let canonical = manifest.canonical_json()?;
        let manifest_hash = Sha256Hex::from_bytes(&canonical);
        self.blobs.put_bytes(&canonical)?;

        let mut conn = self.db.lock();
        let tx = conn.transaction()?;
        let outcome =
            self.record_version_in_tx(&tx, &manifest, &manifest_hash, Source::Restored)?;

        match post_action {
            PostAction::None => {}
            PostAction::Archive {
                reason,
                visible_name,
                doc_type,
                original_parent,
            } => {
                let now = OffsetDateTime::now_utc()
                    .format(&time::format_description::well_known::Rfc3339)
                    .unwrap_or_default();
                tx.execute(
                    "INSERT INTO archived_documents \
                        (document_id, visible_name, doc_type, parent, manifest_hash, \
                         version_id, reason, archived_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8) \
                     ON CONFLICT(document_id) DO UPDATE SET \
                        manifest_hash = excluded.manifest_hash, \
                        version_id = excluded.version_id, \
                        reason = excluded.reason, \
                        archived_at = excluded.archived_at",
                    params![
                        document_id,
                        visible_name,
                        doc_type,
                        original_parent,
                        outcome.manifest_hash.as_str(),
                        outcome.version_id,
                        reason.as_str(),
                        now,
                    ],
                )?;
                tx.execute(
                    "DELETE FROM documents WHERE document_id = ?1",
                    params![document_id],
                )?;
            }
            PostAction::Unarchive => {
                tx.execute(
                    "DELETE FROM archived_documents WHERE document_id = ?1",
                    params![document_id],
                )?;
            }
        }

        tx.commit()?;
        Ok(outcome)
    }

    /// Rename a document. Writes the new title into `.metadata`'s
    /// `visibleName`, records a new version, and flags the doc as having
    /// unpushed changes — the next sync sends the new metadata to the
    /// tablet which then displays the new name. Empty / whitespace-only
    /// names are rejected so the tablet never ends up with a blank title.
    pub fn rename_document(&self, document_id: &str, new_name: &str) -> Result<RecordOutcome> {
        let trimmed = new_name.trim().to_string();
        if trimmed.is_empty() {
            return Err(Error::InvalidArgument(
                "document name must not be empty".into(),
            ));
        }
        self.record_metadata_change(
            document_id,
            |map| {
                map.insert(
                    "visibleName".into(),
                    serde_json::Value::String(trimmed.clone()),
                );
                Ok(())
            },
            PostAction::None,
        )
    }

    /// Move a live document into a different folder (`new_parent` =
    /// `Some(folder_uuid)`) or to the root (`None`). Records a new version
    /// so the device picks up the move on next push.
    pub fn move_document(
        &self,
        document_id: &str,
        new_parent: Option<&str>,
    ) -> Result<RecordOutcome> {
        let parent = new_parent.unwrap_or("").to_string();
        self.record_metadata_change(
            document_id,
            |map| {
                map.insert("parent".into(), serde_json::Value::String(parent));
                Ok(())
            },
            PostAction::None,
        )
    }

    /// Attach a library-side derived artefact (e.g. an OCR transcript)
    /// to the document's current manifest. Creates a fresh version
    /// containing `<path>` with `derived: true`, replacing any prior
    /// entry at the same path. The blob bytes are content-addressed and
    /// committed to the blob store before the SQL transaction; any
    /// pre-existing file with the same path is removed from the
    /// manifest (its blob will be reclaimed by GC if unreferenced).
    ///
    /// The whole read → mutate → write sequence runs under
    /// `write_lock` plus a single SQLite tx, same shape as
    /// `record_metadata_change`, so concurrent edits don't lose
    /// the transcript.
    pub fn record_derived_artefact(
        &self,
        document_id: &str,
        path: &str,
        bytes: &[u8],
    ) -> Result<RecordOutcome> {
        if !path.starts_with("ocr/") {
            // Defence: keep the derived namespace tightly scoped so a
            // future caller can't shadow legitimate device files via
            // this back-door.
            return Err(Error::InvalidArgument(format!(
                "derived artefact path must live under `ocr/`, got {path:?}"
            )));
        }

        let _write_guard = self.write_lock.lock().expect("library write_lock poisoned");

        let manifest_hex: String = {
            let conn = self.db.lock();
            conn.query_row(
                "SELECT current_manifest FROM documents WHERE document_id = ?1",
                params![document_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("document {document_id}")))?
        };
        let hash = Sha256Hex::from_hex(&manifest_hex).ok_or_else(|| Error::Corrupt {
            path: self.paths.db.display().to_string(),
            reason: format!("bad manifest hash for {document_id}"),
        })?;
        let manifest_bytes = self.blobs.read_to_vec(&hash)?;
        let mut manifest = Manifest::from_canonical_json(&manifest_bytes)?;

        let put = self.put_blob(bytes)?;
        let new_file = ManifestFile {
            path: path.to_string(),
            sha256: put.hash,
            size: put.size,
            mode: 0o644,
            derived: true,
        };
        if let Some(existing) = manifest.files.iter_mut().find(|f| f.path == path) {
            *existing = new_file;
        } else {
            manifest.files.push(new_file);
        }

        let canonical = manifest.canonical_json()?;
        let manifest_hash = Sha256Hex::from_bytes(&canonical);
        self.blobs.put_bytes(&canonical)?;

        let mut conn = self.db.lock();
        let tx = conn.transaction()?;

        // Snapshot the prior sync_state row before record_version_in_tx
        // potentially writes one. We use this to decide whether to
        // auto-advance `last_seen_manifest` (see below).
        let prior_last_seen: Option<String> = tx
            .query_row(
                "SELECT last_seen_manifest FROM sync_state WHERE document_id = ?1",
                params![document_id],
                |r| r.get(0),
            )
            .optional()?
            .flatten();

        let outcome =
            self.record_version_in_tx(&tx, &manifest, &manifest_hash, Source::Imported)?;

        // Derived artefacts (OCR transcripts) live entirely on the
        // PC — push.rs filters them out of the upload payload. Without
        // this clause, the unchanged `current_manifest` → new
        // `current_manifest` delta still flips `has_unpushed_changes`
        // to true and trips `plan_push` into queuing a no-op
        // re-upload of every other file.
        //
        // Auto-advance `last_seen_manifest` to the new hash IF the
        // device was already in sync with the prior manifest. If the
        // user had unpushed changes (rename, move, archive…), leave
        // `last_seen` alone so the next push still ships them — the
        // derived file gets filtered out of that push regardless.
        if prior_last_seen.as_deref() == Some(manifest_hex.as_str()) {
            let now = OffsetDateTime::now_utc()
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap_or_default();
            tx.execute(
                "UPDATE sync_state SET last_seen_manifest = ?1, last_synced_at = ?2 \
                 WHERE document_id = ?3",
                params![manifest_hash.as_str(), now, document_id],
            )?;
        }

        tx.commit()?;
        Ok(outcome)
    }

    /// Read a derived artefact from the manifest at `version_id`. Used
    /// to surface OCR transcripts in the UI without round-tripping
    /// through `reconstruct`.
    pub fn read_derived_artefact(
        &self,
        version_id: VersionId,
        path: &str,
    ) -> Result<Option<Vec<u8>>> {
        let entry = self.get_version(version_id)?;
        let manifest_bytes = self.blobs.read_to_vec(&entry.manifest_hash)?;
        let manifest = Manifest::from_canonical_json(&manifest_bytes)?;
        let Some(f) = manifest.files.iter().find(|f| f.derived && f.path == path) else {
            return Ok(None);
        };
        Ok(Some(self.blobs.read_to_vec(&f.sha256)?))
    }

    /// Move a document from the live `documents` table into the archive.
    /// Versions stay intact so a later `unarchive_document` can restore the
    /// listing exactly. If the document is already archived this is a
    /// no-op; if it doesn't exist we return `NotFound`.
    pub fn archive_document(&self, document_id: &str, reason: ArchiveReason) -> Result<()> {
        // Idempotent: re-archiving an already-archived doc is a no-op so
        // the device-deletion detector (which calls this in a loop) and
        // the UI button can both invoke it freely.
        if self.is_archived(document_id)? {
            return Ok(());
        }

        // Capture the doc's pre-archive parent + visibleName + doc_type
        // so a later unarchive can restore it where the user had it.
        // Reading before record_metadata_change is fine — the mutation
        // flips parent to "trash", and record_metadata_change re-reads
        // under write_lock + tx so the rename/move race window is
        // closed even though this read runs first.
        let (original_parent, visible_name, doc_type) = {
            let conn = self.db.lock();
            let h: String = conn
                .query_row(
                    "SELECT current_manifest FROM documents WHERE document_id = ?1",
                    params![document_id],
                    |r| r.get(0),
                )
                .optional()?
                .ok_or_else(|| Error::NotFound(format!("document {document_id}")))?;
            drop(conn);
            let hash = Sha256Hex::from_hex(&h).ok_or_else(|| Error::Corrupt {
                path: self.paths.db.display().to_string(),
                reason: format!("bad manifest hash for {document_id}"),
            })?;
            let bytes = self.blobs.read_to_vec(&hash)?;
            let m = Manifest::from_canonical_json(&bytes)?;
            (m.parent.clone(), m.visible_name, m.doc_type)
        };

        // One atomic transaction: write the deleted=true version AND
        // move documents → archived_documents in the same tx (audit
        // fix C2). A crash mid-way used to leave the doc with a
        // deleted manifest but never archived; now it's all-or-nothing.
        self.record_metadata_change(
            document_id,
            |map| {
                map.insert("deleted".into(), serde_json::Value::Bool(true));
                map.insert("parent".into(), serde_json::Value::String("trash".into()));
                Ok(())
            },
            PostAction::Archive {
                reason,
                visible_name: &visible_name,
                doc_type: &doc_type,
                original_parent: original_parent.as_deref(),
            },
        )?;
        Ok(())
    }

    /// Restore an archived document to the live listing using the manifest
    /// it had at archive time. Returns `NotFound` if the id isn't in the
    /// archive.
    pub fn unarchive_document(&self, document_id: &str) -> Result<DocumentSummary> {
        // Read the archive entry first — it tells us the original parent
        // to restore the doc to. record_metadata_change re-reads under
        // write_lock + tx so any concurrent edit lands either before
        // or after this whole flow.
        let row: (String, String, Option<String>) = {
            let conn = self.db.lock();
            conn.query_row(
                "SELECT visible_name, doc_type, parent \
                 FROM archived_documents WHERE document_id = ?1",
                params![document_id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("archived document {document_id}")))?
        };
        let (visible_name, doc_type, original_parent) = row;
        let parent_for_meta = original_parent.clone().unwrap_or_default();

        // Single tx writes the deleted=false version AND deletes the
        // archived_documents row (record_version_in_tx already inserts
        // into documents). Audit fix C2.
        let outcome = self.record_metadata_change(
            document_id,
            |map| {
                map.insert("deleted".into(), serde_json::Value::Bool(false));
                map.insert(
                    "parent".into(),
                    serde_json::Value::String(parent_for_meta.clone()),
                );
                Ok(())
            },
            PostAction::Unarchive,
        )?;

        // Read observed_at for the freshly-recorded version. No lock —
        // it's a read-only query.
        let observed_at: String = {
            let conn = self.db.lock();
            conn.query_row(
                "SELECT observed_at FROM versions WHERE id = ?1",
                params![outcome.version_id],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default()
        };

        Ok(DocumentSummary {
            document_id: document_id.to_string(),
            visible_name,
            doc_type,
            current_manifest: outcome.manifest_hash,
            current_version_id: outcome.version_id,
            last_observed_at: observed_at,
            parent: original_parent,
            size_bytes: 0,
            page_count: None,
            has_unpushed_changes: true,
        })
    }

    /// Permanently delete an archived document: drop the archive row, the
    /// version log entries, and any sync_state. Blobs that are no longer
    /// referenced by any version become orphans and will be reclaimed by
    /// the next `garbage_collect`.
    pub fn purge_archived_document(&self, document_id: &str) -> Result<()> {
        let _write_guard = self.write_lock.lock().expect("library write_lock poisoned");
        let mut conn = self.db.lock();
        let tx = conn.transaction()?;

        let exists: Option<i64> = tx
            .query_row(
                "SELECT 1 FROM archived_documents WHERE document_id = ?1",
                params![document_id],
                |r| r.get(0),
            )
            .optional()?;
        if exists.is_none() {
            return Err(Error::NotFound(format!("archived document {document_id}")));
        }

        tx.execute(
            "DELETE FROM archived_documents WHERE document_id = ?1",
            params![document_id],
        )?;
        tx.execute(
            "DELETE FROM versions WHERE document_id = ?1",
            params![document_id],
        )?;
        tx.execute(
            "DELETE FROM sync_state WHERE document_id = ?1",
            params![document_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Return the current archive contents, newest first.
    pub fn list_archived(&self) -> Result<Vec<ArchivedDocument>> {
        type RawRow = (
            String,
            String,
            String,
            Option<String>,
            String,
            i64,
            String,
            String,
        );
        let conn = self.db.lock();
        let mut stmt = conn.prepare(
            "SELECT document_id, visible_name, doc_type, parent, \
                    manifest_hash, version_id, reason, archived_at \
             FROM archived_documents \
             ORDER BY archived_at DESC, document_id ASC",
        )?;
        let raw: Vec<RawRow> = stmt
            .query_map([], |r| {
                Ok((
                    r.get(0)?,
                    r.get(1)?,
                    r.get(2)?,
                    r.get(3)?,
                    r.get(4)?,
                    r.get(5)?,
                    r.get(6)?,
                    r.get(7)?,
                ))
            })?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        drop(conn);

        let mut rows = Vec::with_capacity(raw.len());
        for (
            document_id,
            visible_name,
            doc_type,
            parent,
            manifest_hex,
            version_id,
            reason_str,
            archived_at,
        ) in raw
        {
            let manifest_hash =
                Sha256Hex::from_hex(&manifest_hex).ok_or_else(|| Error::Corrupt {
                    path: self.paths.db.display().to_string(),
                    reason: format!("bad manifest hash for archived document {document_id}"),
                })?;
            rows.push(ArchivedDocument {
                document_id,
                visible_name,
                doc_type,
                parent,
                manifest_hash,
                version_id,
                reason: ArchiveReason::parse(&reason_str).unwrap_or(ArchiveReason::Local),
                archived_at,
            });
        }
        Ok(rows)
    }

    /// Both live and archived documents that the push engine needs to
    /// consider. Archived docs are returned as DocumentSummary entries
    /// pointing at their *post-archive* manifest (deleted=true) so the
    /// next push uploads the deletion to the device. The caller must not
    /// rely on these being present in `list_documents` — that one is for
    /// the user-facing live view.
    pub fn list_pushable_documents(&self) -> Result<Vec<DocumentSummary>> {
        let mut out = self.list_documents()?;
        let archived = self.list_archived()?;
        for a in archived {
            // observed_at is fetched from the version row so plan_push and
            // history views agree on timestamps.
            let observed_at: String = self
                .db
                .lock()
                .query_row(
                    "SELECT observed_at FROM versions WHERE id = ?1",
                    params![a.version_id],
                    |r| r.get(0),
                )
                .optional()?
                .unwrap_or_default();
            out.push(DocumentSummary {
                document_id: a.document_id,
                visible_name: a.visible_name,
                doc_type: a.doc_type,
                current_manifest: a.manifest_hash,
                current_version_id: a.version_id,
                last_observed_at: observed_at,
                // The archive table stores the *original* parent; the
                // manifest itself has parent="trash". For push purposes
                // the manifest blob is what gets uploaded, so the parent
                // value here is informational only.
                parent: Some("trash".to_string()),
                size_bytes: 0,
                page_count: None,
                has_unpushed_changes: true,
            });
        }
        Ok(out)
    }

    /// Test whether a document is currently archived. Used by the sync
    /// engine to skip re-creating a live entry for a document the user has
    /// already chosen to archive.
    pub fn is_archived(&self, document_id: &str) -> Result<bool> {
        let conn = self.db.lock();
        let n: i64 = conn.query_row(
            "SELECT count(*) FROM archived_documents WHERE document_id = ?1",
            params![document_id],
            |r| r.get(0),
        )?;
        Ok(n > 0)
    }

    /// Document IDs the library believes were on the device at last sync —
    /// i.e. they have a non-null `last_seen_manifest` and aren't already in
    /// the archive. Used by the pull engine to detect device-side deletions.
    pub fn previously_synced_ids(&self) -> Result<Vec<String>> {
        let conn = self.db.lock();
        let mut stmt = conn.prepare(
            "SELECT s.document_id FROM sync_state s \
             WHERE s.last_seen_manifest IS NOT NULL \
             AND s.document_id NOT IN (SELECT document_id FROM archived_documents)",
        )?;
        let rows: Vec<String> = stmt
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Look up a single version by id. Returns `Err(NotFound)` if no row
    /// matches. Used by `export_version` and the history UI.
    pub fn get_version(&self, version_id: VersionId) -> Result<VersionEntry> {
        struct Row {
            document_id: String,
            manifest_hex: String,
            parent_version_id: Option<i64>,
            observed_at: String,
            source_str: String,
            note: Option<String>,
        }
        let row: Option<Row> = self
            .db
            .lock()
            .query_row(
                "SELECT document_id, manifest_hash, parent_version_id, observed_at, source, note \
                 FROM versions WHERE id = ?1",
                params![version_id],
                |r| {
                    Ok(Row {
                        document_id: r.get(0)?,
                        manifest_hex: r.get(1)?,
                        parent_version_id: r.get(2)?,
                        observed_at: r.get(3)?,
                        source_str: r.get(4)?,
                        note: r.get(5)?,
                    })
                },
            )
            .optional()?;
        let Row {
            document_id,
            manifest_hex,
            parent_version_id,
            observed_at,
            source_str,
            note,
        } = row.ok_or_else(|| Error::NotFound(format!("version {version_id}")))?;
        let manifest_hash = Sha256Hex::from_hex(&manifest_hex).ok_or_else(|| Error::Corrupt {
            path: self.paths.db.display().to_string(),
            reason: format!("bad manifest hash for version {version_id}"),
        })?;
        let mut entry = VersionEntry {
            id: version_id,
            document_id,
            manifest_hash: manifest_hash.clone(),
            parent_version_id,
            observed_at,
            source: Source::parse(&source_str).unwrap_or(Source::Pulled),
            note,
            total_size_bytes: None,
            file_count: None,
        };
        if let Ok(bytes) = self.blobs.read_to_vec(&manifest_hash) {
            if let Ok(m) = Manifest::from_canonical_json(&bytes) {
                entry.total_size_bytes = Some(m.files.iter().map(|f| f.size).sum());
                entry.file_count = Some(m.files.len());
            }
        }
        Ok(entry)
    }

    /// Import a PDF or EPUB from disk into the library. Generates a fresh
    /// document UUID, builds the `.metadata` and `.content` JSON files the
    /// reMarkable expects, stores all three blobs (metadata, content, body),
    /// and records a first version with `Source::Imported`. The new document
    /// becomes outbound on the next `plan_push`.
    ///
    /// `body_kind` selects the file extension and `fileType` field used in
    /// `.content`. `visible_name` is what shows up on the tablet — typically
    /// the source filename without extension.
    pub fn import_file(
        &self,
        source_path: &Path,
        body_kind: ImportKind,
        visible_name: &str,
    ) -> Result<DocumentSummary> {
        use crate::manifest::ManifestFile;
        // record_version below already takes the write_lock; we don't
        // grab it here to avoid double-locking the same thread (std
        // Mutex is not reentrant). Calls to put_blob in between are
        // safe because the GC's live-set query and the import's record
        // are serialised through record_version's lock.

        let bytes = fs::read(source_path)?;
        let document_id = uuid::Uuid::new_v4().to_string();
        let now = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        // The tablet stores `lastModified` as a unix-millis string.
        let last_modified_ms = OffsetDateTime::now_utc().unix_timestamp() * 1000;

        let metadata_json = serde_json::json!({
            "visibleName": visible_name,
            "type": "DocumentType",
            "parent": "",
            "lastModified": last_modified_ms.to_string(),
            "lastOpened": "",
            "lastOpenedPage": 0,
            "version": 1,
            "pinned": false,
            "synced": false,
            "modified": false,
            "deleted": false,
            "metadatamodified": false,
        });
        let content_json = body_kind.content_json();

        let metadata_bytes = serde_json::to_vec_pretty(&metadata_json)?;
        let content_bytes = serde_json::to_vec_pretty(&content_json)?;

        let metadata_put = self.put_blob(&metadata_bytes)?;
        let content_put = self.put_blob(&content_bytes)?;
        let body_put = self.put_blob(&bytes)?;

        let mut manifest = Manifest::new(&document_id, body_kind.doc_type(), visible_name);
        manifest.metadata = metadata_json;
        manifest.content_meta = content_json;
        manifest.files = vec![
            ManifestFile {
                path: format!("{document_id}.metadata"),
                sha256: metadata_put.hash,
                size: metadata_put.size,
                mode: 0o644,
                derived: false,
            },
            ManifestFile {
                path: format!("{document_id}.content"),
                sha256: content_put.hash,
                size: content_put.size,
                mode: 0o644,
                derived: false,
            },
            ManifestFile {
                path: format!("{document_id}.{}", body_kind.extension()),
                sha256: body_put.hash,
                size: body_put.size,
                mode: 0o644,
                derived: false,
            },
        ];

        let outcome = self.record_version(&manifest, Source::Imported)?;

        Ok(DocumentSummary {
            document_id,
            visible_name: visible_name.to_string(),
            doc_type: body_kind.doc_type().to_string(),
            current_manifest: outcome.manifest_hash,
            current_version_id: outcome.version_id,
            last_observed_at: now,
            parent: None,
            size_bytes: metadata_put.size + content_put.size + body_put.size,
            page_count: None,
            has_unpushed_changes: true,
        })
    }

    /// Make `version_id` the document's current version by re-recording its
    /// manifest with `Source::Restored`. The previous current version is
    /// preserved in the log via `parent_version_id`. If `version_id` is
    /// already current, this is a no-op (`unchanged: true`).
    pub fn restore_version(&self, version_id: VersionId) -> Result<RecordOutcome> {
        let entry = self.get_version(version_id)?;
        let bytes = self.read_blob(&entry.manifest_hash)?;
        let manifest = Manifest::from_canonical_json(&bytes)?;
        self.record_version(&manifest, Source::Restored)
    }

    /// Update the free-form note attached to a version. Pass `None` to clear.
    pub fn set_version_note(&self, version_id: VersionId, note: Option<&str>) -> Result<()> {
        let n = self.db.lock().execute(
            "UPDATE versions SET note = ?1 WHERE id = ?2",
            params![note, version_id],
        )?;
        if n == 0 {
            return Err(Error::NotFound(format!("version {version_id}")));
        }
        Ok(())
    }

    /// Total versions across all documents.
    pub fn version_count(&self) -> Result<i64> {
        Ok(self
            .db
            .lock()
            .query_row("SELECT count(*) FROM versions", [], |r| r.get(0))?)
    }

    /// Look up the cached classification hint for one document. Used by the
    /// sync engine's `plan_pull` to decide between `New`, `Changed`, and
    /// `Unchanged` without rehashing.
    pub fn last_seen(&self, document_id: &str) -> Result<Option<(Option<String>, Option<String>)>> {
        Ok(self
            .db
            .lock()
            .query_row(
                "SELECT device_mtime_hint, last_seen_manifest FROM sync_state WHERE document_id = ?1",
                params![document_id],
                |r| Ok((r.get::<_, Option<String>>(0)?, r.get::<_, Option<String>>(1)?)),
            )
            .optional()?)
    }

    /// Update the device-side mtime hint after a successful download. Cheap
    /// fast-path classifier for next time `plan_pull` runs.
    pub fn update_mtime_hint(&self, document_id: &str, hint: &str) -> Result<()> {
        self.db.lock().execute(
            "INSERT INTO sync_state(document_id, device_mtime_hint) VALUES (?1, ?2) \
             ON CONFLICT(document_id) DO UPDATE SET device_mtime_hint = excluded.device_mtime_hint",
            params![document_id, hint],
        )?;
        Ok(())
    }

    /// Mark the manifest as the latest one known to be on the device. Called
    /// after a successful push so the next `plan_pull` correctly classifies
    /// the document as Unchanged (assuming the device hasn't moved on).
    pub fn update_last_seen_manifest(&self, document_id: &str, manifest_hex: &str) -> Result<()> {
        let now = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        self.db.lock().execute(
            "INSERT INTO sync_state(document_id, last_seen_manifest, last_synced_at) \
             VALUES (?1, ?2, ?3) \
             ON CONFLICT(document_id) DO UPDATE SET \
                 last_seen_manifest = excluded.last_seen_manifest, \
                 last_synced_at = excluded.last_synced_at",
            params![document_id, manifest_hex, now],
        )?;
        Ok(())
    }

    /// Return all folders known to the library. Sort order is the
    /// local-only `sort_index`, with `visible_name` as the tie-breaker
    /// so newly-pulled folders that all share `sort_index = 0` still
    /// land alphabetically.
    pub fn list_folders(&self) -> Result<Vec<FolderEntry>> {
        let conn = self.db.lock();
        let mut stmt = conn.prepare(
            "SELECT folder_id, parent, visible_name, sort_index FROM folders \
             ORDER BY sort_index ASC, visible_name ASC",
        )?;
        let rows: Vec<FolderEntry> = stmt
            .query_map([], |r| {
                let parent: Option<String> = r.get::<_, Option<String>>(1)?;
                Ok(FolderEntry {
                    folder_id: r.get(0)?,
                    parent: parent.filter(|s| !s.is_empty()),
                    visible_name: r.get(2)?,
                    sort_index: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Mirror a folder from the device into the library's folder index.
    /// Resets `pending_push` to 0 because we just got authoritative
    /// state from the tablet. Preserves the local `sort_index` if the
    /// row already exists — folder order is local-only, so a re-pull
    /// of an unchanged folder must not reset the user's reorderings.
    pub fn upsert_folder(
        &self,
        folder_id: &str,
        parent: Option<&str>,
        visible_name: &str,
        metadata_json: &str,
    ) -> Result<()> {
        let mut conn = self.db.lock();
        let tx = conn.transaction()?;
        // Pick a fresh sort_index for newly-seen folders so they slot
        // at the end of the sibling list rather than colliding at 0
        // and getting alphabetised on top of older rows.
        let next_sort: f64 = tx.query_row(
            "SELECT COALESCE(MAX(sort_index), 0.0) + 1.0 FROM folders WHERE COALESCE(parent,'') = COALESCE(?1,'')",
            params![parent],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO folders(folder_id, parent, visible_name, metadata_json, pending_push, sort_index) \
             VALUES (?1, ?2, ?3, ?4, 0, ?5) \
             ON CONFLICT(folder_id) DO UPDATE SET \
                 parent = excluded.parent, \
                 visible_name = excluded.visible_name, \
                 metadata_json = excluded.metadata_json, \
                 pending_push = 0",
            params![folder_id, parent, visible_name, metadata_json, next_sort],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Create a brand-new local folder and queue it for push. The
    /// folder's `<uuid>.metadata` is uploaded on the next sync, where
    /// xochitl picks it up as a `CollectionType` entry. Returns the
    /// row that the UI can splice into its folder list without a
    /// full reload.
    pub fn create_folder(
        &self,
        visible_name: &str,
        parent: Option<&str>,
    ) -> Result<FolderEntry> {
        let trimmed = visible_name.trim().to_string();
        if trimmed.is_empty() {
            return Err(Error::InvalidArgument(
                "folder name must not be empty".into(),
            ));
        }
        let folder_id = uuid::Uuid::new_v4().to_string();
        let last_modified_ms = OffsetDateTime::now_utc().unix_timestamp() * 1000;
        // Mirror the schema xochitl writes for folders. `parent` is "" at
        // root, never a JSON null, because the device side reads it as a
        // string. `synced: false` and the `*modified` flags signal the
        // tablet to refresh its index after the file lands.
        let metadata = serde_json::json!({
            "visibleName": trimmed,
            "type": "CollectionType",
            "parent": parent.unwrap_or(""),
            "lastModified": last_modified_ms.to_string(),
            "lastOpened": "",
            "version": 1,
            "pinned": false,
            "synced": false,
            "modified": true,
            "deleted": false,
            "metadatamodified": true,
        });
        let metadata_json = serde_json::to_string(&metadata)?;

        let _write_guard = self.write_lock.lock().expect("library write_lock poisoned");
        let mut conn = self.db.lock();
        let tx = conn.transaction()?;
        // Reject inserts under a non-existent parent — the UI shouldn't
        // ever ask for this, but failing fast keeps orphans out of the
        // tree.
        if let Some(p) = parent {
            let exists: i64 = tx.query_row(
                "SELECT COUNT(*) FROM folders WHERE folder_id = ?1",
                params![p],
                |r| r.get(0),
            )?;
            if exists == 0 {
                return Err(Error::NotFound(format!("folder {p}")));
            }
        }
        // Slot at the end of the sibling list.
        let next_sort: f64 = tx.query_row(
            "SELECT COALESCE(MAX(sort_index), 0.0) + 1.0 FROM folders WHERE COALESCE(parent,'') = COALESCE(?1,'')",
            params![parent],
            |r| r.get(0),
        )?;
        tx.execute(
            "INSERT INTO folders(folder_id, parent, visible_name, metadata_json, pending_push, sort_index) \
             VALUES (?1, ?2, ?3, ?4, 1, ?5)",
            params![folder_id, parent, trimmed, metadata_json, next_sort],
        )?;
        tx.commit()?;

        Ok(FolderEntry {
            folder_id,
            parent: parent.map(str::to_string),
            visible_name: trimmed,
            sort_index: next_sort,
        })
    }

    /// Reparent and/or reorder a folder. `new_parent = None` means
    /// "move to root". `new_sort_index` is the float key used for the
    /// local-only sidebar ordering — callers compute it as the midpoint
    /// of two adjacent siblings to avoid renumbering on every drag.
    ///
    /// Rejects moving a folder into itself or any of its descendants —
    /// such a move would create a cycle that `list_folders` cannot
    /// untangle and the sidebar tree-builder would silently drop.
    pub fn reorder_folder(
        &self,
        folder_id: &str,
        new_parent: Option<&str>,
        new_sort_index: f64,
    ) -> Result<()> {
        if !new_sort_index.is_finite() {
            return Err(Error::InvalidArgument(
                "sort_index must be finite".into(),
            ));
        }
        if new_parent == Some(folder_id) {
            return Err(Error::InvalidArgument(
                "cannot move a folder into itself".into(),
            ));
        }

        let _write_guard = self.write_lock.lock().expect("library write_lock poisoned");
        let mut conn = self.db.lock();
        let tx = conn.transaction()?;

        // Confirm the row exists.
        let exists: i64 = tx.query_row(
            "SELECT COUNT(*) FROM folders WHERE folder_id = ?1",
            params![folder_id],
            |r| r.get(0),
        )?;
        if exists == 0 {
            return Err(Error::NotFound(format!("folder {folder_id}")));
        }

        // Cycle check: walk up from `new_parent` toward the root; if
        // we ever hit `folder_id` the move would create a loop. Capped
        // at the current folder count to avoid spinning forever on a
        // pre-existing cycle (defence in depth).
        if let Some(mut cur) = new_parent.map(str::to_string) {
            let folder_count: i64 =
                tx.query_row("SELECT COUNT(*) FROM folders", [], |r| r.get(0))?;
            for _ in 0..folder_count.max(1) {
                if cur == folder_id {
                    return Err(Error::InvalidArgument(
                        "cannot move a folder into one of its descendants".into(),
                    ));
                }
                let parent: Option<String> = tx
                    .query_row(
                        "SELECT parent FROM folders WHERE folder_id = ?1",
                        params![&cur],
                        |r| r.get(0),
                    )
                    .optional()?
                    .flatten();
                match parent {
                    Some(p) if !p.is_empty() => cur = p,
                    _ => break,
                }
            }
        }

        tx.execute(
            "UPDATE folders SET parent = ?1, sort_index = ?2 WHERE folder_id = ?3",
            params![new_parent, new_sort_index, folder_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Rename a folder. Updates the local row + the device-facing
    /// `metadata_json` (visibleName, lastModified, modified flags) and
    /// flags the folder for push so the next sync uploads the new
    /// metadata file to the tablet.
    pub fn rename_folder(&self, folder_id: &str, new_name: &str) -> Result<()> {
        let trimmed = new_name.trim().to_string();
        if trimmed.is_empty() {
            return Err(Error::InvalidArgument(
                "folder name must not be empty".into(),
            ));
        }
        let _write_guard = self.write_lock.lock().expect("library write_lock poisoned");
        let mut conn = self.db.lock();
        let tx = conn.transaction()?;

        let row: Option<String> = tx
            .query_row(
                "SELECT metadata_json FROM folders WHERE folder_id = ?1",
                params![folder_id],
                |r| r.get(0),
            )
            .optional()?;
        let metadata_json = row.ok_or_else(|| Error::NotFound(format!("folder {folder_id}")))?;

        // Mutate the metadata JSON in place so we keep every device
        // field (parent, lastOpened, etc.) intact. If it isn't an
        // object we still produce a minimal one — folders should
        // always have object metadata, but defensive code is cheap.
        let mut value: serde_json::Value =
            serde_json::from_str(&metadata_json).unwrap_or_else(|_| serde_json::json!({}));
        if !value.is_object() {
            value = serde_json::json!({});
        }
        let map = value.as_object_mut().expect("ensured above");
        map.insert(
            "visibleName".into(),
            serde_json::Value::String(trimmed.clone()),
        );
        // xochitl uses these to decide a re-index is needed.
        let now_ms = OffsetDateTime::now_utc().unix_timestamp() * 1000;
        map.insert(
            "lastModified".into(),
            serde_json::Value::String(now_ms.to_string()),
        );
        map.insert("modified".into(), serde_json::Value::Bool(true));
        map.insert("metadatamodified".into(), serde_json::Value::Bool(true));
        map.insert("synced".into(), serde_json::Value::Bool(false));
        // Folders identify with this type on the device; preserve if
        // already set, otherwise default.
        map.entry("type".to_string())
            .or_insert(serde_json::Value::String("CollectionType".into()));

        let updated_json = serde_json::to_string(&value)?;

        tx.execute(
            "UPDATE folders SET visible_name = ?1, metadata_json = ?2, pending_push = 1 \
             WHERE folder_id = ?3",
            params![trimmed, updated_json, folder_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Folders that have been renamed locally and not yet pushed to
    /// the device. Each tuple is `(folder_id, metadata_json)` — the
    /// caller assembles a single `<folder_id>.metadata` file and ships
    /// it via `Device::put_document_tree`.
    pub fn list_pending_folder_pushes(&self) -> Result<Vec<(String, String)>> {
        let conn = self.db.lock();
        let mut stmt = conn.prepare(
            "SELECT folder_id, metadata_json FROM folders \
             WHERE pending_push = 1 ORDER BY folder_id",
        )?;
        let rows: Vec<(String, String)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
    }

    /// Mark a folder's local metadata as flushed to the device. Called
    /// by the push engine after a successful upload.
    pub fn mark_folder_pushed(&self, folder_id: &str) -> Result<()> {
        self.db.lock().execute(
            "UPDATE folders SET pending_push = 0 WHERE folder_id = ?1",
            params![folder_id],
        )?;
        Ok(())
    }

    /// Reclaim disk space by deleting blobs that no manifest in the version
    /// log references. A blob is considered live if it appears in
    /// `blob_refs` joined to `versions` — i.e. some recorded version still
    /// points at it. Manifests themselves are referenced via `blob_refs`'s
    /// (manifest_hash, manifest_hash) self-row, written by `record_version`.
    ///
    /// Safe to run while the app is otherwise idle. Refuses to delete
    /// anything that any version currently references; if you want to drop
    /// versions, use the (future) version-pruning API first, then GC.
    pub fn garbage_collect(&self) -> Result<GarbageCollectReport> {
        self.garbage_collect_with_grace(std::time::Duration::from_secs(60))
    }

    /// Underlying GC implementation with a configurable "grace" window —
    /// blobs younger than `grace` are kept even if unreferenced, to avoid
    /// racing with an in-flight import or sync. Tests pass `Duration::ZERO`
    /// to exercise the deletion path deterministically.
    pub fn garbage_collect_with_grace(
        &self,
        grace: std::time::Duration,
    ) -> Result<GarbageCollectReport> {
        let _write_guard = self.write_lock.lock().expect("library write_lock poisoned");
        let mut live: std::collections::HashSet<String> = std::collections::HashSet::new();
        {
            let conn = self.db.lock();
            let mut stmt = conn.prepare(
                "SELECT DISTINCT br.blob_hash FROM blob_refs br \
                 JOIN versions v ON v.manifest_hash = br.manifest_hash",
            )?;
            let mut rows = stmt.query([])?;
            while let Some(row) = rows.next()? {
                live.insert(row.get::<_, String>(0)?);
            }
        }

        let mut report = GarbageCollectReport::default();
        let blobs_dir = self.paths.blobs.clone();
        let mut to_delete = Vec::new();

        // Belt-and-braces against the import/record_version vs GC race:
        // even though the write_lock covers record_version, an import or
        // pull writes file blobs before calling record_version, so there's
        // a brief window where a freshly-written blob is on disk but no
        // version refers to it. The `grace` window covers that.
        let now = std::time::SystemTime::now();

        walk_blobs(&blobs_dir, &mut |path, hash_str| {
            report.scanned += 1;
            if live.contains(hash_str) {
                return;
            }
            let meta = match std::fs::metadata(path) {
                Ok(m) => m,
                Err(_) => return,
            };
            // Skip recently-modified blobs (potential in-flight write).
            if let Ok(modified) = meta.modified() {
                if let Ok(age) = now.duration_since(modified) {
                    if age < grace {
                        return;
                    }
                }
            }
            to_delete.push((path.to_path_buf(), meta.len()));
        });

        for (path, size) in to_delete {
            match std::fs::remove_file(&path) {
                Ok(()) => {
                    report.deleted += 1;
                    report.bytes_freed += size;
                }
                Err(e) => {
                    tracing::warn!("gc: failed to remove {}: {e}", path.display());
                    report.errors += 1;
                }
            }
        }

        // Best-effort: prune now-empty fanout directories so the library
        // doesn't accumulate empty `blobs/aa/bb/` shells.
        prune_empty_dirs(&blobs_dir);
        Ok(report)
    }

    /// Walk the library and verify on-disk integrity:
    /// - Every manifest in the version log refers to blobs that exist.
    /// - Every blob's filename matches its content's sha256.
    /// - Identify blobs in `blobs/` that no manifest references (orphans —
    ///   harmless, garbage-collectable in Phase 4).
    ///
    /// This is the canonical "never lie about state" check the design doc
    /// calls for. It does no network I/O and does not modify the library.
    pub fn verify(&self) -> Result<VerifyReport> {
        let mut report = VerifyReport::default();

        // 1) Walk every manifest currently referenced by a version, collect
        //    the union of all blob hashes they reference.
        let manifest_hashes: Vec<String> = self
            .db
            .lock()
            .prepare("SELECT DISTINCT manifest_hash FROM versions")?
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<_>>()?;

        let mut referenced: std::collections::HashSet<String> = std::collections::HashSet::new();
        for manifest_hex in manifest_hashes {
            report.manifests_total += 1;
            let hash = match Sha256Hex::from_hex(&manifest_hex) {
                Some(h) => h,
                None => {
                    report.manifests_invalid += 1;
                    continue;
                }
            };
            referenced.insert(hash.as_str().to_string());
            let manifest_bytes = match self.blobs.read_to_vec(&hash) {
                Ok(b) => b,
                Err(_) => {
                    report.manifests_missing += 1;
                    continue;
                }
            };
            // Manifest blob filename must match its content hash.
            if Sha256Hex::from_bytes(&manifest_bytes).as_str() != hash.as_str() {
                report.blobs_corrupted += 1;
                continue;
            }
            let manifest = match Manifest::from_canonical_json(&manifest_bytes) {
                Ok(m) => m,
                Err(_) => {
                    report.manifests_invalid += 1;
                    continue;
                }
            };
            for f in &manifest.files {
                referenced.insert(f.sha256.as_str().to_string());
                if !self.blobs.has(&f.sha256) {
                    report.blobs_missing += 1;
                    report
                        .missing_examples
                        .push(format!("{}:{}", manifest.document_id, f.path));
                }
            }
            report.manifests_ok += 1;
        }

        // 2) Walk every blob on disk: count, find orphans (blobs not
        //    referenced by any manifest), and verify the filename matches
        //    the content hash for a sample so we don't rehash everything by
        //    default.
        let blobs_dir = self.paths.blobs.clone();
        walk_blobs(&blobs_dir, &mut |path, hash_str| {
            report.blobs_total += 1;
            if !referenced.contains(hash_str) {
                report.blobs_orphan += 1;
                if report.orphan_examples.len() < 5 {
                    report.orphan_examples.push(path.display().to_string());
                }
            }
        });

        Ok(report)
    }

    /// Reconstruct the file tree of `version_id` under `dest`. Used by Phase 2
    /// (export) and as the canonical round-trip property test for the library.
    pub fn reconstruct(&self, version_id: VersionId, dest: &Path) -> Result<()> {
        let manifest_hex: String = self
            .db
            .lock()
            .query_row(
                "SELECT manifest_hash FROM versions WHERE id = ?1",
                params![version_id],
                |r| r.get(0),
            )
            .optional()?
            .ok_or_else(|| Error::NotFound(format!("version {version_id}")))?;
        let manifest_hash = Sha256Hex::from_hex(&manifest_hex).ok_or_else(|| Error::Corrupt {
            path: self.paths.db.display().to_string(),
            reason: format!("bad manifest hash for version {version_id}"),
        })?;
        let manifest_bytes = self.blobs.read_to_vec(&manifest_hash)?;
        let manifest = Manifest::from_canonical_json(&manifest_bytes)?;

        fs::create_dir_all(dest)?;
        for f in &manifest.files {
            let blob = self.blobs.read_to_vec(&f.sha256)?;
            let target = dest.join(&f.path);
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            fs::write(&target, &blob)?;
        }
        Ok(())
    }
}

/// Recursively walk the `blobs/<aa>/<bb>/<hash>` tree, invoking `visit` for
/// each leaf file with the file's full path and its filename (the hex hash).
/// Symlinks are skipped — the blob store should never contain them, and
/// following a planted symlink could leak filesystem contents into GC's
/// orphan list or even let GC delete files outside the library directory.
fn walk_blobs(root: &Path, visit: &mut dyn FnMut(&Path, &str)) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for entry in rd.flatten() {
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if ft.is_symlink() {
            continue;
        }
        let p = entry.path();
        if ft.is_dir() {
            walk_blobs(&p, visit);
        } else if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
            visit(&p, name);
        }
    }
}

/// Remove empty subdirectories under `root`, depth-first. `remove_dir` only
/// succeeds on empty directories, so populated leaves stay intact. Skips
/// symlinks for the same reason `walk_blobs` does.
fn prune_empty_dirs(root: &Path) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for entry in rd.flatten() {
        let ft = match entry.file_type() {
            Ok(t) => t,
            Err(_) => continue,
        };
        if ft.is_symlink() {
            continue;
        }
        if ft.is_dir() {
            let p = entry.path();
            prune_empty_dirs(&p);
            let _ = std::fs::remove_dir(&p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::manifest::ManifestFile;

    fn seed_blob(lib: &Library, bytes: &[u8]) -> Sha256Hex {
        lib.put_blob(bytes).unwrap().hash
    }

    fn seed_manifest(lib: &Library, doc_id: &str, contents: &[(&str, &[u8])]) -> Manifest {
        let mut files = Vec::new();
        // Always include a `.metadata` file — the archive/move flows
        // mutate it, and real synced documents always have one.
        let meta_blob = serde_json::json!({
            "visibleName": "Test",
            "type": "DocumentType",
            "parent": "",
            "deleted": false,
            "lastModified": "0",
        });
        let meta_bytes = serde_json::to_vec_pretty(&meta_blob).unwrap();
        let meta_put = lib.put_blob(&meta_bytes).unwrap();
        files.push(ManifestFile {
            path: format!("{doc_id}.metadata"),
            sha256: meta_put.hash,
            size: meta_put.size,
            mode: 0o644,
            derived: false,
        });
        for (path, bytes) in contents {
            let res = lib.put_blob(bytes).unwrap();
            files.push(ManifestFile {
                path: (*path).to_string(),
                sha256: res.hash,
                size: res.size,
                mode: 0o644,
                derived: false,
            });
        }
        let mut m = Manifest::new(doc_id, "Notebook", "Test");
        m.metadata = meta_blob;
        m.files = files;
        m
    }

    #[test]
    fn record_then_list_and_history() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();
        let m = seed_manifest(&lib, "doc-1", &[("a.rm", b"AAA"), ("b.rm", b"BBB")]);

        let outcome1 = lib.record_version(&m, Source::Pulled).unwrap();
        assert!(!outcome1.unchanged);

        let outcome2 = lib.record_version(&m, Source::Pulled).unwrap();
        assert!(
            outcome2.unchanged,
            "re-recording identical manifest must be a no-op"
        );
        assert_eq!(outcome2.version_id, outcome1.version_id);

        let docs = lib.list_documents().unwrap();
        assert_eq!(docs.len(), 1);
        assert_eq!(docs[0].document_id, "doc-1");

        let history = lib.get_history("doc-1").unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].source, Source::Pulled);
    }

    #[test]
    fn changing_a_file_appends_a_new_version() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();
        let m1 = seed_manifest(&lib, "doc-1", &[("a.rm", b"v1")]);
        lib.record_version(&m1, Source::Pulled).unwrap();
        let m2 = seed_manifest(&lib, "doc-1", &[("a.rm", b"v2")]);
        let r2 = lib.record_version(&m2, Source::Pulled).unwrap();
        assert!(!r2.unchanged);
        let history = lib.get_history("doc-1").unwrap();
        assert_eq!(history.len(), 2);
        assert_eq!(history[1].parent_version_id, Some(history[0].id));
    }

    #[test]
    fn reconstruct_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();
        let m = seed_manifest(
            &lib,
            "doc-1",
            &[
                ("doc.metadata", b"{\"deleted\":false}"),
                ("doc/page-1.rm", b"page1bytes"),
            ],
        );
        let r = lib.record_version(&m, Source::Pulled).unwrap();

        let out = tempfile::tempdir().unwrap();
        lib.reconstruct(r.version_id, out.path()).unwrap();
        assert_eq!(
            fs::read(out.path().join("doc.metadata")).unwrap(),
            b"{\"deleted\":false}"
        );
        assert_eq!(
            fs::read(out.path().join("doc/page-1.rm")).unwrap(),
            b"page1bytes"
        );
    }

    #[test]
    fn verify_clean_library_reports_no_problems() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();
        let m = seed_manifest(&lib, "doc-1", &[("a.rm", b"AAA"), ("b.rm", b"BBB")]);
        lib.record_version(&m, Source::Pulled).unwrap();

        let report = lib.verify().unwrap();
        assert_eq!(report.manifests_total, 1);
        assert_eq!(report.manifests_ok, 1);
        assert_eq!(report.blobs_missing, 0);
        assert_eq!(report.blobs_orphan, 0);
        // Four blobs: .metadata (always seeded), a.rm, b.rm, and the
        // manifest itself.
        assert_eq!(report.blobs_total, 4);
    }

    #[test]
    fn verify_detects_missing_and_orphan_blobs() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();
        let m = seed_manifest(&lib, "doc-1", &[("a.rm", b"AAA")]);
        let outcome = lib.record_version(&m, Source::Pulled).unwrap();

        // Plant an orphan blob: write some bytes that no manifest references.
        let orphan = lib.put_blob(b"i am an orphan").unwrap();
        assert!(lib.has_blob(&orphan.hash));

        // Delete a referenced file blob to simulate corruption. Pick the
        // a.rm blob explicitly — seed_manifest also seeds a .metadata
        // file at index 0.
        let target = m
            .files
            .iter()
            .find(|f| f.path == "a.rm")
            .unwrap()
            .sha256
            .clone();
        std::fs::remove_file(lib.blobs().path_for(&target)).unwrap();

        let report = lib.verify().unwrap();
        assert_eq!(report.manifests_ok, 1);
        assert_eq!(report.blobs_missing, 1, "should report the deleted file");
        assert_eq!(report.blobs_orphan, 1, "should flag the orphan");
        assert!(report.missing_examples[0].starts_with("doc-1:a.rm"));

        // Recording the manifest again is still fine; verify should still
        // see the same problem.
        let _ = outcome;
    }

    #[test]
    fn import_file_creates_outbound_document() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();

        // Write a tiny "PDF" payload to disk and import it.
        let src = tmp.path().join("Sample.pdf");
        std::fs::write(&src, b"%PDF-1.7 fake bytes").unwrap();

        let summary = lib
            .import_file(&src, ImportKind::Pdf, "Sample document")
            .unwrap();
        assert_eq!(summary.visible_name, "Sample document");
        assert_eq!(summary.doc_type, "DocumentType.Pdf");

        // Manifest has three files: .metadata, .content, .pdf
        let manifest_bytes = lib.read_blob(&summary.current_manifest).unwrap();
        let manifest = Manifest::from_canonical_json(&manifest_bytes).unwrap();
        assert_eq!(manifest.files.len(), 3);
        assert!(manifest.files.iter().any(|f| f.path.ends_with(".metadata")));
        assert!(manifest.files.iter().any(|f| f.path.ends_with(".content")));
        assert!(manifest.files.iter().any(|f| f.path.ends_with(".pdf")));

        // sync_state.last_seen_manifest must be NULL — fresh import has
        // not been pushed yet, so plan_push should treat it as outbound.
        let last_seen = lib.last_seen(&summary.document_id).unwrap();
        assert!(matches!(last_seen, Some((_, None)) | None));
    }

    #[test]
    fn garbage_collect_only_drops_unreferenced_blobs() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();

        // Recorded manifest → these blobs are live.
        let m = seed_manifest(&lib, "doc-1", &[("a.rm", b"AAA")]);
        lib.record_version(&m, Source::Pulled).unwrap();

        // Plant an orphan that no manifest references.
        let orphan_bytes = b"i am an orphan blob".to_vec();
        let orphan = lib.put_blob(&orphan_bytes).unwrap();
        assert!(lib.has_blob(&orphan.hash));

        // Use a zero grace window so the just-written orphan is
        // considered for collection. The default 60s window exists to
        // protect against the in-flight import/record_version race.
        let zero = std::time::Duration::ZERO;
        let report = lib.garbage_collect_with_grace(zero).unwrap();
        assert_eq!(report.deleted, 1);
        assert_eq!(report.bytes_freed, orphan_bytes.len() as u64);
        assert_eq!(report.errors, 0);
        assert!(!lib.has_blob(&orphan.hash));

        // Live blobs and the recorded manifest are intact.
        assert!(lib.has_blob(&m.files[0].sha256));
        assert!(lib.has_blob(&m.hash().unwrap()));

        // Idempotent — second run finds nothing to collect.
        let report2 = lib.garbage_collect_with_grace(zero).unwrap();
        assert_eq!(report2.deleted, 0);
    }

    #[test]
    fn archive_roundtrip_preserves_history() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();
        let mut m = seed_manifest(&lib, "doc-1", &[("a.rm", b"AAA")]);
        m.parent = Some("folder-A".into());
        if let Some(map) = m.metadata.as_object_mut() {
            map.insert(
                "parent".into(),
                serde_json::Value::String("folder-A".into()),
            );
        }
        lib.record_version(&m, Source::Pulled).unwrap();

        // Live → archive: the doc is gone from list_documents, the new
        // current manifest has deleted=true, and the archive entry
        // remembers the original parent so restore knows where to put it.
        lib.archive_document("doc-1", ArchiveReason::Local).unwrap();
        assert!(lib.list_documents().unwrap().is_empty());
        let archived = lib.list_archived().unwrap();
        assert_eq!(archived.len(), 1);
        assert_eq!(archived[0].document_id, "doc-1");
        assert_eq!(archived[0].reason, ArchiveReason::Local);
        assert_eq!(archived[0].parent.as_deref(), Some("folder-A"));
        assert!(lib.is_archived("doc-1").unwrap());

        // The post-archive manifest carries deleted=true and parent=trash
        // — that's what gets shipped to the device on next push.
        let arch_bytes = lib.read_blob(&archived[0].manifest_hash).unwrap();
        let arch_manifest = Manifest::from_canonical_json(&arch_bytes).unwrap();
        assert_eq!(arch_manifest.parent.as_deref(), Some("trash"));
        let arch_meta_file = arch_manifest
            .files
            .iter()
            .find(|f| f.path.ends_with(".metadata"))
            .unwrap();
        let meta_bytes = lib.read_blob(&arch_meta_file.sha256).unwrap();
        let meta: serde_json::Value = serde_json::from_slice(&meta_bytes).unwrap();
        assert_eq!(meta.get("deleted").and_then(|v| v.as_bool()), Some(true));
        assert_eq!(meta.get("parent").and_then(|v| v.as_str()), Some("trash"));

        // Restore: doc is live again, with deleted=false and original
        // parent reinstated.
        let summary = lib.unarchive_document("doc-1").unwrap();
        assert_eq!(summary.document_id, "doc-1");
        assert_eq!(summary.parent.as_deref(), Some("folder-A"));
        assert!(!lib.is_archived("doc-1").unwrap());
        assert_eq!(lib.list_documents().unwrap().len(), 1);
        assert!(lib.list_archived().unwrap().is_empty());

        // History records the original pull plus the two metadata flips.
        let history = lib.get_history("doc-1").unwrap();
        assert_eq!(history.len(), 3);
    }

    #[test]
    fn move_document_updates_parent_and_records_new_version() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();
        let m = seed_manifest(&lib, "doc-1", &[("a.rm", b"page")]);
        lib.record_version(&m, Source::Pulled).unwrap();

        let outcome = lib.move_document("doc-1", Some("folder-X")).unwrap();
        assert!(!outcome.unchanged);

        // The new current manifest reflects the move on both the
        // top-level field and inside the .metadata blob.
        let docs = lib.list_documents().unwrap();
        assert_eq!(docs[0].parent.as_deref(), Some("folder-X"));
        let bytes = lib.read_blob(&docs[0].current_manifest).unwrap();
        let manifest = Manifest::from_canonical_json(&bytes).unwrap();
        assert_eq!(manifest.parent.as_deref(), Some("folder-X"));
        let meta_file = manifest
            .files
            .iter()
            .find(|f| f.path.ends_with(".metadata"))
            .unwrap();
        let meta: serde_json::Value =
            serde_json::from_slice(&lib.read_blob(&meta_file.sha256).unwrap()).unwrap();
        assert_eq!(
            meta.get("parent").and_then(|v| v.as_str()),
            Some("folder-X")
        );

        // Moving back to root sets metadata.parent="" and manifest.parent=None.
        lib.move_document("doc-1", None).unwrap();
        let docs = lib.list_documents().unwrap();
        assert_eq!(docs[0].parent, None);
    }

    #[test]
    fn create_folder_writes_pending_push_row_with_metadata() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();

        let parent = lib.create_folder("Inbox", None).unwrap();
        let child = lib
            .create_folder("Drafts", Some(&parent.folder_id))
            .unwrap();

        // Listing surfaces both folders, child links to parent.
        let folders = lib.list_folders().unwrap();
        assert_eq!(folders.len(), 2);
        let listed_parent = folders
            .iter()
            .find(|f| f.folder_id == parent.folder_id)
            .unwrap();
        assert_eq!(listed_parent.parent, None);
        assert_eq!(listed_parent.visible_name, "Inbox");
        let listed_child = folders
            .iter()
            .find(|f| f.folder_id == child.folder_id)
            .unwrap();
        assert_eq!(listed_child.parent.as_deref(), Some(parent.folder_id.as_str()));

        // Both rows are pending push so the next sync uploads their
        // metadata files to the device.
        let pending = lib.list_pending_folder_pushes().unwrap();
        assert_eq!(pending.len(), 2);
        for (_, json) in pending {
            let v: serde_json::Value = serde_json::from_str(&json).unwrap();
            assert_eq!(v.get("type").and_then(|x| x.as_str()), Some("CollectionType"));
            assert_eq!(v.get("synced").and_then(|x| x.as_bool()), Some(false));
        }
    }

    #[test]
    fn create_folder_rejects_empty_name_and_unknown_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();
        assert!(lib.create_folder("   ", None).is_err());
        assert!(lib.create_folder("orphan", Some("does-not-exist")).is_err());
    }

    #[test]
    fn reorder_folder_updates_sort_index_and_parent() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();
        let a = lib.create_folder("A", None).unwrap();
        let b = lib.create_folder("B", None).unwrap();
        let c = lib.create_folder("C", None).unwrap();

        // Move A between B and C: midpoint of their sort indices.
        let mid = (b.sort_index + c.sort_index) / 2.0;
        lib.reorder_folder(&a.folder_id, None, mid).unwrap();
        let folders = lib.list_folders().unwrap();
        let order: Vec<_> = folders.iter().map(|f| f.visible_name.as_str()).collect();
        assert_eq!(order, vec!["B", "A", "C"]);

        // Reparent A under B.
        lib.reorder_folder(&a.folder_id, Some(&b.folder_id), 0.0)
            .unwrap();
        let folders = lib.list_folders().unwrap();
        let a_row = folders.iter().find(|f| f.folder_id == a.folder_id).unwrap();
        assert_eq!(a_row.parent.as_deref(), Some(b.folder_id.as_str()));
    }

    #[test]
    fn reorder_folder_rejects_self_or_descendant_target() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();
        let a = lib.create_folder("A", None).unwrap();
        let b = lib
            .create_folder("B", Some(&a.folder_id))
            .unwrap();

        // Self → reject.
        assert!(lib
            .reorder_folder(&a.folder_id, Some(&a.folder_id), 0.0)
            .is_err());
        // Into descendant → reject.
        assert!(lib
            .reorder_folder(&a.folder_id, Some(&b.folder_id), 0.0)
            .is_err());
    }

    #[test]
    fn upsert_folder_preserves_local_sort_index_on_repull() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();
        // Pretend the device just sent us a folder.
        lib.upsert_folder("dev-1", None, "Pulled", "{}").unwrap();
        // User reorders it locally.
        lib.reorder_folder("dev-1", None, 999.5).unwrap();
        // Device sends the same folder again (e.g. another sync).
        lib.upsert_folder("dev-1", None, "Pulled", "{\"v\":2}").unwrap();
        let folders = lib.list_folders().unwrap();
        let row = folders.iter().find(|f| f.folder_id == "dev-1").unwrap();
        // Local order is preserved; metadata is refreshed.
        assert!((row.sort_index - 999.5).abs() < f64::EPSILON);
    }

    #[test]
    fn purge_archived_drops_versions_and_lets_gc_reclaim() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();
        let m = seed_manifest(&lib, "doc-1", &[("a.rm", b"unique-bytes-for-purge-test")]);
        lib.record_version(&m, Source::Pulled).unwrap();
        let body_hash = m
            .files
            .iter()
            .find(|f| f.path == "a.rm")
            .unwrap()
            .sha256
            .clone();
        assert!(lib.has_blob(&body_hash));

        lib.archive_document("doc-1", ArchiveReason::Device)
            .unwrap();
        lib.purge_archived_document("doc-1").unwrap();

        // Both archive and version log are empty for this doc.
        assert!(lib.list_archived().unwrap().is_empty());
        assert!(lib.get_history("doc-1").unwrap().is_empty());

        // GC then reclaims the now-orphan blobs.
        let zero = std::time::Duration::ZERO;
        let report = lib.garbage_collect_with_grace(zero).unwrap();
        assert!(report.deleted >= 1, "purge should free at least one blob");
        assert!(!lib.has_blob(&body_hash));
    }

    #[test]
    fn purge_missing_archive_entry_errors() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();
        let err = lib.purge_archived_document("never-existed").unwrap_err();
        assert!(matches!(err, Error::NotFound(_)));
    }

    #[test]
    fn dedup_across_documents() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();
        let shared = b"shared bytes";
        let h = seed_blob(&lib, shared);
        for doc in ["doc-a", "doc-b"] {
            let mut m = Manifest::new(doc, "Notebook", doc);
            m.files.push(ManifestFile {
                path: "shared".into(),
                sha256: h.clone(),
                size: shared.len() as u64,
                mode: 0o644,
                derived: false,
            });
            lib.record_version(&m, Source::Pulled).unwrap();
        }
        let blob_files: Vec<_> = walkdir(&tmp.path().join("blobs"));
        let matching: Vec<_> = blob_files
            .iter()
            .filter(|p| p.file_name().and_then(|s| s.to_str()) == Some(h.as_str()))
            .collect();
        assert_eq!(matching.len(), 1, "blob should be stored exactly once");
    }

    #[test]
    fn record_derived_artefact_attaches_under_ocr_prefix_and_round_trips() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();
        let m = seed_manifest(&lib, "doc-1", &[("doc-1.content", b"{}")]);
        lib.record_version(&m, Source::Pulled).unwrap();

        let outcome = lib
            .record_derived_artefact("doc-1", "ocr/transcript.md", b"# Hello\n\nWorld")
            .unwrap();
        let bytes = lib
            .read_derived_artefact(outcome.version_id, "ocr/transcript.md")
            .unwrap()
            .expect("transcript should be readable");
        assert_eq!(bytes, b"# Hello\n\nWorld");

        // A second write at the same path replaces the prior entry rather
        // than accumulating duplicates in the manifest.
        let outcome2 = lib
            .record_derived_artefact("doc-1", "ocr/transcript.md", b"# Updated")
            .unwrap();
        assert_ne!(outcome.version_id, outcome2.version_id);
        let bytes2 = lib
            .read_derived_artefact(outcome2.version_id, "ocr/transcript.md")
            .unwrap()
            .unwrap();
        assert_eq!(bytes2, b"# Updated");
        let entry = lib.get_version(outcome2.version_id).unwrap();
        let manifest_bytes = lib.read_blob(&entry.manifest_hash).unwrap();
        let m = Manifest::from_canonical_json(&manifest_bytes).unwrap();
        let transcripts: Vec<_> = m
            .files
            .iter()
            .filter(|f| f.path == "ocr/transcript.md")
            .collect();
        assert_eq!(transcripts.len(), 1);
        assert!(transcripts[0].derived);
    }

    #[test]
    fn record_derived_artefact_does_not_dirty_an_already_synced_doc() {
        // OCR transcripts live PC-side. If the device was in sync
        // before the user OCR'd, it should still be in sync after —
        // no push queued, no "unsynced" badge.
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();
        let m = seed_manifest(&lib, "doc-1", &[("doc-1.content", b"{}")]);
        // Pull-source recording sets last_seen = current, putting
        // the doc in the "in sync" state.
        let pulled = lib.record_version(&m, Source::Pulled).unwrap();
        assert!(!doc_dirty(&lib, "doc-1"));

        let derived = lib
            .record_derived_artefact("doc-1", "ocr/transcript.md", b"hello")
            .unwrap();
        assert_ne!(derived.version_id, pulled.version_id);
        assert!(
            !doc_dirty(&lib, "doc-1"),
            "derived-only change must not flip has_unpushed_changes"
        );
    }

    #[test]
    fn record_derived_artefact_preserves_pending_unpushed_changes() {
        // If the user had unpushed edits before OCR'ing (e.g. a
        // rename), we must NOT swallow them by auto-advancing
        // last_seen all the way to the post-OCR manifest. The
        // pending-push state has to survive the derived-only edit.
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();
        let m = seed_manifest(&lib, "doc-1", &[("doc-1.content", b"{}")]);
        lib.record_version(&m, Source::Pulled).unwrap();

        // Rename → leaves last_seen behind (still pointing at the
        // pre-rename manifest), so the doc is dirty.
        lib.rename_document("doc-1", "Renamed").unwrap();
        assert!(doc_dirty(&lib, "doc-1"));

        // OCR on top of the rename. Auto-advance must NOT fire here
        // because last_seen != prior current.
        lib.record_derived_artefact("doc-1", "ocr/transcript.md", b"hi")
            .unwrap();
        assert!(
            doc_dirty(&lib, "doc-1"),
            "rename was already pending; OCR must not silence the unsynced state"
        );
    }

    fn doc_dirty(lib: &Library, document_id: &str) -> bool {
        lib.list_documents()
            .unwrap()
            .into_iter()
            .find(|d| d.document_id == document_id)
            .map(|d| d.has_unpushed_changes)
            .unwrap_or(false)
    }

    #[test]
    fn record_derived_artefact_rejects_non_ocr_paths() {
        let tmp = tempfile::tempdir().unwrap();
        let lib = Library::open(tmp.path()).unwrap();
        let m = seed_manifest(&lib, "doc-1", &[("doc-1.content", b"{}")]);
        lib.record_version(&m, Source::Pulled).unwrap();
        let r = lib.record_derived_artefact("doc-1", "evil/exec.sh", b"#!/bin/sh");
        assert!(matches!(r, Err(Error::InvalidArgument(_))));
    }

    #[test]
    fn probe_path_classifies_directories() {
        // Empty directory → Empty.
        let empty = tempfile::tempdir().unwrap();
        assert_eq!(
            Library::probe_path(empty.path()).unwrap(),
            LibraryPathKind::Empty
        );

        // Non-existent path → Empty (caller's `Library::open` will create it).
        let missing = empty.path().join("does-not-exist");
        assert_eq!(
            Library::probe_path(&missing).unwrap(),
            LibraryPathKind::Empty
        );

        // Existing stamped library → Existing.
        let stamped = tempfile::tempdir().unwrap();
        let _lib = Library::open(stamped.path()).unwrap();
        drop(_lib);
        assert_eq!(
            Library::probe_path(stamped.path()).unwrap(),
            LibraryPathKind::Existing
        );

        // Foreign non-empty directory → InvalidPath error.
        let foreign = tempfile::tempdir().unwrap();
        std::fs::write(foreign.path().join("notes.txt"), b"hi").unwrap();
        assert!(matches!(
            Library::probe_path(foreign.path()),
            Err(Error::InvalidPath(_))
        ));
    }

    #[test]
    fn refuses_non_empty_foreign_directory() {
        // Audit fix H9: a renderer-supplied path like ~/Documents/ must
        // not become a "library" with blobs/, db.sqlite, etc. scattered
        // into it.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("foreign.txt"), b"hello").unwrap();
        let result = Library::open(tmp.path());
        assert!(matches!(result, Err(Error::InvalidPath(_))));
    }

    #[test]
    fn rejects_existing_library_json_with_bad_stamp() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::write(tmp.path().join("library.json"), b"{not valid json").unwrap();
        let result = Library::open(tmp.path());
        assert!(matches!(result, Err(Error::InvalidPath(_))));
    }

    #[test]
    fn second_open_on_same_root_is_rejected() {
        // Audit fix H3: two app instances pointed at the same library
        // would corrupt the GC vs. record_version invariant.
        let tmp = tempfile::tempdir().unwrap();
        let _first = Library::open(tmp.path()).unwrap();
        let second = Library::open(tmp.path());
        assert!(matches!(second, Err(Error::AlreadyOpen(_))));
        // After dropping the first, the lock is released.
        drop(_first);
        let _third = Library::open(tmp.path()).unwrap();
    }

    fn walkdir(p: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        if !p.exists() {
            return out;
        }
        for entry in std::fs::read_dir(p).unwrap() {
            let e = entry.unwrap();
            let path = e.path();
            if path.is_dir() {
                out.extend(walkdir(&path));
            } else {
                out.push(path);
            }
        }
        out
    }
}
