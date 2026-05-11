//! Backend trait. The IPC layer holds an `Arc<dyn OcrBackend>` and
//! doesn't care whether it's mistral.rs, llama-cpp-2, or the
//! in-process `Mock` used by tests.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use tokio::sync::mpsc;

use crate::progress::OcrProgressEvent;

/// What the caller asks the backend to do per stroke-image.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct TranscribeOptions {
    /// Optional language hint (BCP-47 like "en", "de", "ja"). Modern
    /// VLMs autodetect, but the hint helps when handwriting is
    /// short / ambiguous between scripts.
    pub language: Option<String>,
    /// If `true`, the backend should output Markdown (headings,
    /// bullets) where it can infer structure. Otherwise plain text.
    pub markdown: bool,
}

/// One transcribed page, paired with the page index and any
/// detected language the model surfaced.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageTranscript {
    pub page_index: usize,
    pub text: String,
    pub detected_language: Option<String>,
}

#[derive(Debug, Error)]
pub enum OcrError {
    /// The Ollama daemon isn't reachable at the configured URL —
    /// either the user hasn't started it or the URL is wrong. The
    /// IPC layer maps this to the `ollama_unconfigured` tagged
    /// error so the UI can route the user to Settings → Ollama.
    #[error("ollama unreachable: {0}")]
    Unreachable(String),
    /// HTTP 404 from Ollama with a body that mentions a missing
    /// model. The user needs to `ollama pull <model>` (or pick a
    /// different model in settings) before retrying.
    #[error("model not pulled: {0}")]
    ModelNotPulled(String),
    /// Ollama returned an unexpected status or malformed response.
    /// Kept distinct from `Unreachable` because it usually means a
    /// real bug in the daemon (or a stale model) and not a config
    /// mistake the user can fix from the UI.
    #[error("ollama backend error: {0}")]
    Backend(String),
    /// Legacy: rendering or local pre-processing failed (PNG encode,
    /// image scaling, …). Retained because `page_render.rs` still
    /// returns it via the `Render` variant.
    #[error("page rendering failed: {0}")]
    Render(String),
    /// Generic inference-side error. Kept for backend impls that
    /// don't fit the more specific variants above (e.g. the `Mock`
    /// impl's negative-path tests).
    #[error("inference failed: {0}")]
    Inference(String),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("cancelled by user")]
    Cancelled,
}

/// Cooperative cancellation primitive shared with the OCR backend.
/// Cheap to clone and to poll. Lives here rather than in the
/// rehydrate-sync `Cancel` type because rehydrate-ocr can't depend on
/// rehydrate-sync (it'd be a dep cycle once IPC pulls them together).
#[derive(Debug, Clone, Default)]
pub struct OcrCancel(Arc<AtomicBool>);

impl OcrCancel {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::Release);
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[async_trait]
pub trait OcrBackend: Send + Sync {
    /// Stable name for the backend (e.g. `"mistral-rs/qwen2-vl-7b"`).
    /// Recorded in the transcript's frontmatter so future re-OCR
    /// can decide whether to invalidate.
    fn name(&self) -> &str;

    /// Transcribe a slice of page PNGs. Sends `OcrProgressEvent`s on
    /// `progress` if provided (page started / finished / failed).
    /// Honours `cancel` between pages.
    async fn transcribe_pages(
        &self,
        pages: Vec<Vec<u8>>,
        opts: &TranscribeOptions,
        progress: Option<mpsc::Sender<OcrProgressEvent>>,
        cancel: OcrCancel,
    ) -> Result<Vec<PageTranscript>, OcrError>;
}

/// Tiny in-process backend used by unit tests and as the default
/// "no model loaded" sentinel until the user downloads a real one.
pub struct Mock {
    pub canned: Vec<String>,
}

#[async_trait]
impl OcrBackend for Mock {
    fn name(&self) -> &str {
        "mock"
    }

    async fn transcribe_pages(
        &self,
        pages: Vec<Vec<u8>>,
        _opts: &TranscribeOptions,
        progress: Option<mpsc::Sender<OcrProgressEvent>>,
        cancel: OcrCancel,
    ) -> Result<Vec<PageTranscript>, OcrError> {
        let mut out = Vec::with_capacity(pages.len());
        for (i, _) in pages.iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(OcrError::Cancelled);
            }
            let text = self
                .canned
                .get(i)
                .cloned()
                .unwrap_or_else(|| format!("(mock OCR for page {i})"));
            if let Some(p) = &progress {
                let _ = p
                    .send(OcrProgressEvent::PageDone {
                        page_index: i,
                        chars: text.chars().count(),
                    })
                    .await;
            }
            out.push(PageTranscript {
                page_index: i,
                text,
                detected_language: None,
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mock_transcribes_pages_in_order() {
        let backend = Mock {
            canned: vec!["one".into(), "two".into()],
        };
        let pages = vec![vec![1u8, 2, 3], vec![4u8, 5, 6]];
        let result = backend
            .transcribe_pages(pages, &TranscribeOptions::default(), None, OcrCancel::new())
            .await
            .unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].text, "one");
        assert_eq!(result[1].text, "two");
    }

    #[tokio::test]
    async fn mock_honours_cancellation() {
        let backend = Mock {
            canned: vec!["one".into()],
        };
        let cancel = OcrCancel::new();
        cancel.cancel();
        let pages = vec![vec![1u8]];
        let result = backend
            .transcribe_pages(pages, &TranscribeOptions::default(), None, cancel)
            .await;
        assert!(matches!(result, Err(OcrError::Cancelled)));
    }
}
