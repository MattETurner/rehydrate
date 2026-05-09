//! Pull planning.
//!
//! `plan_pull` enumerates the device and, for each top-level entry, decides
//! whether it's `New`, `Changed`, `Unchanged`, or `Skipped`. The decision is
//! based on the device-side mtime hint compared to the library's last-seen
//! mtime — fast and cheap. `execute` will re-verify by content hash before
//! recording a new version, so a wrong classification here only costs a
//! download, never a wrong version.

use rehydrate_core::Library;
use rehydrate_device::{Device, RemoteEntry};
use serde::{Deserialize, Serialize};

use crate::error::SyncResult;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum PlanItemStatus {
    New,
    Changed,
    Unchanged,
    Skipped,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DocumentPlan {
    pub entry: RemoteEntry,
    pub status: PlanItemStatus,
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PullPlan {
    pub items: Vec<DocumentPlan>,
}

impl PullPlan {
    pub fn to_transfer(&self) -> impl Iterator<Item = &DocumentPlan> {
        self.items
            .iter()
            .filter(|p| matches!(p.status, PlanItemStatus::New | PlanItemStatus::Changed))
    }
}

pub async fn plan_pull(library: &Library, device: &dyn Device) -> SyncResult<PullPlan> {
    let entries = device.list_documents().await?;
    let mut items = Vec::with_capacity(entries.len());
    for entry in entries {
        let (status, reason) = match entry.kind {
            rehydrate_device::RemoteEntryKind::Folder => (PlanItemStatus::Unchanged, None),
            rehydrate_device::RemoteEntryKind::Document => {
                // Archived documents stay archived even if the device still
                // has them. Restoring is an explicit user action; without
                // this guard a pull would silently reanimate the doc.
                if library.is_archived(&entry.uuid)? {
                    (
                        PlanItemStatus::Skipped,
                        Some("archived locally".to_string()),
                    )
                } else {
                    (classify(library, &entry)?, None)
                }
            }
        };
        items.push(DocumentPlan {
            entry,
            status,
            reason,
        });
    }
    Ok(PullPlan { items })
}

fn classify(library: &Library, entry: &RemoteEntry) -> SyncResult<PlanItemStatus> {
    let Some((seen_mtime, _seen_manifest)) = library.last_seen(&entry.uuid)? else {
        return Ok(PlanItemStatus::New);
    };
    match (seen_mtime, &entry.device_mtime_hint) {
        (Some(seen), Some(now)) if &seen == now => Ok(PlanItemStatus::Unchanged),
        _ => Ok(PlanItemStatus::Changed),
    }
}
