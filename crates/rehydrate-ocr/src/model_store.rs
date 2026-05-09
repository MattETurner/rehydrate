//! On-disk model store. Tracks which weights are present, downloads
//! new ones from a hard-coded host allow-list (HuggingFace + its
//! mirror), and SHA-256 verifies the result before promoting the
//! file from `<name>.gguf.partial` to `<name>.gguf`.
//!
//! The download path is the only network egress in this crate. The
//! `no_egress` integration test asserts that the allow-list is the
//! single point of truth so a refactor can't sneak in a new
//! destination.

use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

use crate::progress::OcrProgressEvent;

/// Hosts the model store will fetch weights from. Anything else is
/// rejected before connect, so a corrupted descriptor can't aim us
/// at attacker-controlled storage. Lower-cased host comparison.
pub const ALLOWED_DOWNLOAD_HOSTS: &[&str] = &["huggingface.co", "hf-mirror.com"];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDescriptor {
    /// Stable directory-safe identifier, e.g. `"qwen2-vl-7b-q4-k-m"`.
    pub id: String,
    /// Display name for the UI.
    pub display_name: String,
    /// HTTPS URL on an allow-listed host. Validated before connect.
    pub download_url: String,
    /// Expected SHA-256 of the downloaded bytes. The download is
    /// rejected if it doesn't match — protects against partial
    /// transfers and source-side rewrites.
    pub sha256: String,
    /// Approximate file size, used by the UI to size progress bars
    /// before the server's Content-Length lands.
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ModelStatus {
    /// File is missing — user must trigger a download.
    Missing,
    /// Partial download present (resume on next call).
    Partial { bytes_done: u64 },
    /// File is present and SHA-256 verified at the time it landed.
    /// We don't re-verify on every status check; do that lazily
    /// when the backend actually loads the weights.
    Ready { path: PathBuf, size: u64 },
}

#[derive(Debug, thiserror::Error)]
pub enum ModelStoreError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("download URL host {0:?} is not in the allow-list")]
    DisallowedHost(String),
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("hash mismatch (expected {expected}, got {got})")]
    HashMismatch { expected: String, got: String },
    #[error("network: {0}")]
    Network(String),
    #[error("server returned HTTP {0}")]
    HttpStatus(u16),
}

pub struct ModelStore {
    root: PathBuf,
}

