use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rmsync_core::manifest::ManifestFile;
use rmsync_core::{Library, Manifest, Source};
use rmsync_device::Device;

use crate::error::{SyncError, SyncResult};
use crate::plan::{DocumentPlan, PlanItemStatus, PullPlan};
use crate::progress::{Progress, ProgressEvent};

/// Cancellation handle. The caller can flip this from any thread; the engine
/// checks between documents (the smallest unit of atomicity).
#[derive(Default, Clone)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone)]
pub struct SyncReport {
    pub recorded: usize,
    pub unchanged: usize,
    pub skipped: usize,
}

pub async fn execute_pull(
    library: &mut Library,
    device: &dyn Device,
    plan: PullPlan,
    progress: Option<Progress>,
    cancel: Cancel,
) -> SyncResult<SyncReport> {
    let total = plan.items.len();
    if let Some(p) = &progress {
        let _ = p
            .send(ProgressEvent::PlanReady {
                total_documents: total,
            })
            .await;
    }

    let mut recorded = 0usize;
    let mut unchanged = 0usize;
    let mut skipped = 0usize;

    for item in plan.items {
        if cancel.is_cancelled() {
            if let Some(p) = &progress {
                let _ = p.send(ProgressEvent::Cancelled).await;
            }
            return Err(SyncError::Cancelled);
        }

        // Folders are tracked but not "fetched"; Phase 1 stores them via
        // metadata only. Mirror device folders into the library db.
        if matches!(item.entry.kind, rmsync_device::RemoteEntryKind::Folder) {
            mirror_folder(library, &item)?;
            continue;
        }

        if matches!(item.status, PlanItemStatus::Unchanged) {
            unchanged += 1;
            continue;
        }
        if matches!(item.status, PlanItemStatus::Skipped) {
            skipped += 1;
            if let Some(p) = &progress {
                let _ = p
                    .send(ProgressEvent::DocumentSkipped {
                        document_id: item.entry.uuid.clone(),
                        reason: item.reason.clone().unwrap_or_default(),
                    })
                    .await;
            }
            continue;
        }

        if let Some(p) = &progress {
            let _ = p
                .send(ProgressEvent::DocumentStarted {
                    document_id: item.entry.uuid.clone(),
                    visible_name: item.entry.visible_name.clone(),
                })
                .await;
        }

        match fetch_and_record(library, device, &item, progress.as_ref()).await {
            Ok(was_unchanged) => {
                if let Some(p) = &progress {
                    let _ = p
                        .send(ProgressEvent::DocumentCompleted {
                            document_id: item.entry.uuid.clone(),
                            unchanged: was_unchanged,
                        })
                        .await;
                }
                if was_unchanged {
                    unchanged += 1;
                } else {
                    recorded += 1;
                }
            }
            Err(e) => {
                tracing::warn!(uuid = %item.entry.uuid, error = %e, "document skipped");
                skipped += 1;
                if let Some(p) = &progress {
                    let _ = p
                        .send(ProgressEvent::DocumentSkipped {
                            document_id: item.entry.uuid.clone(),
                            reason: e.to_string(),
                        })
                        .await;
                }
            }
        }
    }

    if let Some(p) = &progress {
        let _ = p
            .send(ProgressEvent::Done {
                recorded,
                unchanged,
                skipped,
            })
            .await;
    }
    Ok(SyncReport {
        recorded,
        unchanged,
        skipped,
    })
}

fn mirror_folder(library: &mut Library, item: &DocumentPlan) -> SyncResult<()> {
    let metadata_json =
        serde_json::to_string(&item.entry.metadata).unwrap_or_else(|_| "null".into());
    library.upsert_folder(
        &item.entry.uuid,
        item.entry.parent.as_deref(),
        &item.entry.visible_name,
        &metadata_json,
    )?;
    Ok(())
}

