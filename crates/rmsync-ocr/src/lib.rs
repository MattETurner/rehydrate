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

pub use backend::{Mock, OcrBackend, OcrCancel, OcrError, PageTranscript, TranscribeOptions};
pub use model_store::{ModelDescriptor, ModelStatus, ModelStore};
pub use page_render::render_rm_to_png;
pub use progress::OcrProgressEvent;

/// The current default model — multilingual, 7B parameters, Q4_K_M
/// quant. Sized so it fits in 8 GB of memory on Apple Silicon and
/// produces high-quality multilingual handwriting transcription.
///
/// The descriptor is the canonical "what does the user need to
/// download for the recommended OCR experience" answer.
///
/// We point at `bartowski/`'s GGUF mirror rather than the upstream
/// `Qwen/` org repo. The Qwen org's GGUF repo is gated (HuggingFace
/// returns 401 for unauthenticated `resolve/main/<file>` requests)
/// — bartowski's repackage is a public, byte-identical Q4_K_M of the
/// same upstream weights. The SHA-256 below is the LFS hash from
/// HuggingFace's `X-Linked-Etag` header at the time this file was
/// authored; if upstream re-uploads, verification will fail loudly
/// rather than silently fetch new bytes.
pub fn default_model() -> ModelDescriptor {
    ModelDescriptor {
        id: "qwen2-vl-7b-instruct-q4-k-m".into(),
        display_name: "Qwen2-VL 7B Instruct (Q4_K_M)".into(),
        download_url:
            "https://huggingface.co/bartowski/Qwen2-VL-7B-Instruct-GGUF/resolve/main/Qwen2-VL-7B-Instruct-Q4_K_M.gguf"
                .into(),
        sha256: "30f199c2192fce1db0fbbbd484c7b2aa69ccce883890853f9807e1c837405a80"
            .into(),
        size_bytes: 4_683_072_672,
    }
}
