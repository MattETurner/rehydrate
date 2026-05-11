//! On-device OCR for reMarkable notebooks.
//!
//! The crate is the seam between (a) the device-side `.rm` files
//! reHydrate already has, rendered through `page_render`, and (b) a
//! user-provided Ollama daemon that runs the vision-language model.
//! Ollama is responsible for model lifecycle (download, quantisation,
//! GPU memory); this crate owns the network adapter (`http::RestrictedAgent`
//! pins to the configured host, refuses redirects, 30s timeouts), the
//! request shape, and the OcrBackend trait that lets tests substitute
//! a `Mock` impl.

pub mod backend;
pub mod http;
pub mod ollama;
pub mod page_render;
pub mod progress;

pub use backend::{Mock, OcrBackend, OcrCancel, OcrError, PageTranscript, TranscribeOptions};
pub use http::RestrictedAgent;
pub use ollama::OllamaBackend;
pub use page_render::render_rm_to_png;
pub use progress::OcrProgressEvent;

/// Default Ollama model identifier surfaced to the IPC layer and
/// pre-selected in the Settings modal's model dropdown. Qwen3-VL is
/// the current vision-language family on Ollama (released late
/// 2025): the 4B build at ~3.3 GB fits comfortably on 8 GB GPUs and
/// Apple-Silicon unified-memory budgets, and the upstream notes
/// call out improved handwriting recognition (now 32 languages, up
/// from 10) over the previous Qwen2.5-VL line.
pub fn default_model_id() -> &'static str {
    "qwen3-vl:4b"
}
