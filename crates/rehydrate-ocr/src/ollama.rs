//! Ollama OCR backend.
//!
//! Each `transcribe_pages` call iterates pages serially, POSTing the
//! PNG to `{base_url}/api/generate` with a short verbatim-transcription
//! prompt and reading the `response` field. Serial because Ollama is
//! single-GPU bound: launching N parallel requests just serialises at
//! the daemon and inflates per-page latency variance without any
//! throughput gain.
//!
//! The HTTP path goes through `crate::http::RestrictedAgent` so the
//! request always hits the host the user configured — never a redirect
//! target, never another machine. The `no_egress.rs` integration test
//! treats this crate as a sanctioned egress path; new files added here
//! must continue to use the agent rather than constructing `ureq`
//! directly.

use std::time::Duration;

use async_trait::async_trait;
use base64::{engine::general_purpose::STANDARD, Engine};
use serde_json::json;
use tokio::sync::mpsc;

use crate::backend::{
    OcrBackend, OcrCancel, OcrError, PageFailure, PageTranscript, TranscribeOptions,
    TranscribeReport,
};
use crate::http::{AgentError, RestrictedAgent};
use crate::progress::OcrProgressEvent;

/// Per-read timeout for `/api/generate`. ureq applies this to
/// every blocking `read()` on the response socket, so it caps:
///
/// 1. **Time to first byte** — covers the daemon's
///    `prompt_eval` phase. Measured empirically against
///    `qwen3.5:4b` on Apple Silicon with our actual render size
///    (1280-px long edge): the daemon stays silent for ~158s
///    while it tokenises the image, then flushes headers + the
///    first NDJSON line together. The earlier 120s ceiling was
///    *below* that, which is exactly the user-visible
///    "Error encountered in the status line: timed out reading
///    response" we kept seeing. Streaming the response body
///    doesn't help here — it's the request-to-headers gap that
///    blows past 120s, not the inter-token gap.
///
/// 2. **Inter-token gap** during generation. Once the daemon
///    starts streaming, NDJSON lines arrive every few hundred
///    ms even on CPU, so the budget is dominated by (1).
///
/// 600s gives a comfortable headroom for prompt_eval on a slow
/// Apple Silicon laptop or a CPU-only Ollama box (qwen3.5:9b
/// images can take ~5 min of prompt_eval) while still detecting
/// a genuinely wedged daemon in a bounded window. Anything
/// tighter starts shipping false-positive "Ollama unreachable"
/// errors to the user mid-page.
const GENERATE_TIMEOUT: Duration = Duration::from_secs(600);

/// `OcrBackend` impl that talks to a user-provided Ollama daemon.
pub struct OllamaBackend {
    agent: RestrictedAgent,
    base_url: String,
    model: String,
    name: String,
}

impl OllamaBackend {
    /// Build a backend pinned to the host of `base_url`. Fails fast
    /// if the URL doesn't parse, so the caller can return a clean
    /// `ollama_unconfigured` IPC error before any inference work.
    pub fn new(base_url: &str, model: &str) -> Result<Self, OcrError> {
        let agent = RestrictedAgent::for_base_with_timeout(base_url, GENERATE_TIMEOUT)
            .map_err(|e| OcrError::Unreachable(format!("{e}")))?;
        Ok(Self {
            agent,
            base_url: base_url.trim_end_matches('/').to_string(),
            model: model.to_string(),
            name: format!("ollama/{model}"),
        })
    }

