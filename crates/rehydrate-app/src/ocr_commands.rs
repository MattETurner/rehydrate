//! IPC commands for the OCR + publish pipeline. Lives in its own
//! module so `commands.rs` doesn't grow into a wall.
//!
//! Pattern matches the rest of the IPC layer:
//! - `lib_arc(&state)` for the open library.
//! - `tauri::async_runtime::spawn_blocking` for sync work (Ollama
//!   HTTP, page rendering, publish API).
//! - Progress streamed via `app.emit("ocr:*", &event)`.
//! - Server-side dialog pickers for save targets so the renderer
//!   can't aim writes at arbitrary paths (audit fix H7 pattern).
//!
//! OCR backend: built per call from `AppConfig.ollama` (no long-lived
//! `Arc<dyn OcrBackend>` in `AppState`). `OllamaBackend::new` is
//! cheap — one `ureq::AgentBuilder` — and per-call construction
//! means a settings change takes effect immediately without a
//! reload step.

use std::path::PathBuf;
use std::time::Instant;

use rehydrate_core::{Manifest, VersionId};
use rehydrate_ocr::{
    default_model_id, OcrBackend, OcrError, OcrProgressEvent, OllamaBackend, TranscribeOptions,
};
use rehydrate_publish::{
    host_of, DraftPost, GhostClient, GhostCredentials, PublishResult, PublishTarget, Publisher,
    WordpressClient, WordpressCredentials,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::mpsc;

use crate::config;
use crate::keychain;
use crate::state::{
    AppState, OllamaPing, KEYRING_GHOST_CREDS, KEYRING_WORDPRESS_CREDS, OLLAMA_PING_TTL,
};
use crate::util::{err, lib_arc};

const TRANSCRIPT_PATH: &str = "ocr/transcript.md";

// =====================================================================
//   Ollama config + reachability
// =====================================================================

/// `OllamaConfig` re-exported as the IPC DTO. The struct already
/// derives `Serialize + Deserialize`, and using it directly keeps
/// the Rust and TS sides in lockstep — adding a new field touches
/// `config.rs` and the TS interface only.
pub type OllamaConfigDto = config::OllamaConfig;

#[derive(Serialize)]
pub struct PingReport {
    pub ok: bool,
    pub error: Option<String>,
    /// Names of models the daemon reports via `/api/tags`. The UI uses
    /// this to confirm that the model dropdown's selection has
    /// actually been pulled.
    pub models: Vec<String>,
}

/// Curated list of recommended models surfaced in the Settings UI.
/// The UI shows these as the primary options; a "Custom…" row lets
/// the user enter anything else they've pulled.
#[derive(Serialize)]
pub struct CuratedOllamaModel {
    pub id: String,
    pub label: String,
    pub vram_hint: &'static str,
}

#[tauri::command]
pub async fn get_ollama_config() -> Result<OllamaConfigDto, String> {
    Ok(config::load().ollama)
}

#[tauri::command]
pub async fn save_ollama_config(cfg: OllamaConfigDto) -> Result<(), String> {
    validate_ollama_url(&cfg.base_url)?;
    if cfg.model.trim().is_empty() {
        return Err("Pick a model from the list or enter a custom name.".into());
    }
    let mut on_disk = config::load();
    on_disk.ollama = cfg;
    config::save(&on_disk).map_err(err)
}

/// Reject URLs the user shouldn't be pointing OCR at.
///
/// The actual rules — scheme/loopback/private-IP gating — live in
/// `rehydrate_ocr::validate_remote_url`, which is also what
/// `RestrictedAgent::for_base` calls before constructing the ureq
/// agent. Keeping the gate in one place means the IPC probe and the
/// production fetch always see the same answer; a renderer can't
/// bypass the gate by going through a different code path.
fn validate_ollama_url(url_str: &str) -> Result<(), String> {
    rehydrate_ocr::validate_remote_url(url_str).map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn ping_ollama(
    base_url: String,
    state: State<'_, AppState>,
) -> Result<PingReport, String> {
    let report = probe_ollama(base_url.clone()).await?;
    // Every probe — whether from Settings → Test connection, from
    // ocr_status, or from an internal call — must refresh the
    // shared reachability cache. Without this the user can verify
    // in Settings that the daemon is up, return to the library,
    // hit Convert, and still be told it's unreachable for the
    // full 30s cache TTL.
    record_ping(&state, &base_url, report.ok).await;
    Ok(report)
}

/// Internal probe that performs the HTTP call without touching
/// the cache. Lets `ocr_status` re-use the same logic while only
/// recording once at its own call site (and lets the test suite
/// exercise the network shape without a `State` handle).
async fn probe_ollama(base_url: String) -> Result<PingReport, String> {
    // Same scheme + loopback gate the save path enforces — probing
    // bypasses save, so without this the renderer could trigger
    // requests to e.g. cloud-metadata endpoints just by calling
    // ping_ollama with a crafted URL.
    if let Err(msg) = validate_ollama_url(&base_url) {
        return Ok(PingReport {
            ok: false,
            error: Some(msg),
            models: Vec::new(),
        });
    }
    // Spawn-blocking because ureq is sync. Short timeout for the
    // probe — the UI is waiting on this.
    tauri::async_runtime::spawn_blocking(move || -> PingReport {
        let agent = match rehydrate_ocr::RestrictedAgent::for_base_with_timeout(
            &base_url,
            std::time::Duration::from_secs(5),
        ) {
            Ok(a) => a,
            Err(e) => {
                return PingReport {
                    ok: false,
                    error: Some(format!("{e}")),
                    models: Vec::new(),
                }
            }
        };
        let url = format!("{}/api/tags", base_url.trim_end_matches('/'));
        match agent.get(&url, &[]) {
            Ok(resp) if resp.status == 200 => {
                match serde_json::from_str::<TagsResponse>(&resp.body) {
                    Ok(parsed) => PingReport {
                        ok: true,
                        error: None,
                        models: parsed.models.into_iter().map(|m| m.name).collect(),
                    },
                    Err(e) => PingReport {
                        // Daemon answered with 200 — still reachable
                        // even if the body wasn't the JSON we expected
                        // (older / forked builds, proxies). Treat as
                        // reachable so we don't loop back to "Ollama
                        // unreachable"; the empty model list surfaces
                        // a clear "Model not pulled" path instead.
                        ok: true,
                        error: Some(format!(
                            "connected, but /api/tags returned unexpected JSON: {e}"
                        )),
                        models: Vec::new(),
                    },
                }
            }
            Ok(resp) => PingReport {
                ok: false,
                error: Some(format!("HTTP {}", resp.status)),
                models: Vec::new(),
            },
            Err(e) => PingReport {
                ok: false,
                error: Some(format!("{e}")),
                models: Vec::new(),
            },
        }
    })
    .await
    .map_err(err)
}

#[tauri::command]
pub async fn list_curated_ollama_models() -> Result<Vec<CuratedOllamaModel>, String> {
    // Qwen 3.5 (released ~1 month before v1.0.0) supersedes the
    // Qwen3-VL line. Upstream benchmarks: OCRBench 93.1% and
    // OmniDocBench1.5 90.8% — both directly relevant to the
    // handwritten-notebook workload. Two curated tiers keep the
    // dropdown manageable; the "Custom…" option in the picker
    // covers users who pull a different model.
    Ok(vec![
        CuratedOllamaModel {
            id: "qwen3.5:4b".into(),
            label: "Qwen 3.5 4B — default, fast".into(),
            vram_hint: "~4 GB VRAM / Apple Silicon unified memory",
        },
        CuratedOllamaModel {
            id: "qwen3.5:9b".into(),
            label: "Qwen 3.5 9B — sharper at cursive + math".into(),
            vram_hint: "~7 GB VRAM recommended",
        },
    ])
}

#[derive(Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TagsModel>,
}

#[derive(Deserialize)]
struct TagsModel {
    name: String,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum OcrStatusReport {
    /// The configured Ollama URL doesn't respond. UI auto-opens the
    /// Settings modal on the Ollama tab.
    Unreachable {
        base_url: String,
        model: String,
        error: String,
    },
    /// Daemon responds but doesn't have the configured model pulled.
    /// UI surfaces an explainer + a copy-paste `ollama pull` command.
    ModelMissing {
        base_url: String,
        model: String,
        available: Vec<String>,
    },
    /// Ready to transcribe.
    Ready { base_url: String, model: String },
}

#[tauri::command]
pub async fn ocr_status(state: State<'_, AppState>) -> Result<OcrStatusReport, String> {
    let ollama = config::load().ollama;
    let base = ollama.base_url.clone();
    let model = ollama.model.clone();
    let probe = probe_ollama(base.clone()).await?;
    // Always record reachability — daemon answered or not — so a
    // missing-model branch below doesn't lock the reachability
    // cache into a false negative.
    record_ping(&state, &base, probe.ok).await;
    if !probe.ok {
        return Ok(OcrStatusReport::Unreachable {
            base_url: base,
            model,
            error: probe.error.unwrap_or_else(|| "unreachable".into()),
        });
    }
    let model_present = probe.models.iter().any(|m| m == &model);
    if model_present {
        Ok(OcrStatusReport::Ready {
            base_url: base,
            model,
        })
    } else {
        Ok(OcrStatusReport::ModelMissing {
            base_url: base,
            model,
            available: probe.models,
        })
    }
}

/// Helpers take `&AppState` rather than `State<'_, AppState>` so
/// the cache semantics are unit-testable without a Tauri runtime.
/// Tauri's `State<T>` derefs to `&T`, so call sites pass `&state`
/// from the IPC handlers unchanged.
async fn record_ping(state: &AppState, base_url: &str, reachable: bool) {
    *state.last_ollama_ping.write().await = Some(OllamaPing {
        at: Instant::now(),
        base_url: base_url.to_string(),
        reachable,
    });
}

/// Returns the cached reachability of the daemon at `base_url`, or
/// `None` if there's no recent probe to consult. Strictly
/// reachability — the caller still has to handle "reachable but
/// model missing" separately.
async fn cached_reachable(state: &AppState, base_url: &str) -> Option<bool> {
    let guard = state.last_ollama_ping.read().await;
    let p = guard.as_ref()?;
    if p.base_url != base_url {
        return None;
    }
    if p.at.elapsed() > OLLAMA_PING_TTL {
        return None;
    }
    Some(p.reachable)
}

// =====================================================================
//   Transcribe
// =====================================================================

#[derive(Serialize)]
pub struct TranscriptSummary {
    pub document_id: String,
    pub version_id: VersionId,
    pub page_count: usize,
    pub char_count: usize,
    pub model: String,
}

#[derive(Serialize)]
pub struct TranscriptDocument {
    pub document_id: String,
    pub version_id: VersionId,
    pub markdown: String,
    pub model: Option<String>,
    pub created_at: Option<String>,
    pub language: Option<String>,
}

#[tauri::command]
pub async fn transcribe_document(
    app: AppHandle,
    state: State<'_, AppState>,
    document_id: String,
    language: Option<String>,
) -> Result<TranscriptSummary, String> {
    let lib = lib_arc(&state).await?;
    let ollama = config::load().ollama;

    // Reachability gate: if the cached probe says the daemon is
    // down, fail fast with a tagged error the renderer maps to
    // "open Settings → Ollama tab". A miss in the cache falls
    // through to the actual request, which surfaces the same error
    // shape via the backend.
    //
    // CRITICAL: the cache is *strictly* reachability now (see
    // `OllamaPing::reachable`). Earlier versions overloaded it
    // with "model present" or "OCR succeeded", which made
    // failure modes like "wrong model name" poison the cache as
    // "unreachable" for 30s and leave the user staring at an
    // error while Settings → Test connection happily said
    // everything was fine.
    if let Some(false) = cached_reachable(&state, &ollama.base_url).await {
        return Err(unconfigured_error(
            &ollama.base_url,
            &ollama.model,
            "cached probe failed",
        ));
    }

    // Pull the manifest + every `.rm` page blob.
    let docs = lib.list_documents().map_err(err)?;
    let doc = docs
        .iter()
        .find(|d| d.document_id == document_id)
        .ok_or_else(|| format!("document {document_id} not in library"))?;
    let manifest_bytes = lib.read_blob(&doc.current_manifest).map_err(err)?;
    let manifest = Manifest::from_canonical_json(&manifest_bytes).map_err(err)?;

    let mut rm_pages: Vec<_> = manifest
        .files
        .iter()
        .filter(|f| {
            !f.derived
                && f.path.ends_with(".rm")
                && !f.path.contains(".thumbnails")
                && !f.path.ends_with(".local")
        })
        .collect();
    rm_pages.sort_by(|a, b| a.path.cmp(&b.path));
    if rm_pages.is_empty() {
        return Err("document has no .rm pages to transcribe".into());
    }
    // Memory guard: every page renders into a Vec<u8> PNG (~100 KB
    // per page typical, more for dense ink), and all PNGs live in
    // memory together while transcribe_pages iterates them serially
    // through Ollama. A 1 000-page notebook would peak around
    // 100 MB just for the PNG vec — fine on a desktop, but
    // pathological notebook sizes can balloon further. 500 pages
    // is a generous ceiling for any realistic notebook; users with
    // larger ones can split them, which is also a saner OCR
    // workflow (each split takes minutes on CPU).
    const MAX_OCR_PAGES: usize = 500;
    if rm_pages.len() > MAX_OCR_PAGES {
        return Err(format!(
            "notebook has {} pages — OCR is capped at {} pages to keep memory bounded. \
             Split the notebook on the tablet first.",
            rm_pages.len(),
            MAX_OCR_PAGES
        ));
    }

    // Render every page to PNG on a worker thread. Pages with no
    // ink are filtered out BEFORE the model is called — VLMs
    // (qwen3.5:4b in particular) confabulate plausible essay-shape
    // text when handed a pure-white canvas, and the user has hit
    // that bug ("blank notebook → multi-paragraph fake transcript
    // about AI in healthcare"). The blank indices are remembered
    // so the output markdown still preserves page ordering — each
    // skipped page becomes an empty entry in the same slot.
    let mut blobs = Vec::with_capacity(rm_pages.len());
    for f in &rm_pages {
        blobs.push(lib.read_blob(&f.sha256).map_err(err)?);
    }
    let total_pages = blobs.len();
    /// Bundle returned from the per-page rendering pass — keeps the
    /// `spawn_blocking` closure's signature out of clippy's
    /// type-complexity bucket and documents what each field is for
    /// at the call site.
    struct RenderedPages {
        /// PNG bytes for every non-blank page, in notebook order.
        pages_png: Vec<Vec<u8>>,
        /// One entry per notebook page: true if the page was blank
        /// and therefore skipped before the model was called.
        blank_mask: Vec<bool>,
        /// `slice_to_notebook[i]` = notebook index of the i-th
        /// element in `pages_png`. Used to remap backend progress
        /// events from slice-index back to notebook-index.
        slice_to_notebook: Vec<usize>,
    }
    let rendered =
        tauri::async_runtime::spawn_blocking(move || -> Result<RenderedPages, String> {
            let mut out = Vec::with_capacity(blobs.len());
            let mut blank = Vec::with_capacity(blobs.len());
            // Backend events index into the post-filter slice, but
            // the UI counts and reports against the notebook's real
            // page index. `slice_to_notebook[i]` lets the forwarder
            // remap a backend event's `page_index` back to the
            // user-visible position; without this the progress chip
            // ticks 1, 2, 3 for a 5-page notebook with two blanks
            // and the user thinks two pages went missing.
            let mut slice_to_notebook = Vec::new();
            for (idx, bytes) in blobs.iter().enumerate() {
                if !rehydrate_ocr::rm_page_has_ink(bytes) {
                    blank.push(true);
                    continue;
                }
                match rehydrate_ocr::render_rm_to_png(bytes) {
                    Ok(png) => {
                        out.push(png);
                        blank.push(false);
                        slice_to_notebook.push(idx);
                    }
                    Err(e) => {
                        return Err(format!("page {idx} render failed: {e}"));
                    }
                }
            }
            Ok(RenderedPages {
                pages_png: out,
                blank_mask: blank,
                slice_to_notebook,
            })
        })
        .await
        .map_err(err)??;
    let RenderedPages {
        pages_png,
        blank_mask,
        slice_to_notebook,
    } = rendered;
    let blank_count = blank_mask.iter().filter(|b| **b).count();
    if pages_png.is_empty() {
        // Every page in the notebook is blank. Returning an empty
        // transcript with a frontmatter header lets the renderer
        // show the "transcript exists but is empty" state instead
        // of failing with a confusing error — and it locks in the
        // page count so the user sees we did consider all N pages.
        let model_name = format!("ollama/{}", ollama.model);
        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();
        let markdown = format!(
            "---\nmodel: {model_name}\ncreated_at: {now}\nnote: skipped — all pages blank\n---\n\n"
        );
        let outcome = lib
            .record_derived_artefact(&document_id, TRANSCRIPT_PATH, markdown.as_bytes())
            .map_err(err)?;
        return Ok(TranscriptSummary {
            document_id,
            version_id: outcome.version_id,
            page_count: total_pages,
            char_count: 0,
            model: model_name,
        });
    }

    // Build the backend now that we know we have work to do. Cheap
    // (one AgentBuilder); fails fast on a bad URL so we surface the
    // tagged error before any PNG rendering.
    let backend = match OllamaBackend::new(&ollama.base_url, &ollama.model) {
        Ok(b) => b,
        Err(e) => {
            record_ping(&state, &ollama.base_url, false).await;
            return Err(unconfigured_error(
                &ollama.base_url,
                &ollama.model,
                &format!("{e}"),
            ));
        }
    };

    // Emit a `PageDone` event for every blank page upfront so the
    // progress chip ticks through the full notebook count rather
    // than stopping at the non-blank subset. The chars: 0 keeps the
    // running character total honest. Direct emit (rather than
    // routing through the channel) bypasses the remapper below,
    // which only applies to backend-originated events with a
    // slice-index.
    for (notebook_idx, was_blank) in blank_mask.iter().enumerate() {
        if *was_blank {
            let _ = app.emit(
                "ocr:progress",
                &OcrProgressEvent::PageDone {
                    page_index: notebook_idx,
                    chars: 0,
                },
            );
        }
    }

    // Forward backend progress to the renderer, remapping the
    // backend's slice-index `page_index` to the notebook's real
    // index so events emitted to the UI line up with the synthetic
    // blank events above.
    let (tx, mut rx) = mpsc::channel::<OcrProgressEvent>(32);
    let app_for_emit = app.clone();
    let forwarder = tauri::async_runtime::spawn(async move {
        while let Some(mut ev) = rx.recv().await {
            match &mut ev {
                OcrProgressEvent::PageStarted { page_index }
                | OcrProgressEvent::PageDone { page_index, .. }
                | OcrProgressEvent::PageFailed { page_index, .. } => {
                    if let Some(real) = slice_to_notebook.get(*page_index) {
                        *page_index = *real;
                    }
                }
                OcrProgressEvent::Done { .. } => {}
            }
            let _ = app_for_emit.emit("ocr:progress", &ev);
        }
    });

    let opts = TranscribeOptions {
        language,
        markdown: true,
    };
    // Reset the shared OCR cancel handle (it may have been tripped
    // by a previous `cancel_ocr` call); the renderer can flip it
    // again to abort the in-flight transcribe at the next page.
    state.ocr_cancel.reset();
    let pages_result = backend
        .transcribe_pages(pages_png, &opts, Some(tx), state.ocr_cancel.clone())
        .await;
    let _ = forwarder.await;
    let report = match pages_result {
        Ok(r) => {
            record_ping(&state, &ollama.base_url, true).await;
            r
        }
        Err(OcrError::Unreachable(msg)) => {
            record_ping(&state, &ollama.base_url, false).await;
            return Err(unconfigured_error(&ollama.base_url, &ollama.model, &msg));
        }
        Err(e) => {
            return Err(format!("OCR failed: {e}"));
        }
    };
    // Refuse to commit if every page failed. Without this guard the
    // user gets a "transcript saved" toast pointing at a file
    // containing nothing but failure placeholders — and worse, the
    // empty transcript would claim authority over the document
    // (next auto-OCR sweep would skip it because it "already has"
    // a transcript). Surface the per-page errors so support
    // diagnostics aren't a black box.
    if report.is_all_failed() {
        let sample = report
            .failures
            .first()
            .map(|f| f.message.clone())
            .unwrap_or_else(|| "(no failure details)".into());
        return Err(format!(
            "OCR failed for every page ({} pages attempted, none succeeded). \
             First failure: {sample}",
            report.failures.len()
        ));
    }
    let pages = report.pages;
    let page_failures = report.failures;

    // Assemble Markdown with a small frontmatter block recording
    // model + timestamp + language so future re-OCR can decide
    // whether to invalidate.
    let model_name = backend.name().to_string();
    let now = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();
    let detected_lang = pages
        .iter()
        .find_map(|p| p.detected_language.clone())
        .unwrap_or_default();
    // The frontmatter has to be honest about what's missing. The
    // user is going to act on this transcript (search, publish,
    // archive); if 3 of 50 pages failed they should see that at the
    // top of the file, not have it buried in an `ocr:progress`
    // event they never saw.
    let mut markdown = String::new();
    markdown.push_str("---\n");
    markdown.push_str(&format!("model: {model_name}\n"));
    markdown.push_str(&format!("created_at: {now}\n"));
    if !detected_lang.is_empty() {
        markdown.push_str(&format!("language: {detected_lang}\n"));
    }
    let transcribed_count = pages.len();
    markdown.push_str(&format!(
        "pages_transcribed: {transcribed_count} of {total_pages}\n"
    ));
    markdown.push_str("---\n\n");
    if blank_count > 0 {
        // Record how many pages we skipped so the user can spot it
        // in the transcript header without re-running OCR. Avoids
        // the previous failure mode where blank pages produced
        // hallucinated essay text indistinguishable from a real
        // transcript.
        markdown.push_str(&format!(
            "_note: {blank_count} blank page{} skipped_\n\n",
            if blank_count == 1 { "" } else { "s" }
        ));
    }
    if !page_failures.is_empty() {
        // Surface the failure count up front. Per-page placeholders
        // below make the gaps visible inline; this summary is for
        // skim-readers.
        markdown.push_str(&format!(
            "_note: {} page{} could not be transcribed (placeholder shown inline below)_\n\n",
            page_failures.len(),
            if page_failures.len() == 1 { "" } else { "s" }
        ));
    }
    let mut total_chars = 0usize;
    // Splice entries back together respecting the notebook's
    // original page order. Three classes:
    //   * blank → skip (already handled by blank_mask).
    //   * failed → emit a placeholder so the user can see WHERE the
    //     gap is and re-run OCR on those pages specifically.
    //   * succeeded → emit the transcribed text.
    // Pre-v1.0 we just iterated `pages.iter()` and dropped failed
    // pages silently — a wedged Ollama could produce a 1-page
    // transcript for a 200-page notebook with no signal.
    let failure_indices: std::collections::HashSet<usize> =
        page_failures.iter().map(|f| f.page_index).collect();
    let mut next_transcribed = pages.iter();
    for (i, was_blank) in blank_mask.iter().enumerate() {
        if i > 0 {
            markdown.push_str("\n\n");
        }
        if *was_blank {
            continue;
        }
        if failure_indices.contains(&i) {
            markdown.push_str(&format!(
                "*[Page {}: transcription failed — re-run OCR to retry]*",
                i + 1
            ));
            continue;
        }
        let Some(page) = next_transcribed.next() else {
            break;
        };
        if !page.text.is_empty() {
            markdown.push_str(&page.text);
        }
        total_chars += page.text.chars().count();
    }
    if !markdown.ends_with('\n') {
        markdown.push('\n');
    }

    let outcome = lib
        .record_derived_artefact(&document_id, TRANSCRIPT_PATH, markdown.as_bytes())
        .map_err(err)?;

    Ok(TranscriptSummary {
        document_id,
        version_id: outcome.version_id,
        // Reports the full notebook page count so the user sees a
        // truthful "X pages" tally regardless of how many were blank.
        page_count: total_pages,
        char_count: total_chars,
        model: model_name,
    })
}

/// Build the tagged error string the frontend uses to decide
/// whether to auto-open the Settings modal on the Ollama tab. We
/// JSON-encode rather than free-text so the renderer can match by
/// `.kind == "ollama_unconfigured"` and still see the human message.
fn unconfigured_error(base_url: &str, model: &str, reason: &str) -> String {
    let v = serde_json::json!({
        "kind": "ollama_unconfigured",
        "base_url": base_url,
        "model": model,
        "message": format!(
            "Couldn't reach Ollama at {base_url} (model {model}): {reason}"
        ),
    });
    v.to_string()
}

#[tauri::command]
pub async fn get_transcript(
    state: State<'_, AppState>,
    version_id: VersionId,
) -> Result<Option<TranscriptDocument>, String> {
    let lib = lib_arc(&state).await?;
    let bytes = match lib
        .read_derived_artefact(version_id, TRANSCRIPT_PATH)
        .map_err(err)?
    {
        Some(b) => b,
        None => return Ok(None),
    };
    let entry = lib.get_version(version_id).map_err(err)?;
    let markdown = String::from_utf8_lossy(&bytes).into_owned();
    let (model, created_at, language) = parse_frontmatter(&markdown);
    Ok(Some(TranscriptDocument {
        document_id: entry.document_id,
        version_id,
        markdown,
        model,
        created_at,
        language,
    }))
}

fn parse_frontmatter(md: &str) -> (Option<String>, Option<String>, Option<String>) {
    let mut model = None;
    let mut created_at = None;
    let mut language = None;
    let trimmed = md.strip_prefix("---\n").unwrap_or(md);
    if trimmed.as_ptr() == md.as_ptr() {
        return (model, created_at, language);
    }
    if let Some(end) = trimmed.find("\n---") {
        for line in trimmed[..end].lines() {
            if let Some(v) = line.strip_prefix("model: ") {
                model = Some(v.trim().to_string());
            } else if let Some(v) = line.strip_prefix("created_at: ") {
                created_at = Some(v.trim().to_string());
            } else if let Some(v) = line.strip_prefix("language: ") {
                language = Some(v.trim().to_string());
            }
        }
    }
    (model, created_at, language)
}

// =====================================================================
//   Export transcript
// =====================================================================

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ExportFormat {
    Txt,
    Markdown,
}

#[derive(Serialize)]
pub struct ExportTranscriptResult {
    pub path: PathBuf,
}

#[tauri::command]
pub async fn export_transcript(
    app: AppHandle,
    state: State<'_, AppState>,
    version_id: VersionId,
    format: ExportFormat,
) -> Result<Option<ExportTranscriptResult>, String> {
    let lib = lib_arc(&state).await?;
    let bytes = lib
        .read_derived_artefact(version_id, TRANSCRIPT_PATH)
        .map_err(err)?
        .ok_or_else(|| "no transcript on this version".to_string())?;
    let entry = lib.get_version(version_id).map_err(err)?;
    let manifest_bytes = lib.read_blob(&entry.manifest_hash).map_err(err)?;
    let manifest = Manifest::from_canonical_json(&manifest_bytes).map_err(err)?;
    let safe_name = sanitize(&manifest.visible_name);
    let (default_name, ext) = match format {
        ExportFormat::Txt => (format!("{safe_name}-v{version_id}.txt"), "txt"),
        ExportFormat::Markdown => (format!("{safe_name}-v{version_id}.md"), "md"),
    };

    let app_for_pick = app.clone();
    let picked = tauri::async_runtime::spawn_blocking(move || {
        app_for_pick
            .dialog()
            .file()
            .add_filter(ext, &[ext])
            .set_file_name(&default_name)
            .blocking_save_file()
    })
    .await
    .map_err(err)?;

    let Some(file_path) = picked else {
        return Ok(None);
    };
    let target = file_path
        .into_path()
        .map_err(|e| format!("could not resolve picked file: {e}"))?;

    let payload: Vec<u8> = match format {
        ExportFormat::Markdown => bytes,
        ExportFormat::Txt => {
            // Strip frontmatter so the .txt is purely transcript text.
            let md = String::from_utf8_lossy(&bytes).into_owned();
            strip_frontmatter(&md).into_bytes()
        }
    };
    std::fs::write(&target, &payload).map_err(err)?;
    Ok(Some(ExportTranscriptResult { path: target }))
}

fn sanitize(s: &str) -> String {
    s.chars()
        .map(|c| {
            if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_') {
                c
            } else {
                '_'
            }
        })
        .collect::<String>()
        .trim()
        .to_string()
}

fn strip_frontmatter(md: &str) -> String {
    let trimmed = md.strip_prefix("---\n").unwrap_or(md);
    if trimmed.as_ptr() == md.as_ptr() {
        return md.to_string();
    }
    if let Some(end) = trimmed.find("\n---") {
        let after = &trimmed[end + 4..];
        return after.trim_start_matches('\n').to_string();
    }
    md.to_string()
}

// =====================================================================
//   Publish
// =====================================================================

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PublishKind {
    Ghost,
    Wordpress,
}

impl PublishKind {
    fn target(&self) -> PublishTarget {
        match self {
            PublishKind::Ghost => PublishTarget::Ghost,
            PublishKind::Wordpress => PublishTarget::Wordpress,
        }
    }
}

/// Tagged "no credentials" error used by the renderer to route the
/// user to Settings → Publishing. Mirrors `unconfigured_error` for
/// Ollama so the UI can use one error-shape pattern for both.
fn publishing_unconfigured_error(target: &str) -> String {
    let v = serde_json::json!({
        "kind": "publish_unconfigured",
        "target": target,
        "message": format!(
            "No {target} credentials saved. Open Settings → Publishing to add them."
        ),
    });
    v.to_string()
}

#[tauri::command]
pub async fn publish_transcript(
    state: State<'_, AppState>,
    version_id: VersionId,
    target: PublishKind,
) -> Result<PublishResult, String> {
    let lib = lib_arc(&state).await?;
    let bytes = lib
        .read_derived_artefact(version_id, TRANSCRIPT_PATH)
        .map_err(err)?
        .ok_or_else(|| "no transcript on this version".to_string())?;
    let md = String::from_utf8_lossy(&bytes).into_owned();
    let body = strip_frontmatter(&md);
    let mut html = String::new();
    pulldown_cmark::html::push_html(&mut html, pulldown_cmark::Parser::new(&body));

    let entry = lib.get_version(version_id).map_err(err)?;
    let manifest_bytes = lib.read_blob(&entry.manifest_hash).map_err(err)?;
    let manifest = Manifest::from_canonical_json(&manifest_bytes).map_err(err)?;

    let post = DraftPost {
        title: manifest.visible_name.clone(),
        html,
        tags: vec!["from-rehydrate".into()],
    };

    let kind = target.target();
    let target_label = match kind {
        PublishTarget::Ghost => "ghost",
        PublishTarget::Wordpress => "wordpress",
    };
    tauri::async_runtime::spawn_blocking(move || -> Result<PublishResult, String> {
        let publisher: Box<dyn Publisher> = match kind {
            PublishTarget::Ghost => match load_ghost_client() {
                Ok(c) => Box::new(c),
                Err(_) => return Err(publishing_unconfigured_error(target_label)),
            },
            PublishTarget::Wordpress => match load_wordpress_client() {
                Ok(c) => Box::new(c),
                Err(_) => return Err(publishing_unconfigured_error(target_label)),
            },
        };
        publisher.publish_draft(&post).map_err(err)
    })
    .await
    .map_err(err)?
}

fn load_ghost_client() -> Result<GhostClient, String> {
    let json = keychain::read_slot(KEYRING_GHOST_CREDS)
        .ok_or_else(|| "no Ghost credentials saved".to_string())?;
    let creds: GhostCredentials = serde_json::from_str(&json).map_err(err)?;
    GhostClient::new(creds).map_err(err)
}

fn load_wordpress_client() -> Result<WordpressClient, String> {
    let json = keychain::read_slot(KEYRING_WORDPRESS_CREDS)
        .ok_or_else(|| "no WordPress credentials saved".to_string())?;
    let creds: WordpressCredentials = serde_json::from_str(&json).map_err(err)?;
    WordpressClient::new(creds).map_err(err)
}

#[tauri::command]
pub async fn set_ghost_credentials(creds: GhostCredentials) -> Result<(), String> {
    // Validate before the credentials hit the keychain so a misshaped
    // URL never leaves the IPC layer. `validate_remote_url` blocks
    // `http://` to public hosts and any literal-IP private/link-local
    // target — both of which would leak the Admin API key in
    // cleartext or pivot to internal services.
    rehydrate_publish::validate_remote_url(&creds.base_url).map_err(err)?;
    let json = serde_json::to_string(&creds).map_err(err)?;
    keychain::write_slot(KEYRING_GHOST_CREDS, &json)
}

#[tauri::command]
pub async fn forget_ghost_credentials() -> Result<(), String> {
    keychain::forget_slot(KEYRING_GHOST_CREDS)
}

#[tauri::command]
pub async fn set_wordpress_credentials(creds: WordpressCredentials) -> Result<(), String> {
    // Mirrors the Ghost path — see the rationale there. WordPress
    // Application Passwords ship as Basic auth, so plaintext is
    // even more catastrophic.
    rehydrate_publish::validate_remote_url(&creds.base_url).map_err(err)?;
    let json = serde_json::to_string(&creds).map_err(err)?;
    keychain::write_slot(KEYRING_WORDPRESS_CREDS, &json)
}

#[tauri::command]
pub async fn forget_wordpress_credentials() -> Result<(), String> {
    keychain::forget_slot(KEYRING_WORDPRESS_CREDS)
}

#[derive(Serialize)]
pub struct PublishCredentialStatus {
    pub ghost: bool,
    pub wordpress: bool,
}

#[tauri::command]
pub async fn publish_credential_status() -> Result<PublishCredentialStatus, String> {
    Ok(PublishCredentialStatus {
        ghost: keychain::read_slot(KEYRING_GHOST_CREDS).is_some(),
        wordpress: keychain::read_slot(KEYRING_WORDPRESS_CREDS).is_some(),
    })
}

/// Open a Ghost / WordPress draft URL in the user's default browser.
///
/// Sister command to `open_support_url`, but allowlisted dynamically
/// against the saved publish credentials rather than a hard-coded
/// prefix. The acceptance rules:
///
/// 1. `target` must have credentials in the keychain. No credentials
///    → no concept of a "trusted host for `target`" → refuse.
/// 2. The URL's host (ASCII-lowercase, via `host_of`) must match the
///    saved `base_url`'s host for that target. A renderer XSS that
///    only got hold of `target` and a forged URL can't pivot the
///    browser to an attacker-controlled domain — at worst it opens
///    a path on the user's own Ghost / WordPress site.
/// 3. The URL must parse and have a scheme of `http`/`https`.
///    `validate_remote_url` enforces this and the same SSRF guards
///    `set_ghost_credentials` already applies on the saved base URL.
///
/// Used by the Transcript drawer's "Draft created — View draft"
/// toast action so the user can actually open the post that was just
/// published. Before this, the `edit_url` was surfaced as plain text
/// in a transient toast and the user couldn't click it.
#[tauri::command]
pub async fn open_publish_url(
    url: String,
    target: PublishKind,
    app: AppHandle,
) -> Result<(), String> {
    rehydrate_publish::validate_remote_url(&url).map_err(err)?;

    let url_host = host_of(&url)
        .ok_or_else(|| "could not parse host from URL".to_string())?;

    // Resolve the trusted host for `target` from saved credentials.
    // `load_*_client` returns "no credentials saved" — bubble that as a
    // distinct error so the renderer can prompt the user to configure
    // publishing instead of silently failing.
    let trusted_host = match target.target() {
        PublishTarget::Ghost => {
            let json = keychain::read_slot(KEYRING_GHOST_CREDS)
                .ok_or_else(|| "no Ghost credentials saved".to_string())?;
            let creds: GhostCredentials = serde_json::from_str(&json).map_err(err)?;
            host_of(&creds.base_url)
                .ok_or_else(|| "saved Ghost base URL has no host".to_string())?
        }
        PublishTarget::Wordpress => {
            let json = keychain::read_slot(KEYRING_WORDPRESS_CREDS)
                .ok_or_else(|| "no WordPress credentials saved".to_string())?;
            let creds: WordpressCredentials = serde_json::from_str(&json).map_err(err)?;
            host_of(&creds.base_url)
                .ok_or_else(|| "saved WordPress base URL has no host".to_string())?
        }
    };

    if url_host != trusted_host {
        let target_name = match target {
            PublishKind::Ghost => "Ghost",
            PublishKind::Wordpress => "WordPress",
        };
        return Err(format!(
            "refusing to open URL: host {url_host} does not match saved {target_name} host {trusted_host}",
        ));
    }

    app.opener()
        .open_url(&url, None::<&str>)
        .map_err(|e| format!("could not open {url}: {e}"))
}

#[tauri::command]
pub async fn ping_publish_target(target: PublishKind) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || -> Result<(), String> {
        let publisher: Box<dyn Publisher> = match target.target() {
            PublishTarget::Ghost => Box::new(load_ghost_client()?),
            PublishTarget::Wordpress => Box::new(load_wordpress_client()?),
        };
        publisher.ping().map_err(err)
    })
    .await
    .map_err(err)?
}

