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
    /// Library-side derived artefact (OCR transcript, future cache
    /// products) that should travel with the manifest but never get
    /// pushed to the device. Skipped from canonical JSON when
    /// `false` so existing manifests roundtrip byte-for-byte.
    #[serde(default, skip_serializing_if = "is_false")]
    pub derived: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
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
        let m: Self = serde_json::from_slice(bytes)?;
        m.validate_paths()?;
        Ok(m)
    }

    /// Reject any `files[].path` that could escape its destination
    /// directory when used by `Library::reconstruct` or the document
    /// opener: absolute paths, parent-directory components, NUL bytes,
    /// Windows drive letters, and backslashes (since paths are stored
    /// in posix form). Manifests come from device blobs that we don't
    /// otherwise validate; this is the choke point.
    pub fn validate_paths(&self) -> Result<()> {
        for f in &self.files {
            let p = &f.path;
            if p.is_empty() {
                return Err(crate::Error::Corrupt {
                    path: "<manifest>".into(),
                    reason: "empty file path".into(),
                });
            }
            if p.contains('\0') || p.contains('\\') {
                return Err(crate::Error::Corrupt {
                    path: "<manifest>".into(),
                    reason: format!("file path contains forbidden char: {p:?}"),
                });
            }
            if p.starts_with('/') || p.starts_with('~') {
                return Err(crate::Error::Corrupt {
                    path: "<manifest>".into(),
                    reason: format!("absolute file path not allowed: {p:?}"),
                });
            }
            // Windows drive letter check (e.g. "C:\\..." or "C:/...").
            if p.len() >= 2 && p.as_bytes().get(1) == Some(&b':') {
                return Err(crate::Error::Corrupt {
                    path: "<manifest>".into(),
                    reason: format!("drive-prefixed path not allowed: {p:?}"),
                });
            }
            for component in p.split('/') {
                if component == ".." {
                    return Err(crate::Error::Corrupt {
                        path: "<manifest>".into(),
                        reason: format!("parent-directory traversal in path: {p:?}"),
                    });
                }
            }
        }
        Ok(())
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
                derived: false,
            },
            ManifestFile {
                path: "a".into(),
                sha256: h(b"a"),
                size: 1,
                mode: 0o644,
                derived: false,
            },
        ];
        let mut b = a.clone();
        b.files.reverse();

        assert_eq!(a.canonical_json().unwrap(), b.canonical_json().unwrap());
        assert_eq!(a.hash().unwrap(), b.hash().unwrap());
    }

    #[test]
    fn derived_field_is_skipped_when_false_for_canonical_stability() {
        // A manifest written before the `derived` field existed must
        // hash identically after being loaded and re-serialized — the
        // canonical JSON is the version identifier, so any drift here
        // is silent data corruption.
        let pre_field_json = br#"{"content_meta":null,"doc_type":"Notebook","document_id":"u","files":[{"mode":420,"path":"a","sha256":"0000000000000000000000000000000000000000000000000000000000000000","size":1}],"metadata":null,"parent":null,"schema":1,"visible_name":"X"}"#;
        let m = Manifest::from_canonical_json(pre_field_json).unwrap();
        assert!(!m.files[0].derived);
        let canonical = m.canonical_json().unwrap();
        assert_eq!(
            canonical, pre_field_json,
            "canonical JSON drifted: derived: false must be skipped"
        );
    }

    #[test]
    fn derived_files_roundtrip() {
        // Files are sorted by path during canonicalization, so build
        // the manifest pre-sorted to make the equality check trivial.
        let m = Manifest {
            schema: MANIFEST_SCHEMA,
            document_id: "u".into(),
            doc_type: "Notebook".into(),
            visible_name: "T".into(),
            parent: None,
            metadata: Value::Null,
            content_meta: Value::Null,
            files: vec![
                ManifestFile {
                    path: "ocr/transcript.md".into(),
                    sha256: h(b"transcript"),
                    size: 1,
                    mode: 0o644,
                    derived: true,
                },
                ManifestFile {
                    path: "page.rm".into(),
                    sha256: h(b"page"),
                    size: 1,
                    mode: 0o644,
                    derived: false,
                },
            ],
        };
        let bytes = m.canonical_json().unwrap();
        let parsed = Manifest::from_canonical_json(&bytes).unwrap();
        assert_eq!(parsed, m);
        let transcript = parsed.files.iter().find(|f| f.derived).unwrap();
        assert_eq!(transcript.path, "ocr/transcript.md");
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
                derived: false,
            }],
        };
        let bytes = m.canonical_json().unwrap();
        let parsed = Manifest::from_canonical_json(&bytes).unwrap();
        assert_eq!(parsed, m);
        // Re-canonicalizing the parsed manifest yields the same bytes.
        assert_eq!(parsed.canonical_json().unwrap(), bytes);
    }
}
