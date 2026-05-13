//! Push planning and execution.
//!
//! A push is the mirror of a pull: every document whose `current_manifest`
//! differs from `sync_state.last_seen_manifest` gets reconstructed from the
//! blob store and uploaded via `Device::put_document_tree`.
//!
//! Documents that have no `sync_state` row are *library-only* — typically
//! imports — and are skipped here because Phase 3 has no `delete_document`
//! semantics on the device side yet. Phase 4 will handle imports.

use rehydrate_core::{DocumentSummary, Library, Manifest};
use rehydrate_device::{Device, RemoteFile};
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

    // After document push, flush any folder operations. Renames /
    // reparents / creations upload the folder's `<uuid>.metadata`;
    // deletions hard-remove every `<uuid>*` artefact on the device
    // so xochitl drops the folder outright instead of moving it to
    // its Trash view (which is what `deleted: true` in metadata
    // would do). Either shape ends with `mark_folder_pushed` so the
    // local row is reconciled (cleared, or dropped for tombstones).
    //
    // Failures here are logged and counted as skips so a single
    // broken folder doesn't block the rest of the queue. Audit fix
    // M3: propagate DB errors instead of treating them as "no
    // pending folders" — silently skipping a folder op used to make
    // the user think the sync succeeded.
    let pending_folder_ops = library.list_pending_folder_pushes()?;
    for op in pending_folder_ops {
        if cancel.is_cancelled() {
            break;
        }
        let folder_id = op.folder_id().to_string();
        let push_result = match op {
            rehydrate_core::FolderPushOp::Upsert {
                ref folder_id,
                ref metadata_json,
            } => {
                let file = rehydrate_device::RemoteFile {
                    path: format!("{folder_id}.metadata"),
                    bytes: metadata_json.clone().into_bytes(),
                    mode: 0o644,
                };
                device.put_document_tree(folder_id, &[file]).await
            }
            rehydrate_core::FolderPushOp::Delete { ref folder_id } => {
                device.delete_document_tree(folder_id).await
            }
        };
        match push_result {
            Ok(()) => {
                // Audit fix M2: the mark_folder_pushed failure path
                // used to log-and-continue, so the same folder op
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

    // Refresh the tablet's document index once for the whole push
    // session. Per-document restarts would blank the UI for several
    // seconds each. A failure here means the files are safely on
    // the device but the tablet UI may keep showing the old state
    // until the user reboots — surface a warning event so the UI
    // can tell the user, but don't fail the sync (the next push
    // will retry the refresh).
    if pushed > 0 && !cancel.is_cancelled() {
        if let Err(e) = device.refresh_document_index().await {
            tracing::warn!(error = %e, "post-push index refresh failed");
            if let Some(p) = &progress {
                let _ = p
                    .send(ProgressEvent::Warning {
                        message: format!(
                            "Files uploaded successfully, but the tablet's document index \
                             didn't refresh. Reboot the tablet, or it will pick up the \
                             changes on the next sync. ({e})"
                        ),
                    })
                    .await;
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

    // Files are now on the device. We MUST advance last_seen_manifest
    // or the next push will treat this doc as outbound again and
    // re-upload — overwriting any device-side edit the user makes in
    // the interim. Transient SQLite errors (busy, locked) get a few
    // retries with backoff so we don't lose the device-side state
    // because of momentary contention. If every retry fails the
    // error propagates: the caller logs it and the next push will
    // re-run this branch, which is safe because both the device
    // write and the DB update are idempotent.
    let doc_id = &item.document.document_id;
    let manifest_hex = item.document.current_manifest.as_str();
    let mut last_err = None;
    for attempt in 0..5u32 {
        match library.update_last_seen_manifest(doc_id, manifest_hex) {
            Ok(()) => return Ok(()),
            Err(e) => {
                last_err = Some(e);
                // Exponential-ish backoff: 25, 50, 100, 200, 400 ms.
                let delay_ms = 25u64 << attempt;
                tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
            }
        }
    }
    Err(last_err.expect("loop ran at least once").into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rehydrate_core::manifest::ManifestFile;
    use rehydrate_core::{Manifest, Sha256Hex, Source};
    use rehydrate_device::fake::FakeDevice;

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
    async fn folder_delete_op_hard_removes_metadata_from_device() {
        // Regression guard for commit 24ea264. A folder delete must
        // route the push through Device::delete_document_tree
        // (sweeping every `<uuid>*` artefact off the device) rather
        // than uploading metadata with `deleted: true`. The latter
        // would only move the folder to xochitl's Trash view; the
        // user would still see it on the tablet until they emptied
        // trash there manually. The core test in rehydrate-core
        // verifies the queue enqueues Delete (not Upsert); this one
        // pins what actually happens on the wire.
        let lib_dir = tempfile::tempdir().unwrap();
        let dev_dir = tempfile::tempdir().unwrap();
        let lib = Library::open(lib_dir.path()).unwrap();
        let dev = FakeDevice::new(dev_dir.path());

        // Create + first push → metadata lands on the fake device.
        let folder = lib.create_folder("Journal", None).unwrap();
        let plan = plan_push(&lib).unwrap();
        execute_push(&lib, &dev, plan, None, Cancel::default())
            .await
            .unwrap();
        let metadata_path = dev_dir
            .path()
            .join(format!("{}.metadata", folder.folder_id));
        assert!(
            metadata_path.exists(),
            "precondition: folder metadata must be on device after first push",
        );
        // Stray sibling artefact under the same uuid prefix proves
        // the delete sweep takes everything, not just .metadata.
        let stray = dev_dir.path().join(format!("{}.stray", folder.folder_id));
        std::fs::write(&stray, b"garbage").unwrap();

        // Delete + second push.
        lib.delete_folder(&folder.folder_id).unwrap();
        let plan = plan_push(&lib).unwrap();
        execute_push(&lib, &dev, plan, None, Cancel::default())
            .await
            .unwrap();

        assert!(
            !metadata_path.exists(),
            "Delete op must hard-remove <uuid>.metadata from device, not upload deleted:true",
        );
        assert!(
            !stray.exists(),
            "Delete op must sweep every <uuid>* artefact, not just .metadata",
        );
        // The library row is gone too (mark_folder_pushed drops tombstones).
        assert!(lib.list_folders().unwrap().is_empty());
    }

    #[tokio::test]
    async fn folder_reparent_pushes_new_parent_in_metadata_payload() {
        // Regression guard for commit 49e935b. The core test in
        // rehydrate-core proves the queue payload carries the new
        // parent. This one closes the loop by reading what landed on
        // the device — a future refactor that silently stripped or
        // re-keyed the `parent` field between queue and
        // put_document_tree would slip past the queue-side test.
        let lib_dir = tempfile::tempdir().unwrap();
        let dev_dir = tempfile::tempdir().unwrap();
        let lib = Library::open(lib_dir.path()).unwrap();
        let dev = FakeDevice::new(dev_dir.path());

        let parent = lib.create_folder("Journal", None).unwrap();
        let child = lib.create_folder("BH", None).unwrap();
        let plan = plan_push(&lib).unwrap();
        execute_push(&lib, &dev, plan, None, Cancel::default())
            .await
            .unwrap();

        // Drag BH under Journal.
        lib.reorder_folder(&child.folder_id, Some(&parent.folder_id), 0.0)
            .unwrap();
        let plan = plan_push(&lib).unwrap();
        execute_push(&lib, &dev, plan, None, Cancel::default())
            .await
            .unwrap();

        let metadata_path = dev_dir.path().join(format!("{}.metadata", child.folder_id));
        let bytes = std::fs::read(&metadata_path).expect("child metadata on device");
        let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(
            v.get("parent").and_then(|x| x.as_str()),
            Some(parent.folder_id.as_str()),
            "device-side metadata must carry the new parent uuid",
        );
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