#[tauri::command]
pub async fn default_ollama_model() -> Result<String, String> {
    Ok(default_model_id().to_string())
}

/// Live documents whose current version doesn't already have an
/// `ocr/transcript.md` derived artefact. The auto-OCR-at-startup
/// sweep iterates this list. Documents are returned in `(visible
/// name, doc id)` shape — the renderer wants the title for the
/// progress chip, the id for the actual `transcribe_document`
/// call.
#[derive(Serialize)]
pub struct OcrCandidate {
    pub document_id: String,
    pub visible_name: String,
}

#[tauri::command]
pub async fn list_documents_needing_ocr(
    state: State<'_, AppState>,
) -> Result<Vec<OcrCandidate>, String> {
    let lib = lib_arc(&state).await?;
    // `list_documents` is cheap (one SELECT + a manifest blob hash
    // per row); the per-doc `read_derived_artefact` is a manifest
    // parse + a hash lookup that returns None without reading any
    // additional blob bytes if the path isn't in the manifest.
    // O(docs); fine for libraries up to several thousand entries
    // and below the user's "wait, why is the app frozen" threshold.
    let docs = lib.list_documents().map_err(err)?;
    let mut out = Vec::with_capacity(docs.len());
    for doc in docs {
        // Notebooks are the only doc type the OCR pipeline can
        // handle today — PDFs and EPUBs carry their own text and
        // would just produce duplicate/inferior transcripts.
        if doc.doc_type != "Notebook" {
            continue;
        }
        let existing = lib
            .read_derived_artefact(doc.current_version_id, TRANSCRIPT_PATH)
            .map_err(err)?;
        if existing.is_none() {
            out.push(OcrCandidate {
                document_id: doc.document_id,
                visible_name: doc.visible_name,
            });
        }
    }
    Ok(out)
}