    /// Compose the OCR prompt. The instruction is intentionally
    /// short — modern vision-language models follow brief
    /// instructions more reliably than long ones, and the prompt is
    /// already part of the per-page payload so brevity also keeps
    /// the request small.
    ///
    /// The `/no_think` directive at the end is a Qwen3 family
    /// convention: even when the request-level `"think": false`
    /// flag is silently ignored (older Ollama daemons, or
    /// third-party fine-tunes that don't honour it), the inline
    /// directive convinces the model to skip its chain-of-thought
    /// prefix. Harmless for models that don't recognise it.
    fn build_prompt(opts: &TranscribeOptions) -> String {
        let mut parts: Vec<String> = Vec::new();
        parts.push(
            "Transcribe the handwritten and printed text in this image verbatim.".to_string(),
        );
        if opts.markdown {
            parts.push(
                "Preserve structure with Markdown (headings, bullet lists) where it's clear from the layout."
                    .to_string(),
            );
        } else {
            parts.push("Return plain text only — no Markdown formatting.".to_string());
        }
        if let Some(lang) = &opts.language {
            parts.push(format!(
                "The document is primarily in {lang}; default to that script if a glyph is ambiguous."
            ));
        }
        // Anti-hallucination clause. Belt-and-suspenders for the
        // primary fix in `rm_page_has_ink`: blank pages are filtered
        // out by the renderer before they reach this prompt, but a
        // page with only faint or near-empty strokes could still
        // slip through. Without this line, qwen3.5:4b on a
        // blank-looking canvas confabulates plausible essay text
        // ("The Impact of Artificial Intelligence on Healthcare..."
        // was the user-reported example). Empirically, models in
        // the Qwen 3.5 family DO honour "return nothing" when it's
        // stated this explicitly.
        parts.push(
            "If the image is blank, near-blank, or contains no readable handwriting / print, \
             return an empty response — do not invent, summarise, or guess."
                .to_string(),
        );
        parts.push(
            "Output ONLY the transcription. No preamble, no commentary, no surrounding quotes."
                .to_string(),
        );
        parts.push("/no_think".to_string());
        parts.join(" ")
    }

    /// Single-page request. Synchronous (blocking) because ureq is
    /// blocking; callers run this inside `spawn_blocking`.
    ///
    /// Uses Ollama's NDJSON streaming response (`stream: true`):
    /// one JSON line per generation step, concatenated client-side.
    /// Streaming is necessary but not sufficient for the user-
    /// visible reliability story:
    ///
    /// * `stream: false` buffers the entire response so the
    ///   client can't even see HTTP headers until generation is
    ///   over — every page took a 10+ minute timeout on
    ///   `qwen3.5:9b` on CPU.
    /// * `stream: true` flushes headers *and* the first NDJSON
    ///   line as soon as the daemon has its first generated
    ///   token. That fixed the worst case but left a smaller
    ///   one: the `prompt_eval` phase (vision tokenisation) is
    ///   still buffered, so for a normal notebook page on CPU
    ///   the daemon stays silent for ~2-3 minutes before any
    ///   byte arrives. The 120s read timeout used to fire inside
    ///   that gap and surface as "Error encountered in the
    ///   status line: timed out reading response" — see the
    ///   `GENERATE_TIMEOUT` doc for the empirical numbers and
    ///   why 600s is the right ceiling.
    ///
    /// Net: stream the response, but size the timeout for the
    /// pre-stream silence, not for the inter-token gap.
    fn transcribe_one_blocking(&self, png_bytes: &[u8], prompt: &str) -> Result<String, OcrError> {
        // `think: false` disables the Qwen3 / Qwen3.5 family's
        // chain-of-thought trace. Without it the model is free to
        // emit `<think>…</think>` blocks before the actual answer,
        // which (a) wastes inference time — a 9B page that should
        // take 30 s spends a minute "thinking" first — and (b)
        // pollutes the transcript when the closing tag is missing
        // and the wrapper text leaks through. The flag is honoured
        // by Ollama 0.5+ for the Qwen3 family; older daemons
        // silently ignore unknown fields, so it's safe to send
        // unconditionally. As a second line of defence we strip
        // any `<think>…</think>` block from the response below.
        let payload = json!({
            "model": self.model,
            "prompt": prompt,
            "images": [STANDARD.encode(png_bytes)],
            "stream": true,
            "think": false,
            "options": { "temperature": 0.1 },
        });
        let url = format!("{}/api/generate", self.base_url);

        let mut full = String::new();
        let mut error_body: Option<String> = None;
        let mut saw_done = false;
        let status = self
            .agent
            .post_json_streaming(&url, &[], &payload, |line| {
                // Non-200 responses arrive as a single buffered line
                // (the shared agent calls the callback once with the
                // body, then returns the status code). We can't tell
                // status from inside the callback without changing
                // the agent's signature, so we always try to parse
                // each line — failures stash the raw body for the
                // post-call status branch to surface.
                let v: serde_json::Value = match serde_json::from_str(line) {
                    Ok(v) => v,
                    Err(_) => {
                        // Not JSON — probably an error body from a
                        // non-2xx. Remember it; the status check
                        // below decides whether to surface it.
                        if error_body.is_none() {
                            error_body = Some(line.to_string());
                        }
                        return Ok(());
                    }
                };
                if let Some(chunk) = v.get("response").and_then(|s| s.as_str()) {
                    full.push_str(chunk);
                }
                // The daemon emits `{"done": true, ...}` as its
                // final line. Capture that so we can distinguish
                // "stream completed cleanly" from "connection died
                // mid-generation" — the latter is a network issue
                // we want to surface rather than silently truncate.
                if v.get("done").and_then(|d| d.as_bool()) == Some(true) {
                    saw_done = true;
                }
                Ok(())
            })
            .map_err(map_agent_error)?;

        match status {
            200 if saw_done => Ok(strip_thinking(&full)),
            200 => Err(OcrError::Backend(
                "ollama stream ended before the model signalled \
                 `done: true` — the connection likely dropped \
                 mid-generation. Try again."
                    .into(),
            )),
            // A 404 from `/api/generate` is overwhelmingly "model not
            // pulled" — Ollama always serves the endpoint itself, so
            // there's no realistic "route not found" miss here. The
            // previous heuristic of grepping the body for "model"
            // matched too narrowly (missed daemons that responded
            // with "manifest not found" wording) and risked silent
            // misclassification when wording shifted between
            // versions.
            404 => Err(OcrError::ModelNotPulled(format!(
                "ollama doesn't have `{}` pulled — run `ollama pull {}` (or pick a different \
                 model in Settings)",
                self.model, self.model
            ))),
            500..=599 => Err(OcrError::Backend(format!(
                "ollama returned HTTP {}: {}",
                status,
                truncate(&error_body.unwrap_or_default(), 240)
            ))),
            other => Err(OcrError::Backend(format!(
                "ollama returned HTTP {other}: {}",
                truncate(&error_body.unwrap_or_default(), 240)
            ))),
        }
    }
}

