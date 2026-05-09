//! On-device OCR for reMarkable notebooks.
//!
//! The heavy VLM runtime is feature-gated: `mistral` opts in to
//! `mistralrs` + Metal kernels via `mistral.rs`, which adds 5–10
//! minutes to a cold workspace build. Without the feature the
//! crate ships only the backend trait and the `Mock` impl —
//! enough for unit tests and dev iteration on the rest of the
//! workspace.
//!
//! Privacy invariant: outbound HTTP only fires on user-explicit
//! action. The mistral.rs path triggers HuggingFace downloads via
//! `hf-hub`; the legacy `model_store` path uses ureq against the
//! same allow-list. No network on launch.

pub mod backend;
#[cfg(feature = "mistral")]
pub mod mistral;
pub mod model_store;
pub mod page_render;
pub mod progress;

pub use backend::{Mock, OcrBackend, OcrCancel, OcrError, PageTranscript, TranscribeOptions};
#[cfg(feature = "mistral")]
pub use mistral::{MistralRsBackend, DEFAULT_MODEL_ID};
pub use model_store::{ModelDescriptor, ModelStatus, ModelStore};
pub use page_render::render_rm_to_png;
pub use progress::OcrProgressEvent;

/// Public default model identifier surfaced to the IPC layer. With
/// the `mistral` feature this is the HuggingFace repo ID for
/// Qwen2.5-VL-7B-Instruct, which mistral.rs downloads via hf-hub.
/// Without the feature this is a stable string the UI can show
/// while the runtime itself is unavailable.
pub fn default_model_id() -> &'static str {
    #[cfg(feature = "mistral")]
    {
        DEFAULT_MODEL_ID
    }
    #[cfg(not(feature = "mistral"))]
    {
        "Qwen/Qwen2.5-VL-7B-Instruct"
    }
}

/// Approximate weights size in bytes for the default model — used
/// only to size progress bars in the UI before any download
/// reports an actual Content-Length.
pub fn default_model_size_hint() -> u64 {
    // Qwen2.5-VL-7B-Instruct safetensors total ≈ 16 GB. ISQ-Q4
    // quant happens at load time, so the user pays the full
    // download cost once.
    16_000_000_000
}
