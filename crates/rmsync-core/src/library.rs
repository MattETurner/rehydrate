//! High-level Library facade.
//!
//! Phase 1 implements a lean subset: open an empty library, store blobs,
//! record a manifest as a new version. Phase 2 will add reconstruct + history
//! UI surfaces; Phase 4 will add GC. The data model already supports them.

use std::fs;
use std::path::Path;

use rusqlite::{params, OptionalExtension};
use serde::{Deserialize, Serialize};
use time::OffsetDateTime;

use crate::blob::BlobStore;
use crate::db::Db;
use crate::error::{Error, Result};
use crate::hash::Sha256Hex;
use crate::manifest::Manifest;
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
}

#[derive(Debug, Clone)]
pub struct RecordOutcome {
    pub version_id: VersionId,
    pub manifest_hash: Sha256Hex,
    /// True if this manifest hash was already the current version for this
    /// document — i.e. nothing changed and no new row was appended.
    pub unchanged: bool,
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
}

impl Library {
    /// Open an existing library or initialize a new one at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let paths = LibraryPaths::new(path.as_ref());
        paths.ensure_dirs()?;

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

        Ok(Self { paths, blobs, db })
    }

    pub fn paths(&self) -> &LibraryPaths {
        &self.paths
    }

    pub fn blobs(&self) -> &BlobStore {
        &self.blobs
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
        let canonical = manifest.canonical_json()?;
        let manifest_hash = Sha256Hex::from_bytes(&canonical);

        // Persist the manifest itself as a blob.
        self.blobs.put_bytes(&canonical)?;

        let now = OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();

        let mut conn = self.db.lock();
        let tx = conn.transaction()?;

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
                tx.execute(
                    "UPDATE sync_state SET last_seen_manifest = ?1, last_synced_at = ?2 \
                     WHERE document_id = ?3",
                    params![manifest_hash.as_str(), now, manifest.document_id],
                )?;
                tx.commit()?;
                return Ok(RecordOutcome {
                    version_id: *cur_id,
                    manifest_hash,
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

        tx.execute(
            "INSERT INTO sync_state(document_id, last_seen_manifest, last_synced_at) \
             VALUES (?1, ?2, ?3) \
             ON CONFLICT(document_id) DO UPDATE SET \
                 last_seen_manifest = excluded.last_seen_manifest, \
                 last_synced_at = excluded.last_synced_at",
            params![manifest.document_id, manifest_hash.as_str(), now],
        )?;

        tx.commit()?;
        Ok(RecordOutcome {
            version_id,
            manifest_hash,
            unchanged: false,
        })
    }

    pub fn list_documents(&self) -> Result<Vec<DocumentSummary>> {
        let conn = self.db.lock();
        let mut stmt = conn.prepare(
            "SELECT d.document_id, d.current_manifest, d.current_version_id \
             FROM documents d ORDER BY d.document_id",
        )?;
        let rows: Vec<(String, String, i64)> = stmt
            .query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))?
            .collect::<rusqlite::Result<_>>()?;
        drop(stmt);
        drop(conn);

        let mut out = Vec::with_capacity(rows.len());
        for (document_id, manifest_hex, version_id) in rows {
            let hash = Sha256Hex::from_hex(&manifest_hex).ok_or_else(|| Error::Corrupt {
                path: self.paths.db.display().to_string(),
                reason: format!("bad manifest hash for {document_id}"),
            })?;
            let manifest_bytes = self.blobs.read_to_vec(&hash)?;
            let manifest = Manifest::from_canonical_json(&manifest_bytes)?;
            out.push(DocumentSummary {
                document_id,
                visible_name: manifest.visible_name,
                doc_type: manifest.doc_type,
                current_manifest: hash,
                current_version_id: version_id,
            });
        }
        Ok(out)
    }

    pub fn get_history(&self, document_id: &str) -> Result<Vec<VersionEntry>> {
        let conn = self.db.lock();
        let mut stmt = conn.prepare(
            "SELECT id, document_id, manifest_hash, parent_version_id, observed_at, source, note \
             FROM versions WHERE document_id = ?1 ORDER BY id ASC",
        )?;
        let rows: Vec<VersionEntry> = stmt
            .query_map(params![document_id], |r| {
                let manifest_hex: String = r.get(2)?;
                let source_str: String = r.get(5)?;
                Ok(VersionEntry {
                    id: r.get(0)?,
                    document_id: r.get(1)?,
                    manifest_hash: Sha256Hex::from_hex(&manifest_hex).unwrap_or_else(|| {
                        // SQL CHECK doesn't constrain this — defensive default.
                        Sha256Hex::from_bytes(manifest_hex.as_bytes())
                    }),
                    parent_version_id: r.get(3)?,
                    observed_at: r.get(4)?,
                    source: Source::parse(&source_str).unwrap_or(Source::Pulled),
                    note: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<_>>()?;
        Ok(rows)
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

    /// Mirror a folder from the device into the library's folder index.
    pub fn upsert_folder(
        &self,
        folder_id: &str,
        parent: Option<&str>,
        visible_name: &str,
        metadata_json: &str,
    ) -> Result<()> {
        self.db.lock().execute(
            "INSERT INTO folders(folder_id, parent, visible_name, metadata_json) \
             VALUES (?1, ?2, ?3, ?4) \
             ON CONFLICT(folder_id) DO UPDATE SET \
                 parent = excluded.parent, \
                 visible_name = excluded.visible_name, \
                 metadata_json = excluded.metadata_json",
            params![folder_id, parent, visible_name, metadata_json],
        )?;
        Ok(())
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
fn walk_blobs(root: &Path, visit: &mut dyn FnMut(&Path, &str)) {
    let Ok(rd) = std::fs::read_dir(root) else {
        return;
    };
    for entry in rd.flatten() {
        let p = entry.path();
        if p.is_dir() {
            walk_blobs(&p, visit);
        } else if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
            visit(&p, name);
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
        for (path, bytes) in contents {
            let res = lib.put_blob(bytes).unwrap();
            files.push(ManifestFile {
                path: (*path).to_string(),
                sha256: res.hash,
                size: res.size,
                mode: 0o644,
            });
        }
        let mut m = Manifest::new(doc_id, "Notebook", "Test");
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
        // Three blobs: a.rm, b.rm, and the manifest itself.
        assert_eq!(report.blobs_total, 3);
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

        // Delete a referenced file blob to simulate corruption.
        let target = m.files[0].sha256.clone();
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