fn map_agent_error(err: AgentError) -> OcrError {
    match err {
        AgentError::Network(msg) => OcrError::Unreachable(msg),
        AgentError::InvalidUrl(msg) | AgentError::UnsafeUrl(msg) => OcrError::Backend(msg),
        // Cross-host can only happen if the agent's host changed
        // mid-flight (or someone bypassed `OllamaBackend::new`).
        // Treat as a config bug rather than a transient network
        // problem so retries don't paper over it.
        AgentError::CrossHost { found, expected } => {
            OcrError::Backend(format!("agent host mismatch: {found} != {expected}"))
        }
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        let mut t = s[..max].to_string();
        t.push('…');
        t
    }
}

/// Strip any `<think>…</think>` blocks the Qwen3 family emits when
/// thinking-mode is on (or when the `think: false` request flag is
/// ignored by an older daemon). Handles three real-world shapes:
///
/// * closed block: `<think>…</think>` followed by the answer →
///   block removed, answer preserved;
/// * unclosed block: model started thinking and ran out of tokens
///   before closing the tag → drop everything from `<think>` on
///   (better an empty transcript than a transcript that's just the
///   reasoning trace);
/// * answer-only: no thinking block → identity (trimmed).
fn strip_thinking(raw: &str) -> String {
    // Some Qwen3-family fine-tunes emit `<Think>` / `<THINK>` /
    // `<think >` instead of the canonical lowercase tag, so the
    // match is case-insensitive on a lowercased shadow string while
    // we splice on the original to keep byte offsets aligned.
    let mut s = raw.to_string();
    loop {
        let lower = s.to_ascii_lowercase();
        let Some(open) = lower.find("<think>") else {
            break;
        };
        match lower[open..].find("</think>") {
            Some(close_rel) => {
                let close = open + close_rel + "</think>".len();
                s.replace_range(open..close, "");
            }
            None => {
                // Unclosed thinking block.
                s.truncate(open);
                break;
            }
        }
    }
    s.trim().to_string()
}

#[async_trait]
impl OcrBackend for OllamaBackend {
    fn name(&self) -> &str {
        &self.name
    }

