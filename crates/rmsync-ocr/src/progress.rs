//! Progress event types streamed from OCR + model-download flows
//! through `tokio::mpsc` channels to the IPC layer's
//! `app.emit("ocr:*", ...)` calls.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OcrProgressEvent {
    /// Backend has started rendering / inference for a page.
    PageStarted { page_index: usize },
    /// One page produced N characters of transcript.
    PageDone { page_index: usize, chars: usize },
    /// One page failed; the run continues with the rest.
    PageFailed {
        page_index: usize,
        message: String,
    },
    /// Whole run finished (success or cancelled). Used to close out
    /// the renderer's "running" UI state cleanly.
    Done {
        pages_done: usize,
        total_chars: usize,
    },
    /// Model download is in flight. `total` may be `None` if the
    /// server didn't return a Content-Length.
    DownloadProgress { done: u64, total: Option<u64> },
    /// Model download finished; the runtime is ready to serve.
    DownloadDone,
}