impl ModelStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    fn dir_for(&self, descriptor: &ModelDescriptor) -> PathBuf {
        self.root.join(&descriptor.id)
    }

    fn final_path(&self, descriptor: &ModelDescriptor) -> PathBuf {
        let file_name = filename_from_url(&descriptor.download_url)
            .unwrap_or_else(|| format!("{}.gguf", descriptor.id));
        self.dir_for(descriptor).join(file_name)
    }

    fn partial_path(&self, descriptor: &ModelDescriptor) -> PathBuf {
        let mut p = self.final_path(descriptor);
        let name = p
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("download.gguf")
            .to_string();
        p.set_file_name(format!("{name}.partial"));
        p
    }

    pub fn status(&self, descriptor: &ModelDescriptor) -> ModelStatus {
        let final_p = self.final_path(descriptor);
        if let Ok(meta) = fs::metadata(&final_p) {
            return ModelStatus::Ready {
                path: final_p,
                size: meta.len(),
            };
        }
        let partial = self.partial_path(descriptor);
        if let Ok(meta) = fs::metadata(&partial) {
            return ModelStatus::Partial {
                bytes_done: meta.len(),
            };
        }
        ModelStatus::Missing
    }

    /// Synchronous (uses `ureq`) download with SHA-256 verification.
    /// Run from a `spawn_blocking` so the IPC thread isn't tied up.
    /// Sends `DownloadProgress` events on `progress` if provided.
    pub fn download(
        &self,
        descriptor: &ModelDescriptor,
        progress: Option<mpsc::Sender<OcrProgressEvent>>,
    ) -> Result<PathBuf, ModelStoreError> {
        // Verify host allow-list before opening a connection.
        let host = host_of(&descriptor.download_url)
            .ok_or_else(|| ModelStoreError::InvalidUrl(descriptor.download_url.clone()))?;
        if !ALLOWED_DOWNLOAD_HOSTS.contains(&host.as_str()) {
            return Err(ModelStoreError::DisallowedHost(host));
        }

        fs::create_dir_all(self.dir_for(descriptor))?;
        let partial = self.partial_path(descriptor);
        let final_p = self.final_path(descriptor);

        // Resume support: range-request from the partial size if any.
        let already = fs::metadata(&partial).map(|m| m.len()).unwrap_or(0);
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(30))
            .timeout_read(Duration::from_secs(30 * 60)) // GGUFs are large
            .redirects(5) // HF's CDN does redirect to a presigned S3 URL
            .build();
        let mut req = agent.get(&descriptor.download_url);
        if already > 0 {
            req = req.set("Range", &format!("bytes={already}-"));
        }
        let resp = req.call().map_err(|e| match e {
            ureq::Error::Status(code, _) => ModelStoreError::HttpStatus(code),
            ureq::Error::Transport(t) => ModelStoreError::Network(t.to_string()),
        })?;

        // Total = partial-already + remaining-Content-Length, when known.
        let remaining: Option<u64> = resp
            .header("Content-Length")
            .and_then(|s| s.parse::<u64>().ok());
        let total = remaining.map(|r| already + r);

        let mut out = OpenOptions::new()
            .create(true)
            .append(already > 0)
            .write(true)
            .open(&partial)?;

        // Stream into the partial file while updating the rolling hash.
        let mut hasher = Sha256::new();
        // Re-hash the bytes already on disk so the rolling SHA reflects
        // the full body, not just the resume tail.
        if already > 0 {
            let mut existing = File::open(&partial)?;
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = existing.read(&mut buf)?;
                if n == 0 {
                    break;
                }
                hasher.update(&buf[..n]);
            }
        }

        let mut reader = resp.into_reader();
        let mut buf = [0u8; 64 * 1024];
        let mut done = already;
        loop {
            let n = reader.read(&mut buf)?;
            if n == 0 {
                break;
            }
            hasher.update(&buf[..n]);
            out.write_all(&buf[..n])?;
            done += n as u64;
            if let Some(p) = &progress {
                // Drop send errors silently — receiver might be gone.
                let _ = p
                    .blocking_send(OcrProgressEvent::DownloadProgress { done, total });
            }
        }
        out.flush()?;
        drop(out);

        // Verify against the descriptor's expected hash.
        let got = hex::encode(hasher.finalize());
        if got != descriptor.sha256 {
            // Drop the bad partial: leaving it around means the next
            // retry resumes from those bytes and the rolling hash can
            // never converge on the expected value, so the user is
            // permanently stuck. A fresh download is what they need.
            let _ = fs::remove_file(&partial);
            return Err(ModelStoreError::HashMismatch {
                expected: descriptor.sha256.clone(),
                got,
            });
        }
        fs::rename(&partial, &final_p)?;
        if let Some(p) = &progress {
            let _ = p.blocking_send(OcrProgressEvent::DownloadDone);
        }
        Ok(final_p)
    }
}

fn host_of(url_str: &str) -> Option<String> {
    let parsed = url::Url::parse(url_str).ok()?;
    parsed.host_str().map(|h| h.to_lowercase())
}

fn filename_from_url(url_str: &str) -> Option<String> {
    let parsed = url::Url::parse(url_str).ok()?;
    parsed
        .path_segments()
        .and_then(|mut s| s.next_back())
        .map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn descriptor() -> ModelDescriptor {
        ModelDescriptor {
            id: "test-model".into(),
            display_name: "Test".into(),
            download_url: "https://huggingface.co/example/model.gguf".into(),
            sha256: "0000000000000000000000000000000000000000000000000000000000000000"
                .into(),
            size_bytes: 1024,
        }
    }

    #[test]
    fn fresh_status_is_missing() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf());
        match store.status(&descriptor()) {
            ModelStatus::Missing => (),
            other => panic!("expected Missing, got {other:?}"),
        }
    }

    #[test]
    fn partial_file_is_recognised() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf());
        let d = descriptor();
        let dir = store.dir_for(&d);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(store.partial_path(&d), b"abc").unwrap();
        match store.status(&d) {
            ModelStatus::Partial { bytes_done } => assert_eq!(bytes_done, 3),
            other => panic!("expected Partial, got {other:?}"),
        }
    }

    #[test]
    fn ready_file_is_recognised() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf());
        let d = descriptor();
        std::fs::create_dir_all(store.dir_for(&d)).unwrap();
        std::fs::write(store.final_path(&d), b"finished").unwrap();
        match store.status(&d) {
            ModelStatus::Ready { size, .. } => assert_eq!(size, 8),
            other => panic!("expected Ready, got {other:?}"),
        }
    }

    #[test]
    fn download_rejects_disallowed_host() {
        let tmp = tempfile::tempdir().unwrap();
        let store = ModelStore::new(tmp.path().to_path_buf());
        let mut d = descriptor();
        d.download_url = "https://evil.com/model.gguf".into();
        let r = store.download(&d, None);
        match r {
            Err(ModelStoreError::DisallowedHost(_)) => (),
            other => panic!("expected DisallowedHost, got {other:?}"),
        }
    }
}
