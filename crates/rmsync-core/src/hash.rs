use std::fmt;

use serde::{de, Deserialize, Deserializer, Serialize};
use sha2::{Digest, Sha256};

/// Hex-lowercased SHA-256. The canonical content address used everywhere in
/// the library (blob filenames, manifest references, version identifiers).
///
/// Deserialization is strict: any string that isn't exactly 64 lowercase
/// hex characters is rejected. The previous derived `Deserialize` (via
/// `#[serde(transparent)]`) accepted uppercase, short, or non-hex input;
/// callers that did `[..12]` on the inner string could then panic, and
/// the blob fanout silently missed on disk.
#[derive(Clone, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct Sha256Hex(String);

impl<'de> Deserialize<'de> for Sha256Hex {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let s = String::deserialize(deserializer)?;
        Self::from_hex(&s).ok_or_else(|| {
            de::Error::custom(format!(
                "invalid sha256 hex (expected 64 lowercase hex chars, got {})",
                s.len()
            ))
        })
    }
}

impl Sha256Hex {
    pub fn from_bytes(bytes: &[u8]) -> Self {
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        Sha256Hex(hex::encode(hasher.finalize()))
    }

    pub fn from_hex(s: impl Into<String>) -> Option<Self> {
        let s = s.into();
        if s.len() == 64
            && s.chars()
                .all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase())
        {
            Some(Sha256Hex(s))
        } else {
            None
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Two-hex-char fanout used for sharding the blob store: e.g. `ab/cd`.
    pub fn fanout(&self) -> (&str, &str) {
        (&self.0[0..2], &self.0[2..4])
    }
}

impl fmt::Debug for Sha256Hex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        // Defensive: from_hex enforces 64 chars, but Debug must never panic.
        let prefix = self.0.get(..12).unwrap_or(self.0.as_str());
        write!(f, "sha256:{prefix}")
    }
}

impl fmt::Display for Sha256Hex {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

pub struct StreamingHasher {
    hasher: Sha256,
    written: u64,
}

impl StreamingHasher {
    pub fn new() -> Self {
        Self {
            hasher: Sha256::new(),
            written: 0,
        }
    }

    pub fn update(&mut self, chunk: &[u8]) {
        self.hasher.update(chunk);
        self.written += chunk.len() as u64;
    }

    pub fn finish(self) -> (Sha256Hex, u64) {
        (Sha256Hex(hex::encode(self.hasher.finalize())), self.written)
    }
}

impl Default for StreamingHasher {
    fn default() -> Self {
        Self::new()
    }
}
