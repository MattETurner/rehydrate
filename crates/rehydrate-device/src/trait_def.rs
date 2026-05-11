use async_trait::async_trait;

use crate::error::DeviceResult;
use crate::model::{DeviceInfo, RemoteEntry, RemoteFile};

/// Single seam between the app and the reMarkable tablet. Phase 1
/// established the read-only surface; Phase 3 added `put_document_tree`.
/// `delete_document` and `move_document` are deferred to Phase 4.
#[async_trait]
pub trait Device: Send + Sync {
    async fn ping(&self) -> DeviceResult<DeviceInfo>;

    async fn list_documents(&self) -> DeviceResult<Vec<RemoteEntry>>;

    async fn fetch_document_tree(&self, uuid: &str) -> DeviceResult<Vec<RemoteFile>>;

    /// Upload a document's full file tree, replacing whatever is there.
    /// `files[].path` is the relative path within the xochitl directory
    /// (e.g. `"<uuid>.metadata"` or `"<uuid>/page-1.rm"`). Implementations
    /// must write atomically per file (tmp + rename). The files land on
    /// disk on the device but the document index may still be stale —
    /// call `refresh_document_index` after a batch of uploads.
    async fn put_document_tree(
        &self,
        uuid: &str,
        files: &[crate::model::RemoteFile],
    ) -> DeviceResult<()>;

    /// Force the device to re-read its document index so freshly-pushed
    /// changes become visible without a manual reboot. On the
    /// reMarkable that's a `systemctl restart xochitl`; the fake
    /// device is a no-op. Called once per push session — multiple
    /// per-document restarts would be both slow and disruptive (each
    /// restart blanks the tablet for a few seconds).
    ///
    /// Returning `Err` means the files are safely on the device but the
    /// tablet UI won't reflect them until the user reboots or until a
    /// subsequent refresh succeeds. The push engine treats this as a
    /// soft failure: the sync is still reported as a success but a
    /// warning event flows through the progress channel.
    async fn refresh_document_index(&self) -> DeviceResult<()> {
        Ok(())
    }
}