    async fn transcribe_pages(
        &self,
        pages: Vec<Vec<u8>>,
        opts: &TranscribeOptions,
        progress: Option<mpsc::Sender<OcrProgressEvent>>,
        cancel: OcrCancel,
    ) -> Result<TranscribeReport, OcrError> {
        let prompt = Self::build_prompt(opts);
        let mut out = Vec::with_capacity(pages.len());
        let mut failures: Vec<PageFailure> = Vec::new();
        for (i, png) in pages.into_iter().enumerate() {
            if cancel.is_cancelled() {
                return Err(OcrError::Cancelled);
            }
            if let Some(p) = &progress {
                let _ = p
                    .send(OcrProgressEvent::PageStarted { page_index: i })
                    .await;
            }
            // The blocking ureq call would otherwise stall the
            // tokio runtime; offload to the blocking pool so other
            // app work (UI events, sync) keeps flowing. Cloning
            // `self.agent` (now `Clone` — see `http.rs`) preserves
            // ureq's connection pool across pages, so a 50-page
            // notebook reuses one TCP connection to Ollama instead
            // of rebuilding it per request.
            let backend = OllamaBackend {
                agent: self.agent.clone(),
                base_url: self.base_url.clone(),
                model: self.model.clone(),
                name: self.name.clone(),
            };
            let prompt_clone = prompt.clone();
            let res = tokio::task::spawn_blocking(move || {
                backend.transcribe_one_blocking(&png, &prompt_clone)
            })
            .await
            .map_err(|e| OcrError::Backend(format!("join: {e}")))?;
            match res {
                Ok(text) => {
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
                        detected_language: opts.language.clone(),
                    });
                }
                Err(e) => {
                    // Surface the per-page failure on the progress
                    // channel AND record it in `failures` so the
                    // caller can build an honest transcript that
                    // shows the user exactly where the gaps are.
                    // Pre-v1.0 we only sent the event and dropped
                    // the page — a wedged daemon could produce a
                    // 1-page transcript for a 200-page notebook
                    // with no signal at all in the output file.
                    let msg = format!("{e}");
                    if let Some(p) = &progress {
                        let _ = p
                            .send(OcrProgressEvent::PageFailed {
                                page_index: i,
                                message: msg.clone(),
                            })
                            .await;
                    }
                    // Unreachable / ModelNotPulled are fatal for
                    // the whole run — retry-per-page would just
                    // hammer a daemon that isn't going to start
                    // responding for another N pages.
                    if matches!(
                        &e,
                        OcrError::Unreachable(_)
                            | OcrError::ModelNotPulled(_)
                            | OcrError::Cancelled
                    ) {
                        return Err(e);
                    }
                    failures.push(PageFailure {
                        page_index: i,
                        message: msg,
                    });
                }
            }
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
        Ok(TranscribeReport {
            pages: out,
            failures,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prompt_includes_markdown_hint_when_requested() {
        let p = OllamaBackend::build_prompt(&TranscribeOptions {
            language: None,
            markdown: true,
        });
        assert!(p.to_lowercase().contains("markdown"));
    }

    #[test]
    fn prompt_excludes_markdown_when_plain() {
        let p = OllamaBackend::build_prompt(&TranscribeOptions {
            language: None,
            markdown: false,
        });
        assert!(p.to_lowercase().contains("plain text"));
    }

    #[test]
    fn prompt_inlines_language_hint() {
        let p = OllamaBackend::build_prompt(&TranscribeOptions {
            language: Some("de".into()),
            markdown: false,
        });
        assert!(p.contains("de"));
    }

    #[test]
    fn constructor_rejects_unparseable_url() {
        match OllamaBackend::new("not a url", "qwen3.5:4b") {
            Err(OcrError::Unreachable(_)) => {}
            Err(other) => panic!("expected Unreachable, got {other}"),
            Ok(_) => panic!("expected error from unparseable URL"),
        }
    }

    #[test]
    fn constructor_accepts_localhost_default() {
        let backend = OllamaBackend::new("http://localhost:11434", "qwen3.5:4b")
            .expect("localhost URL parses");
        assert_eq!(backend.name(), "ollama/qwen3.5:4b");
    }

    #[test]
    fn strip_thinking_removes_closed_block() {
        let r = strip_thinking("<think>let me read…</think>\nThe page says hello.");
        assert_eq!(r, "The page says hello.");
    }

    #[test]
    fn strip_thinking_removes_multiple_blocks() {
        let r = strip_thinking("<think>step 1</think>line one\n<think>step 2</think>line two");
        assert_eq!(r, "line one\nline two");
    }

    #[test]
    fn strip_thinking_drops_unclosed_block_entirely() {
        // The model ran out of tokens mid-thinking; better an empty
        // transcript than dumping the reasoning trace as the
        // user's "OCR result".
        let r = strip_thinking("<think>still reasoning about the layout when");
        assert_eq!(r, "");
    }

    #[test]
    fn strip_thinking_is_identity_when_no_block() {
        let r = strip_thinking("Plain transcript text.");
        assert_eq!(r, "Plain transcript text.");
    }

    #[test]
    fn strip_thinking_preserves_prefix_before_first_block() {
        let r = strip_thinking("answer line<think>side note</think> tail");
        assert_eq!(r, "answer line tail");
    }
}
