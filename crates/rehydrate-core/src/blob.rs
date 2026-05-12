//! Content-addressed blob store.
//!
//! Blobs are stored under `<library>/blobs/<aa>/<bb>/<sha256>` where `<aa>` and
//! `<bb>` are the first two pairs of hex characters of the sha256 — a 2-byte
//! fanout. Writes go through `<library>/tmp/` and are atomically renamed into
//! place. Blobs are immutable; a duplicate `put` is a no-op.

use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::PathBuf;

use crate::error::{CoreError, Result};
use crate::hash::{Sha256Hex, StreamingHasher};
use crate::paths::LibraryPaths;

pub struct BlobStore {
    paths: LibraryPaths,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PutOutcome {
    /// The blob was newly written.
    Stored,
    /// The blob was already present; the caller's input was discarded.
    Deduplicated,
}

#[derive(Debug, Clone)]
pub struct PutResult {
    pub hash: Sha256Hex,
    pub size: u64,
    pub outcome: PutOutcome,
}

impl BlobStore {
    pub fn new(paths: LibraryPaths) -> Self {
        Self { paths }
    }

    pub fn paths(&self) -> &LibraryPaths {
        &self.paths
    }

    pub fn has(&self, hash: &Sha256Hex) -> bool {
        self.paths.blob_path(hash).exists()
    }

    /// Store bytes into the blob store. Returns the hash, size, and whether
    /// the blob was newly written or deduplicated against an existing entry.
    pub fn put_bytes(&self, bytes: &[u8]) -> Result<PutResult> {
        self.put_reader(&mut std::io::Cursor::new(bytes))
    }

    /// Stream a reader into the blob store. The reader is consumed exactly
    /// once; on the dedup path we still consume it to avoid surprising the
    /// caller (they passed us bytes; we hashed them).
    pub fn put_reader<R: Read>(&self, reader: &mut R) -> Result<PutResult> {
        // Stage to <tmp>/<random>; we don't yet know the hash, so we can't
        // place it directly under blobs/.
        fs::create_dir_all(&self.paths.tmp)?;
        let staging = tempfile::NamedTempFile::new_in(&self.paths.tmp)?;
        let (staging_file, staging_path) = staging.into_parts();
        let mut writer = io::BufWriter::new(staging_file);
        let mut hasher = StreamingHasher::new();

        let mut buf = [0u8; 64 * 1024];
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            writer.write_all(&buf[..n])?;
        }
        writer.flush()?;
        let inner = writer
            .into_inner()
            .map_err(|e| CoreError::Io(io::Error::other(e.to_string())))?;
        inner.sync_all()?;
        drop(inner);

        let (hash, size) = hasher.finish();
        let final_path = self.paths.blob_path(&hash);

        if final_path.exists() {
            // Already present; discard staged copy.
            let _ = fs::remove_file(&staging_path);
            return Ok(PutResult {
                hash,
                size,
                outcome: PutOutcome::Deduplicated,
            });
        }

        if let Some(parent) = final_path.parent() {
            fs::create_dir_all(parent)?;
        }

        // Atomic rename. On the same filesystem this is atomic on all
        // platforms we support. tempfile's NamedTempFile drop would unlink
        // the path, so we keep the PathBuf and call rename ourselves.
        match fs::rename(&staging_path, &final_path) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                // Race: another writer landed first. Discard our copy.
                let _ = fs::remove_file(&staging_path);
                return Ok(PutResult {
                    hash,
                    size,
                    outcome: PutOutcome::Deduplicated,
                });
            }
            Err(e) => {
                let _ = fs::remove_file(&staging_path);
                return Err(e.into());
            }
        }

        // Best-effort fsync of the directory so the rename hits the disk.
        if let Some(parent) = final_path.parent() {
            if let Ok(dir) = File::open(parent) {
                let _ = dir.sync_all();
            }
        }

        Ok(PutResult {
            hash,
            size,
            outcome: PutOutcome::Stored,
        })
    }

    pub fn open(&self, hash: &Sha256Hex) -> Result<File> {
        let p = self.paths.blob_path(hash);
        File::open(&p).map_err(|e| match e.kind() {
            io::ErrorKind::NotFound => CoreError::MissingBlob(hash.to_string()),
            _ => CoreError::Io(e),
        })
    }

    pub fn read_to_vec(&self, hash: &Sha256Hex) -> Result<Vec<u8>> {
        let mut f = self.open(hash)?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        Ok(buf)
    }

    pub fn path_for(&self, hash: &Sha256Hex) -> PathBuf {
        self.paths.blob_path(hash)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn store(tmp: &tempfile::TempDir) -> BlobStore {
        let paths = LibraryPaths::new(tmp.path());
        paths.ensure_dirs().unwrap();
        BlobStore::new(paths)
    }

    #[test]
    fn put_get_roundtrip() {
        let tmp = tempfile::tempdir().unwrap();
        let bs = store(&tmp);
        let r = bs.put_bytes(b"hello world").unwrap();
        assert_eq!(r.outcome, PutOutcome::Stored);
        assert_eq!(r.size, 11);
        assert!(bs.has(&r.hash));
        assert_eq!(bs.read_to_vec(&r.hash).unwrap(), b"hello world");
    }

    #[test]
    fn put_is_idempotent() {
        let tmp = tempfile::tempdir().unwrap();
        let bs = store(&tmp);
        let a = bs.put_bytes(b"same").unwrap();
        let b = bs.put_bytes(b"same").unwrap();
        assert_eq!(a.hash, b.hash);
        assert_eq!(b.outcome, PutOutcome::Deduplicated);
    }

    #[test]
    fn distinct_inputs_distinct_hashes() {
        let tmp = tempfile::tempdir().unwrap();
        let bs = store(&tmp);
        let a = bs.put_bytes(b"alpha").unwrap();
        let b = bs.put_bytes(b"beta").unwrap();
        assert_ne!(a.hash, b.hash);
    }
}
