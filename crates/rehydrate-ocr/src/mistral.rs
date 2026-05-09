//! Embedded vision-LLM backend backed by mistral.rs.
//!
//! Compiled only when the `mistral` feature is enabled. Without
//! it, the crate ships only the `Mock` backend and builds in a
//! fraction of the time (mistral.rs pulls candle + Metal kernels
//! and adds 5–10 minutes to a cold build).
//!
//! Privacy: model weights are fetched by mistral.rs's bundled
//! `hf-hub` client, which only contacts `huggingface.co`. Triggered
//! exclusively by the user clicking "Download model" — never on
//! launch. The `no_egress` integration test waives the reqwest ban
//! for `rehydrate-ocr` because hf-hub depends on it; we still ban
//! it in the business-logic crates (core / device / sync).
//!
//! Quality: defaults to Qwen2.5-VL-7B-Instruct, the current SOTA
//! open multilingual handwriting OCR at the 7B size class. With
//! ISQ-Q4 (mistral.rs's in-situ 4-bit quant) it loads in ~5 GB of
//! RAM. Accepts any of the supported VLM IDs via
//! [`MistralRsBackend::load`] — Qwen2.5-VL-3B is the natural
//! drop-down for memory-constrained machines.

#![cfg(feature = "mistral")]

use std::sync::Arc;

use async_trait::async_trait;
use mistralrs::{IsqBits, Model, ModelBuilder, MultimodalMessages, TextMessageRole};
use tokio::sync::mpsc;

use crate::backend::{OcrBackend, OcrCancel, OcrError, PageTranscript, TranscribeOptions};
use crate::progress::OcrProgressEvent;

/// HuggingFace repo IDs for the supported VLMs. The default is the
/// 7B Qwen2.5-VL Instruct, which mistral.rs supports natively. The
/// `_3B` variant is an opt-in for users on RAM-constrained machines.
pub const DEFAULT_MODEL_ID: &str = "Qwen/Qwen2.5-VL-7B-Instruct";

/// Prompt handed to the VLM along with each page raster. Designed
/// for handwriting transcription — explicit about preserving
/// structure and not adding commentary, so the response is the
/// transcript and only the transcript.
const TRANSCRIBE_PROMPT_BASE: &str = "Transcribe the handwritten text in this image exactly as written. \
Preserve line breaks, paragraph structure, lists, and any visible headings. \
Output only the transcript text — no preamble, no explanations, no markdown code fences.";

pub struct MistralRsBackend {
    name: String,
    model: Arc<Model>,
}

impl MistralRsBackend {
    /// Build (and download, if necessary) a vision-LLM keyed by
    /// HuggingFace repo ID. The first call triggers a multi-GB
    /// download via `hf-hub`; subsequent calls hit the local cache
    /// at `~/.cache/huggingface/hub/`.
    pub async fn load(model_id: impl Into<String>) -> Result<Self, OcrError> {
        let id = model_id.into();
        tracing::info!(model = %id, "loading mistral.rs vision model");
        let model = ModelBuilder::new(&id)
            .with_auto_isq(IsqBits::Four)
            .with_logging()
            .build()
            .await
            .map_err(|e| OcrError::Inference(format!("model load failed: {e}")))?;
        tracing::info!(model = %id, "mistral.rs model ready");
        Ok(Self {
            name: format!("mistralrs/{id}"),
            model: Arc::new(model),
        })
    }
}

#[async_trait]
impl OcrBackend for MistralRsBackend {
    fn name(&self) -> &str {
        &self.name
    }

    async fn transcribe_pages(
        &self,
        pages: Vec<Vec<u8>>,
        opts: &TranscribeOptions,
        progress: Option<mpsc::Sender<OcrProgressEvent>>,
        cancel: OcrCancel,
    ) -> Result<Vec<PageTranscript>, OcrError> {
        let mut out = Vec::with_capacity(pages.len());
        let prompt = build_prompt(opts);

        for (idx, png_bytes) in pages.into_iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(OcrError::Cancelled);
            }
            if let Some(p) = &progress {
                let _ = p.send(OcrProgressEvent::PageStarted { page_index: idx }).await;
            }

            let image = image::load_from_memory(&png_bytes).map_err(|e| {
                OcrError::Render(format!("page {idx} not a decodable image: {e}"))
            })?;

            let messages = MultimodalMessages::new().add_image_message(
                TextMessageRole::User,
                prompt.clone(),
                vec![image],
            );

            let response = self
                .model
                .send_chat_request(messages)
                .await
                .map_err(|e| OcrError::Inference(format!("page {idx}: {e}")))?;

            let text = response
                .choices
                .first()
                .and_then(|c| c.message.content.clone())
                .unwrap_or_default()
                .trim()
                .to_string();

            if let Some(p) = &progress {
                let _ = p
                    .send(OcrProgressEvent::PageDone {
                        page_index: idx,
                        chars: text.chars().count(),
                    })
                    .await;
            }

            out.push(PageTranscript {
                page_index: idx,
                text,
                detected_language: None,
            });
        }

        if let Some(p) = &progress {
            let total_chars: usize = out.iter().map(|p| p.text.chars().count()).sum();
            let _ = p
                .send(OcrProgressEvent::Done {
                    pages_done: out.len(),
                    total_chars,
                })
                .await;
        }
        Ok(out)
    }
}

fn build_prompt(opts: &TranscribeOptions) -> String {
    let mut p = String::from(TRANSCRIBE_PROMPT_BASE);
    if let Some(lang) = opts.language.as_deref() {
        if !lang.trim().is_empty() {
            p.push_str(&format!(
                "\n\nThe text is primarily in {lang}; recognise that script accordingly."
            ));
        }
    }
    if opts.markdown {
        p.push_str(
            "\n\nWhere the original handwriting clearly indicates structure (titles, bullet \
             lists, indented quotes), output it as Markdown. Otherwise output plain prose.",
        );
    }
    p
}
