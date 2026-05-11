//! Progress event types streamed from the OCR pipeline through
//! `tokio::mpsc` channels to the IPC layer's `app.emit("ocr:*", ...)`
//! calls.
//!
//! The Ollama pivot removed `DownloadProgress` / `ModelLoading` /
//! `DownloadDone` — Ollama runs out-of-process and handles its own
//! model lifecycle, so the app no longer reports those phases.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OcrProgressEvent {
    /// Backend has started rendering / inference for a page.
    PageStarted { page_index: usize },
    /// One page produced N characters of transcript.
    PageDone { page_index: usize, chars: usize },
    /// One page failed; the run continues with the rest.
    PageFailed { page_index: usize, message: String },
    /// Whole run finished (success or cancelled). Used to close out
    /// the renderer's "running" UI state cleanly.
    Done {
        pages_done: usize,
        total_chars: usize,
    },
}
