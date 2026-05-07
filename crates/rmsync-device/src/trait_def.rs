use async_trait::async_trait;

use crate::error::DeviceResult;
use crate::model::{DeviceInfo, RemoteEntry, RemoteFile};

/// Single seam between the app and the reMarkable tablet. Phase 1 surface.
/// Phase 3 will add `put_document_tree`, `delete_document`, `move_document`.
#[async_trait]
pub trait Device: Send + Sync {
    async fn ping(&self) -> DeviceResult<DeviceInfo>;

    async fn list_documents(&self) -> DeviceResult<Vec<RemoteEntry>>;

    async fn fetch_document_tree(&self, uuid: &str) -> DeviceResult<Vec<RemoteFile>>;
}