#[cfg(test)]
mod ollama_url_tests {
    use super::validate_ollama_url;

    #[test]
    fn accepts_localhost_http() {
        validate_ollama_url("http://localhost:11434").unwrap();
        validate_ollama_url("http://127.0.0.1:11434").unwrap();
        validate_ollama_url("http://[::1]:11434").unwrap();
    }

    #[test]
    fn accepts_remote_https_named() {
        validate_ollama_url("https://ollama.example.com").unwrap();
    }

    #[test]
    fn rejects_https_to_private_ip() {
        // A renderer XSS that flipped the configured base URL to one
        // of these could exfiltrate page imagery to an internal host
        // the user never intended; the audit specifically flagged
        // `https://10.0.0.5` and `https://169.254.169.254` as
        // SSRF-adjacent targets that previously slipped through.
        assert!(validate_ollama_url("https://10.0.0.5:11434").is_err());
        assert!(validate_ollama_url("https://169.254.169.254").is_err());
        assert!(validate_ollama_url("https://192.168.1.10:11434").is_err());
    }

    #[test]
    fn rejects_non_loopback_plain_http() {
        // The classic LAN-Ollama misconfiguration that would
        // otherwise leak page images and transcripts in plaintext.
        assert!(validate_ollama_url("http://10.0.0.5:11434").is_err());
        assert!(validate_ollama_url("http://ollama.example.com").is_err());
        assert!(validate_ollama_url("http://192.168.1.10:11434").is_err());
    }

