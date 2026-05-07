use std::fmt;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// Hex-lowercased SHA-256. The canonical content address used everywhere in
/// the library (blob filenames, manifest references, version identifiers).
#[derive(Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Sha256Hex(String);

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
        write!(f, "sha256:{}", &self.0[..12])
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
