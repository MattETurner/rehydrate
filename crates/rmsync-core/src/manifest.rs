//! Document manifests.
//!
//! A `Manifest` is a snapshot of one document at one point in time: the names
//! and content hashes of every file in the document's tree, plus the device-
//! side metadata as it stood. The hash of the manifest's canonical JSON is
//! what identifies a "version" in the version log.
//!
//! Canonicalization rules:
//! - `files` is sorted by `path`.
//! - JSON is emitted with sorted object keys at every level.
//! - No trailing newline. UTF-8.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::hash::Sha256Hex;

pub const MANIFEST_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Manifest {
    pub schema: u32,
    pub document_id: String,
    pub doc_type: String,
    pub visible_name: String,
    #[serde(default)]
    pub parent: Option<String>,
    #[serde(default)]
    pub metadata: Value,
    #[serde(default)]
    pub content_meta: Value,
    pub files: Vec<ManifestFile>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ManifestFile {
    pub path: String,
    pub sha256: Sha256Hex,
    pub size: u64,
    pub mode: u32,
}

impl Manifest {
    pub fn new(
        document_id: impl Into<String>,
        doc_type: impl Into<String>,
        visible_name: impl Into<String>,
    ) -> Self {
        Self {
            schema: MANIFEST_SCHEMA,
            document_id: document_id.into(),
            doc_type: doc_type.into(),
            visible_name: visible_name.into(),
            parent: None,
            metadata: Value::Null,
            content_meta: Value::Null,
            files: Vec::new(),
        }
    }

    /// Serialize to canonical JSON: sorted file list, sorted object keys at
    /// every level. The hash of the returned bytes is the version identifier.
    pub fn canonical_json(&self) -> Result<Vec<u8>> {
        let mut clone = self.clone();
        clone.files.sort_by(|a, b| a.path.cmp(&b.path));
        // Round-trip through serde_json::Value to enforce sorted keys.
        let v: Value = serde_json::to_value(&clone)?;
        let canonical = canonicalize_value(v);
        Ok(serde_json::to_vec(&canonical)?)
    }

    pub fn hash(&self) -> Result<Sha256Hex> {
        Ok(Sha256Hex::from_bytes(&self.canonical_json()?))
    }

    pub fn from_canonical_json(bytes: &[u8]) -> Result<Self> {
        Ok(serde_json::from_slice(bytes)?)
    }
}

/// Recursively reorder a `serde_json::Value` so that all maps have keys in
/// lexicographic order. `serde_json` with the `preserve_order` feature retains
/// insertion order of maps, so we explicitly sort here.
fn canonicalize_value(v: Value) -> Value {
    match v {
        Value::Object(map) => {
            let mut entries: Vec<(String, Value)> = map.into_iter().collect();
            entries.sort_by(|a, b| a.0.cmp(&b.0));
            let mut out = serde_json::Map::with_capacity(entries.len());
            for (k, v) in entries {
                out.insert(k, canonicalize_value(v));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.into_iter().map(canonicalize_value).collect()),
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(b: &[u8]) -> Sha256Hex {
        Sha256Hex::from_bytes(b)
    }

    #[test]
    fn canonical_is_stable_across_file_order() {
        let mut a = Manifest::new("uuid", "Notebook", "Test");
        a.files = vec![
            ManifestFile {
                path: "b".into(),
                sha256: h(b"b"),
                size: 1,
                mode: 0o644,
            },
            ManifestFile {
                path: "a".into(),
                sha256: h(b"a"),
                size: 1,
                mode: 0o644,
            },
        ];
        let mut b = a.clone();
        b.files.reverse();

        assert_eq!(a.canonical_json().unwrap(), b.canonical_json().unwrap());
        assert_eq!(a.hash().unwrap(), b.hash().unwrap());
    }

    #[test]
    fn roundtrip_via_canonical() {
        let m = Manifest {
            schema: MANIFEST_SCHEMA,
            document_id: "doc-uuid".into(),
            doc_type: "DocumentType.Pdf".into(),
            visible_name: "Hello".into(),
            parent: Some("folder-uuid".into()),
            metadata: serde_json::json!({"deleted": false, "lastModified": "1700000000000"}),
            content_meta: serde_json::json!({"pageCount": 12}),
            files: vec![ManifestFile {
                path: "doc.pdf".into(),
                sha256: h(b"pdf"),
                size: 1024,
                mode: 0o644,
            }],
        };
        let bytes = m.canonical_json().unwrap();
        let parsed = Manifest::from_canonical_json(&bytes).unwrap();
        assert_eq!(parsed, m);
        // Re-canonicalizing the parsed manifest yields the same bytes.
        assert_eq!(parsed.canonical_json().unwrap(), bytes);
    }
}
