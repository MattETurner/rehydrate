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
use std::sync::Arc;
use std::time::Instant;

use rehydrate_core::{Library, Manifest, VersionId};
use rehydrate_ocr::{
    default_model_id, OcrBackend, OcrCancel, OcrError, OcrProgressEvent, OllamaBackend,
    TranscribeOptions,
};
use rehydrate_publish::{
    DraftPost, GhostClient, GhostCredentials, PublishResult, PublishTarget, Publisher,
    WordpressClient, WordpressCredentials,
};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, State};
use tauri_plugin_dialog::DialogExt;
use tokio::sync::mpsc;

use crate::config;
use crate::state::{
    AppState, OllamaPing, KEYRING_GHOST_CREDS, KEYRING_SERVICE, KEYRING_WORDPRESS_CREDS,
    OLLAMA_PING_TTL,
};

const TRANSCRIPT_PATH: &str = "ocr/transcript.md";

fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

async fn lib_arc(state: &State<'_, AppState>) -> Result<Arc<Library>, String> {
    state
        .library
        .lock()
        .await
        .as_ref()
        .map(Arc::clone)
        .ok_or_else(|| "no library is open".to_string())
}

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

/// Reject URLs that the user shouldn't be pointing OCR at. Three
/// gates:
///
/// 1. Must parse with a host (the existing `host_of` check).
/// 2. Scheme must be `http` or `https` — `file://`, `gopher://`,
///    etc. would be Just Weird and could trick ureq into doing
///    something unexpected.
/// 3. Non-loopback hosts may not use plain `http://`. Localhost is
///    fine over http (the daemon doesn't speak HTTPS); but if the
///    user points at a LAN box, encrypt the link. Otherwise PNG
///    page renders and transcript responses cross the network in
///    plaintext, and anyone on-path sees handwriting content.
///
/// Both `save_ollama_config` and `ping_ollama` route through this
/// so the renderer can't bypass the gates by going straight to the
/// probe endpoint.
fn validate_ollama_url(url_str: &str) -> Result<(), String> {
    let parsed =
        url::Url::parse(url_str).map_err(|e| format!("Ollama URL is not a valid URL: {e}"))?;
    let host = parsed
        .host_str()
        .ok_or_else(|| format!("Ollama URL must have a host (got {url_str:?})"))?;
    let scheme = parsed.scheme();
    if scheme != "http" && scheme != "https" {
        return Err(format!(
            "Ollama URL scheme must be http or https (got {scheme:?})"
        ));
    }
    let is_loopback = host.eq_ignore_ascii_case("localhost")
        || host == "127.0.0.1"
        || host == "::1"
        || host == "[::1]";
    if scheme == "http" && !is_loopback {
        return Err(format!(
            "plain HTTP is only allowed for localhost. {host:?} must use https:// — \
             otherwise notebook page images and transcripts cross the network in plaintext."
        ));
    }
    Ok(())
}

