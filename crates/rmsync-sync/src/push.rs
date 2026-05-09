//! Push planning and execution.
//!
//! A push is the mirror of a pull: every document whose `current_manifest`
//! differs from `sync_state.last_seen_manifest` gets reconstructed from the
//! blob store and uploaded via `Device::put_document_tree`.
//!
//! Documents that have no `sync_state` row are *library-only* — typically
//! imports — and are skipped here because Phase 3 has no `delete_document`
//! semantics on the device side yet. Phase 4 will handle imports.

use rmsync_core::{DocumentSummary, Library, Manifest};
use rmsync_device::{Device, RemoteFile};
use serde::{Deserialize, Serialize};

use crate::error::{SyncError, SyncResult};
use crate::execute::Cancel;
use crate::progress::{Progress, ProgressEvent};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PushItemStatus {
    Outbound,
    Unchanged,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushItem {
    pub document: DocumentSummary,
    pub status: PushItemStatus,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PushPlan {
    pub items: Vec<PushItem>,
}

pub fn plan_push(library: &Library) -> SyncResult<PushPlan> {
    // Includes archived documents whose new (deleted=true) manifest still
    // needs to reach the device. After the push, last_seen_manifest gets
    // updated and they classify as Unchanged on the next plan_push.
    let docs = library.list_pushable_documents()?;
    let mut items = Vec::with_capacity(docs.len());
    for doc in docs {
        let last_seen = library.last_seen(&doc.document_id)?;
        let item = match last_seen.and_then(|(_, m)| m) {
            // Library and device manifest agree → nothing to push.
            Some(last_manifest) if last_manifest == doc.current_manifest.as_str() => PushItem {
                document: doc,
                status: PushItemStatus::Unchanged,
                reason: None,
            },
            // Library has a different manifest (restored or updated) — push.
            // OR: no last_seen_manifest at all, meaning the document was
            // imported and has never been synced. Either way it's outbound.
            // The device-side put_document_tree call creates new files just
            // as readily as it overwrites existing ones.
            _ => PushItem {
                document: doc,
                status: PushItemStatus::Outbound,
                reason: None,
            },
        };
        items.push(item);
    }
    Ok(PushPlan { items })
}

#[derive(Debug, Clone)]
pub struct PushReport {
    pub pushed: usize,
    pub unchanged: usize,
    pub skipped: usize,
}

pub async fn execute_push(
    library: &Library,
    device: &dyn Device,
    plan: PushPlan,
    progress: Option<Progress>,
    cancel: Cancel,
) -> SyncResult<PushReport> {
    let total = plan.items.len();
    if let Some(p) = &progress {
        let _ = p
            .send(ProgressEvent::PlanReady {
                total_documents: total,
            })
            .await;
    }

    let mut pushed = 0usize;
    let mut unchanged = 0usize;
    let mut skipped = 0usize;

    for item in plan.items {
        if cancel.is_cancelled() {
            if let Some(p) = &progress {
                let _ = p.send(ProgressEvent::Cancelled).await;
            }
            return Err(SyncError::Cancelled);
        }
        match item.status {
            PushItemStatus::Unchanged => {
                unchanged += 1;
                continue;
            }
            PushItemStatus::Skipped => {
                skipped += 1;
                if let Some(p) = &progress {
                    let _ = p
                        .send(ProgressEvent::DocumentSkipped {
                            document_id: item.document.document_id.clone(),
                            reason: item.reason.clone().unwrap_or_default(),
                        })
                        .await;
                }
                continue;
            }
            PushItemStatus::Outbound => {}
        }

        if let Some(p) = &progress {
            let _ = p
                .send(ProgressEvent::DocumentStarted {
                    document_id: item.document.document_id.clone(),
                    visible_name: item.document.visible_name.clone(),
                })
                .await;
        }

        match push_one(library, device, &item, progress.as_ref()).await {
            Ok(()) => {
                if let Some(p) = &progress {
                    let _ = p
                        .send(ProgressEvent::DocumentCompleted {
                            document_id: item.document.document_id.clone(),
                            unchanged: false,
                        })
                        .await;
                }
                pushed += 1;
            }
            Err(e) => {
                tracing::warn!(uuid = %item.document.document_id, error = %e, "push skipped");
                skipped += 1;
                if let Some(p) = &progress {
                    let _ = p
                        .send(ProgressEvent::DocumentSkipped {
                            document_id: item.document.document_id.clone(),
                            reason: e.to_string(),
                        })
                        .await;
                }
            }
        }
    }

    // After document push, flush any folder renames. A folder push is
    // a single `<folder_id>.metadata` file uploaded via the same
    // put_document_tree primitive — the tablet's xochitl picks up the
    // rename when it next refreshes its file index. Failures here are
    // logged and counted as skips so a single broken folder doesn't
    // block the rest of the queue.
    // Audit fix M3: propagate DB errors instead of treating them as
    // "no pending folders" — silently skipping a folder rename used
    // to make the user think the sync succeeded.
    let pending_folders = library.list_pending_folder_pushes()?;
    for (folder_id, metadata_json) in pending_folders {
        if cancel.is_cancelled() {
            break;
        }
        let file = rmsync_device::RemoteFile {
            path: format!("{folder_id}.metadata"),
            bytes: metadata_json.into_bytes(),
            mode: 0o644,
        };
        match device.put_document_tree(&folder_id, &[file]).await {
            Ok(()) => {
                // Audit fix M2: the mark_folder_pushed failure path
                // used to log-and-continue, so the same folder rename
                // re-pushed forever. Count it as skipped instead so
                // the user sees a non-zero skip count and the loop
                // doesn't claim success.
                if let Err(e) = library.mark_folder_pushed(&folder_id) {
                    tracing::warn!(
                        folder = %folder_id,
                        error = %e,
                        "could not clear folder pending_push (will retry next sync)"
                    );
                    skipped += 1;
                } else {
                    pushed += 1;
                }
            }
            Err(e) => {
                tracing::warn!(folder = %folder_id, error = %e, "folder push failed");
                skipped += 1;
            }
        }
    }

    if let Some(p) = &progress {
        let _ = p
            .send(ProgressEvent::Done {
                recorded: pushed,
                unchanged,
                skipped,
            })
            .await;
    }
    Ok(PushReport {
        pushed,
        unchanged,
        skipped,
    })
}

async fn push_one(
    library: &Library,
    device: &dyn Device,
    item: &PushItem,
    progress: Option<&Progress>,
) -> SyncResult<()> {
    let manifest_bytes = library.read_blob(&item.document.current_manifest)?;
    let manifest = Manifest::from_canonical_json(&manifest_bytes)?;

    let mut files = Vec::with_capacity(manifest.files.len());
    for f in &manifest.files {
        // Skip library-side derived artefacts (OCR transcripts, etc.).
        // They live in the manifest so they version + restore + GC
        // cleanly, but they don't belong on the tablet's xochitl
        // file index.
        if f.derived {
            continue;
        }
        let bytes = library.read_blob(&f.sha256)?;
        let size = bytes.len() as u64;
        if let Some(p) = progress {
            let _ = p
                .send(ProgressEvent::FileFetched {
                    document_id: item.document.document_id.clone(),
                    file: f.path.clone(),
                    bytes: size,
                    deduped: false,
                })
                .await;
        }
        files.push(RemoteFile {
            path: f.path.clone(),
            bytes,
            mode: f.mode,
        });
    }

    device
        .put_document_tree(&item.document.document_id, &files)
        .await?;

    library.update_last_seen_manifest(
        &item.document.document_id,
        item.document.current_manifest.as_str(),
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmsync_core::manifest::ManifestFile;
    use rmsync_core::{Manifest, Sha256Hex, Source};
    use rmsync_device::fake::FakeDevice;

    fn seed_doc(lib: &Library, doc_id: &str, name: &str, files: &[(&str, &[u8])]) {
        let mut m = Manifest::new(doc_id, "Notebook", name);
        for (path, bytes) in files {
            let res = lib.put_blob(bytes).unwrap();
            m.files.push(ManifestFile {
                path: (*path).to_string(),
                sha256: res.hash,
                size: res.size,
                mode: 0o644,
                derived: false,
            });
        }
        lib.record_version(&m, Source::Pulled).unwrap();
        // Record sync_state so plan_push doesn't classify as Skipped.
        lib.update_last_seen_manifest(
            doc_id,
            Sha256Hex::from_bytes(&m.canonical_json().unwrap()).as_str(),
        )
        .unwrap();
    }

    #[tokio::test]
    async fn push_uploads_changed_documents() {
        let lib_dir = tempfile::tempdir().unwrap();
        let dev_dir = tempfile::tempdir().unwrap();
        let lib = Library::open(lib_dir.path()).unwrap();
        let dev = FakeDevice::new(dev_dir.path());

        seed_doc(
            &lib,
            "doc-1",
            "Doc One",
            &[("doc-1.metadata", b"{}"), ("doc-1.content", b"{}")],
        );

        // First plan: nothing to push — last_seen matches current.
        let plan = plan_push(&lib).unwrap();
        assert!(plan
            .items
            .iter()
            .all(|i| i.status == PushItemStatus::Unchanged));

        // Restore (no-op since same manifest)... so simulate a change by
        // recording a new version on top.
        let mut m = Manifest::new("doc-1", "Notebook", "Doc One renamed");
        let res = lib.put_blob(b"{\"renamed\":true}").unwrap();
        m.files.push(ManifestFile {
            path: "doc-1.metadata".into(),
            sha256: res.hash,
            size: res.size,
            mode: 0o644,
            derived: false,
        });
        let res2 = lib.put_blob(b"{}").unwrap();
        m.files.push(ManifestFile {
            path: "doc-1.content".into(),
            sha256: res2.hash,
            size: res2.size,
            mode: 0o644,
            derived: false,
        });
        lib.record_version(&m, Source::Restored).unwrap();

        let plan = plan_push(&lib).unwrap();
        let outbound = plan
            .items
            .iter()
            .filter(|i| i.status == PushItemStatus::Outbound)
            .count();
        assert_eq!(outbound, 1);

        let report = execute_push(&lib, &dev, plan, None, Cancel::default())
            .await
            .unwrap();
        assert_eq!(report.pushed, 1);

        // The fake device now has the renamed metadata.
        let on_device = std::fs::read(dev_dir.path().join("doc-1.metadata")).unwrap();
        assert_eq!(on_device, b"{\"renamed\":true}");

        // Re-planning shows nothing outbound.
        let plan = plan_push(&lib).unwrap();
        assert!(plan
            .items
            .iter()
            .all(|i| i.status == PushItemStatus::Unchanged));
    }

    #[tokio::test]
    async fn derived_files_are_not_pushed_to_device() {
        // Library-side artefacts (OCR transcripts, future caches)
        // travel with the manifest for versioning + restore but must
        // never end up on the tablet's xochitl file index.
        let lib_dir = tempfile::tempdir().unwrap();
        let dev_dir = tempfile::tempdir().unwrap();
        let lib = Library::open(lib_dir.path()).unwrap();
        let dev = FakeDevice::new(dev_dir.path());

        seed_doc(
            &lib,
            "doc-1",
            "Doc One",
            &[("doc-1.metadata", b"{}"), ("doc-1.content", b"{}")],
        );
        lib.record_derived_artefact("doc-1", "ocr/transcript.md", b"# Hi")
            .unwrap();

        let plan = plan_push(&lib).unwrap();
        execute_push(&lib, &dev, plan, None, Cancel::default())
            .await
            .unwrap();

        // The derived file must not be present on the device.
        let on_device = dev_dir.path().join("ocr").join("transcript.md");
        assert!(!on_device.exists(), "derived file leaked to device");
    }
}