    #[test]
    fn rejects_non_http_schemes() {
        // ureq would do something unexpected with these.
        assert!(validate_ollama_url("file:///etc/passwd").is_err());
        assert!(validate_ollama_url("gopher://example.com").is_err());
    }

    #[test]
    fn rejects_malformed_urls() {
        assert!(validate_ollama_url("").is_err());
        assert!(validate_ollama_url("not a url").is_err());
        assert!(validate_ollama_url("http://").is_err());
    }
}

#[cfg(test)]
mod reachability_cache_tests {
    //! Cache-semantics regressions. Three bugs cohabited the
    //! earlier version of this file:
    //!
    //! 1. `OllamaPing::ok` was overloaded — `ocr_status` wrote
    //!    `model_present` into the same field `transcribe_document`
    //!    later read as "daemon reachable". A missing model thus
    //!    locked the cache into a false negative for 30 s.
    //!
    //! 2. `ping_ollama` (the Settings → Test Connection IPC
    //!    handler) didn't update the cache at all, so a user
    //!    verifying the connection in Settings could not clear a
    //!    stale negative reading.
    //!
    //! 3. The cache then short-circuited the next transcribe with
    //!    "Ollama unreachable" while Settings simultaneously said
    //!    everything was fine — exactly the symptom the user
    //!    reported.
    //!
    //! These tests pin the contract that prevents the regression.
    use super::*;
    use crate::state::AppState;