#[tauri::command]
pub async fn ping_ollama(base_url: String) -> Result<PingReport, String> {
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
    let report = tauri::async_runtime::spawn_blocking(move || -> PingReport {
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
        match agent.get(&url) {
            Ok(resp) if resp.status == 200 => {
                match serde_json::from_str::<TagsResponse>(&resp.body) {
                    Ok(parsed) => PingReport {
                        ok: true,
                        error: None,
                        models: parsed.models.into_iter().map(|m| m.name).collect(),
                    },
                    Err(e) => PingReport {
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
    .map_err(err)?;
    Ok(report)
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
    let probe = ping_ollama(base.clone()).await?;
    if !probe.ok {
        return Ok(OcrStatusReport::Unreachable {
            base_url: base,
            model,
            error: probe.error.unwrap_or_else(|| "unreachable".into()),
        });
    }
    let model_present = probe.models.iter().any(|m| m == &model);
    record_ping(&state, &base, model_present).await;
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

async fn record_ping(state: &State<'_, AppState>, base_url: &str, ok: bool) {
    *state.last_ollama_ping.write().await = Some(OllamaPing {
        at: Instant::now(),
        base_url: base_url.to_string(),
        ok,
    });
}

async fn cached_ping_ok(state: &State<'_, AppState>, base_url: &str) -> Option<bool> {
    let guard = state.last_ollama_ping.read().await;
    let p = guard.as_ref()?;
    if p.base_url != base_url {
        return None;
    }
    if p.at.elapsed() > OLLAMA_PING_TTL {
        return None;
    }
    Some(p.ok)
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
    if let Some(false) = cached_ping_ok(&state, &ollama.base_url).await {
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

    // Render every page to PNG on a worker thread.
    let mut blobs = Vec::with_capacity(rm_pages.len());
    for f in &rm_pages {
        blobs.push(lib.read_blob(&f.sha256).map_err(err)?);
    }
    let pages_png = tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::with_capacity(blobs.len());
        for (idx, bytes) in blobs.iter().enumerate() {
            match rehydrate_ocr::render_rm_to_png(bytes) {
                Ok(png) => out.push(png),
                Err(e) => {
                    return Err(format!("page {idx} render failed: {e}"));
                }
            }
        }
        Ok::<Vec<Vec<u8>>, String>(out)
    })
    .await
    .map_err(err)??;

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

    // Forward backend progress to the renderer.
    let (tx, mut rx) = mpsc::channel::<OcrProgressEvent>(32);
    let app_for_emit = app.clone();
    let forwarder = tauri::async_runtime::spawn(async move {
        while let Some(ev) = rx.recv().await {
            let _ = app_for_emit.emit("ocr:progress", &ev);
        }
    });

    let opts = TranscribeOptions {
        language,
        markdown: true,
    };
    let cancel = OcrCancel::new();
    let pages_result = backend
        .transcribe_pages(pages_png, &opts, Some(tx), cancel)
        .await;
    let _ = forwarder.await;
    let pages = match pages_result {
        Ok(p) => {
            record_ping(&state, &ollama.base_url, true).await;
            p
        }
        Err(OcrError::Unreachable(msg)) => {
            record_ping(&state, &ollama.base_url, false).await;
            return Err(unconfigured_error(&ollama.base_url, &ollama.model, &msg));
        }
        Err(e) => {
            return Err(format!("OCR failed: {e}"));
        }
    };

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
    let mut markdown = String::new();
    markdown.push_str("---\n");
    markdown.push_str(&format!("model: {model_name}\n"));
    markdown.push_str(&format!("created_at: {now}\n"));
    if !detected_lang.is_empty() {
        markdown.push_str(&format!("language: {detected_lang}\n"));
    }
    markdown.push_str("---\n\n");
    let mut total_chars = 0usize;
    for (i, page) in pages.iter().enumerate() {
        if i > 0 {
            markdown.push_str("\n\n");
        }
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
        page_count: pages.len(),
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
    let json = read_keychain(KEYRING_GHOST_CREDS)
        .ok_or_else(|| "no Ghost credentials saved".to_string())?;
    let creds: GhostCredentials = serde_json::from_str(&json).map_err(err)?;
    GhostClient::new(creds).map_err(err)
}

fn load_wordpress_client() -> Result<WordpressClient, String> {
    let json = read_keychain(KEYRING_WORDPRESS_CREDS)
        .ok_or_else(|| "no WordPress credentials saved".to_string())?;
    let creds: WordpressCredentials = serde_json::from_str(&json).map_err(err)?;
    WordpressClient::new(creds).map_err(err)
}

fn read_keychain(slot: &str) -> Option<String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, slot).ok()?;
    entry.get_password().ok()
}

fn write_keychain(slot: &str, value: &str) -> Result<(), String> {
    keyring::Entry::new(KEYRING_SERVICE, slot)
        .map_err(err)?
        .set_password(value)
        .map_err(err)
}

fn forget_keychain(slot: &str) -> Result<(), String> {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, slot) {
        // Some keyring backends return a "not found" error when the
        // entry doesn't exist; treat as success.
        let _ = entry.delete_credential();
    }
    Ok(())
}

#[tauri::command]
pub async fn set_ghost_credentials(creds: GhostCredentials) -> Result<(), String> {
    let json = serde_json::to_string(&creds).map_err(err)?;
    write_keychain(KEYRING_GHOST_CREDS, &json)
}

#[tauri::command]
pub async fn forget_ghost_credentials() -> Result<(), String> {
    forget_keychain(KEYRING_GHOST_CREDS)
}

#[tauri::command]
pub async fn set_wordpress_credentials(creds: WordpressCredentials) -> Result<(), String> {
    let json = serde_json::to_string(&creds).map_err(err)?;
    write_keychain(KEYRING_WORDPRESS_CREDS, &json)
}

#[tauri::command]
pub async fn forget_wordpress_credentials() -> Result<(), String> {
    forget_keychain(KEYRING_WORDPRESS_CREDS)
}

#[derive(Serialize)]
pub struct PublishCredentialStatus {
    pub ghost: bool,
    pub wordpress: bool,
}

#[tauri::command]
pub async fn publish_credential_status() -> Result<PublishCredentialStatus, String> {
    Ok(PublishCredentialStatus {
        ghost: read_keychain(KEYRING_GHOST_CREDS).is_some(),
        wordpress: read_keychain(KEYRING_WORDPRESS_CREDS).is_some(),
    })
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
    fn accepts_remote_https() {
        validate_ollama_url("https://ollama.example.com").unwrap();
        validate_ollama_url("https://10.0.0.5:11434").unwrap();
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
