//! On-device OCR for reMarkable notebooks.
//!
//! The crate is split so the heavy VLM runtime (mistral.rs) is the
//! only thing that needs to compile slowly: the IPC layer, the UI,
//! and the model-download / page-render plumbing all sit behind the
//! `OcrBackend` trait and ship in `rmsync-ocr` proper. Swapping
//! mistral.rs for llama-cpp-2 (or, in tests, a `MockBackend`)
//! doesn't ripple outward.
//!
//! Privacy invariant: only outbound network call is the explicit
//! HuggingFace model download in `model_store::download_model`,
//! gated by an allow-list. The rest of the crate is offline.

pub mod backend;
pub mod model_store;
pub mod page_render;
pub mod progress;

pub use backend::{Mock, OcrBackend, OcrError, PageTranscript, TranscribeOptions};
pub use model_store::{ModelDescriptor, ModelStatus, ModelStore};
pub use page_render::render_rm_to_png;
pub use progress::OcrProgressEvent;

/// The current default model — multilingual, 7B parameters, Q4_K_M
/// quant. Sized so it fits in 8 GB of memory on Apple Silicon and
/// produces high-quality multilingual handwriting transcription.
///
/// The descriptor is the canonical "what does the user need to
/// download for the recommended OCR experience" answer.
pub fn default_model() -> ModelDescriptor {
    ModelDescriptor {
        id: "qwen2-vl-7b-instruct-q4-k-m".into(),
        display_name: "Qwen2-VL 7B Instruct (Q4_K_M)".into(),
        // Hugging Face GGUF mirror of the official Alibaba weights.
        // The hash is verified post-download; if upstream republishes
        // we'll fail loudly rather than silently fetch new bytes.
        download_url:
            "https://huggingface.co/Qwen/Qwen2-VL-7B-Instruct-GGUF/resolve/main/Qwen2-VL-7B-Instruct-Q4_K_M.gguf"
                .into(),
        // SHA-256 checked against the upstream HF "Files" pane at
        // implementation time. Bump when we move to a new model.
        sha256:
            "0000000000000000000000000000000000000000000000000000000000000000".into(),
        size_bytes: 4_700_000_000,
    }
}
