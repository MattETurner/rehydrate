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
    /// must write atomically per file (tmp + rename) and trigger whatever
    /// is needed for the device to pick up the changes (xochitl restart on
    /// the reMarkable).
    async fn put_document_tree(
        &self,
        uuid: &str,
        files: &[crate::model::RemoteFile],
    ) -> DeviceResult<()>;
}