    #[tokio::test]
    async fn record_ping_persists_reachable_flag_per_base_url() {
        let state = AppState::new();
        assert_eq!(
            cached_reachable(&state, "http://localhost:11434").await,
            None
        );

        record_ping(&state, "http://localhost:11434", true).await;
        assert_eq!(
            cached_reachable(&state, "http://localhost:11434").await,
            Some(true),
        );

        // Switching base URL invalidates the cached reading — the
        // user may have edited Settings between probes.
        assert_eq!(cached_reachable(&state, "http://remote:11434").await, None);

        record_ping(&state, "http://localhost:11434", false).await;
        assert_eq!(
            cached_reachable(&state, "http://localhost:11434").await,
            Some(false),
        );
    }

    #[tokio::test]
    async fn a_successful_probe_clears_a_prior_unreachable_cache() {
        // Models the user's reported flow: an earlier OCR run wrote
        // `reachable: false` (network blip, daemon restart, etc.),
        // then the user hit Settings → Test Connection and saw a
        // success. The next OCR start must NOT short-circuit on
        // the stale negative reading — `ping_ollama` is required
        // to refresh the cache from any call site, which means a
        // subsequent `record_ping(true)` overwrites the prior
        // `false`. The bug was that `ping_ollama` never wrote to
        // the cache, leaving the old `false` in place.
        let state = AppState::new();
        record_ping(&state, "http://localhost:11434", false).await;
        assert_eq!(
            cached_reachable(&state, "http://localhost:11434").await,
            Some(false),
        );

        // Settings' Test Connection now succeeds → must replace
        // the negative reading, not coexist with it.
        record_ping(&state, "http://localhost:11434", true).await;
        assert_eq!(
            cached_reachable(&state, "http://localhost:11434").await,
            Some(true),
            "a fresh successful probe must overwrite a stale negative cache",
        );
    }
}
