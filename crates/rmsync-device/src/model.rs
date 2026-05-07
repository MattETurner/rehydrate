use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceInfo {
    pub model: String,
    pub serial: Option<String>,
    pub software_version: Option<String>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RemoteEntryKind {
    Folder,
    Document,
}

/// One top-level entry on the device — a folder or a document — as exposed
/// by `Device::list_documents`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RemoteEntry {
    pub uuid: String,
    pub visible_name: String,
    pub doc_type: String,
    pub parent: Option<String>,
    pub kind: RemoteEntryKind,
    /// Device-side mtime as a hint for change detection. Authoritative
    /// comparison is always by content hash; this is just a fast filter.
    pub device_mtime_hint: Option<String>,
    /// Free-form additional metadata captured verbatim from the device.
    #[serde(default)]
    pub metadata: serde_json::Value,
}

/// One file inside a document's tree, returned by
/// `Device::fetch_document_tree`. The `bytes` payload is held in memory for
/// Phase 1 simplicity; Phase 1 documents are small enough that this is fine.
/// A streaming variant will be added when push and large-PDF imports land.
#[derive(Debug, Clone)]
pub struct RemoteFile {
    pub path: String,
    pub bytes: Vec<u8>,
    pub mode: u32,
}