async fn fetch_and_record(
    library: &mut Library,
    device: &dyn Device,
    item: &DocumentPlan,
    progress: Option<&Progress>,
) -> SyncResult<bool> {
    let files = device.fetch_document_tree(&item.entry.uuid).await?;

    // Hash + store every file. Build the manifest as we go.
    let mut manifest = Manifest::new(
        &item.entry.uuid,
        &item.entry.doc_type,
        &item.entry.visible_name,
    );
    manifest.parent = item.entry.parent.clone();
    manifest.metadata = item.entry.metadata.clone();

    // The `.content` file, if present, becomes content_meta.
    if let Some(content_file) = files
        .iter()
        .find(|f| f.path == format!("{}.content", item.entry.uuid))
    {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&content_file.bytes) {
            manifest.content_meta = v;
        }
    }

    for f in &files {
        let res = library.put_blob(&f.bytes)?;
        manifest.files.push(ManifestFile {
            path: f.path.clone(),
            sha256: res.hash.clone(),
            size: res.size,
            mode: f.mode,
        });
        if let Some(p) = progress {
            let _ = p
                .send(ProgressEvent::FileFetched {
                    document_id: item.entry.uuid.clone(),
                    file: f.path.clone(),
                    bytes: res.size,
                    deduped: matches!(res.outcome, rmsync_core::blob::PutOutcome::Deduplicated),
                })
                .await;
        }
    }

    let outcome = library.record_version(&manifest, Source::Pulled)?;

    // Cache the device-side mtime hint for fast next-time classification.
    if let Some(hint) = &item.entry.device_mtime_hint {
        library.update_mtime_hint(&item.entry.uuid, hint)?;
    }

    Ok(outcome.unchanged)
}

#[cfg(test)]
mod tests {
    use super::*;
    use rmsync_device::fake::FakeDevice;
    use std::fs;

    fn seed_fake_doc(root: &std::path::Path, uuid: &str, visible_name: &str, page_bytes: &[u8]) {
        fs::write(
            root.join(format!("{uuid}.metadata")),
            format!(
                r#"{{"visibleName":"{visible_name}","type":"DocumentType","lastModified":"100"}}"#
            ),
        )
        .unwrap();
        fs::write(
            root.join(format!("{uuid}.content")),
            r#"{"fileType":"notebook","pageCount":1}"#,
        )
        .unwrap();
        fs::create_dir_all(root.join(uuid)).unwrap();
        fs::write(root.join(uuid).join("page-1.rm"), page_bytes).unwrap();
    }

    #[tokio::test]
    async fn clean_pull_records_versions_and_dedupes() {
        let dev_root = tempfile::tempdir().unwrap();
        let lib_root = tempfile::tempdir().unwrap();
        seed_fake_doc(dev_root.path(), "doc-a", "A", b"shared");
        seed_fake_doc(dev_root.path(), "doc-b", "B", b"shared");

        let mut lib = rmsync_core::Library::open(lib_root.path()).unwrap();
        let dev = FakeDevice::new(dev_root.path());

        let plan = crate::plan::plan_pull(&lib, &dev).await.unwrap();
        assert_eq!(plan.items.len(), 2);
        assert!(plan.items.iter().all(|p| p.status == PlanItemStatus::New));

        let report = execute_pull(&mut lib, &dev, plan, None, Cancel::default())
            .await
            .unwrap();
        assert_eq!(report.recorded, 2);
        assert_eq!(report.unchanged, 0);

        // Re-pull: now everything should be unchanged.
        let plan2 = crate::plan::plan_pull(&lib, &dev).await.unwrap();
        let report2 = execute_pull(&mut lib, &dev, plan2, None, Cancel::default())
            .await
            .unwrap();
        assert_eq!(report2.recorded, 0);
        assert_eq!(report2.unchanged, 2);

        // The shared "shared" page bytes should exist as a single blob.
        let lib_blobs = lib_root.path().join("blobs");
        let shared_hash = rmsync_core::Sha256Hex::from_bytes(b"shared");
        let mut count = 0;
        for entry in walkdir::walk(&lib_blobs) {
            if entry.file_name().and_then(|n| n.to_str()) == Some(shared_hash.as_str()) {
                count += 1;
            }
        }
        assert_eq!(count, 1, "shared page must be deduplicated");
    }

    /// Tiny dir walker used only by tests so we don't need a separate dev-dep.
    mod walkdir {
        use std::path::{Path, PathBuf};
        pub fn walk(p: &Path) -> Vec<PathBuf> {
            let mut out = Vec::new();
            if !p.exists() {
                return out;
            }
            for e in std::fs::read_dir(p).unwrap() {
                let path = e.unwrap().path();
                if path.is_dir() {
                    out.extend(walk(&path));
                } else {
                    out.push(path);
                }
            }
            out
        }
    }
}
