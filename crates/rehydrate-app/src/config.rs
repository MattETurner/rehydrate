//! Persistent app config. A tiny JSON file in the platform's config dir
//! that remembers the last-opened library so the user doesn't have to
//! click "Open library" on every launch.
//!
//! macOS:   ~/Library/Application Support/reHydrate/config.json
//! Linux:   ~/.config/rehydrate/config.json
//! Windows: %APPDATA%\reHydrate\config.json

use std::fs;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// Path of the library to auto-open on next launch.
    #[serde(default)]
    pub library_path: Option<PathBuf>,
    /// Recently-used libraries, most-recent-first. Lets the user
    /// switch between per-device libraries without re-picking the
    /// folder each time.
    #[serde(default)]
    pub recent_libraries: Vec<RecentLibrary>,
    /// Connection settings for the user's Ollama daemon. `#[serde(default)]`
    /// covers configs written before this field existed — pre-1.0 users
    /// get the localhost defaults on first launch after upgrading.
    #[serde(default)]
    pub ollama: OllamaConfig,
    /// Schema-migration counter. Bumped each time we ship a
    /// one-shot transformation of an on-disk field. `load()` runs
    /// migrations up to `CURRENT_SCHEMA` and rewrites the file. A
    /// missing field reads back as `0` (pre-migration), so older
    /// configs flow through every migration exactly once.
    #[serde(default)]
    pub schema_version: u32,
    /// Device connection settings (USB-ethernet endpoint + xochitl root).
    /// Defaults match a stock reMarkable 2; users on other models can
    /// override via config or env vars.
    #[serde(default)]
    pub device: DeviceConfig,
}

/// Latest migration version recognised by this build. See
/// `migrate()` for the per-step transformations applied when a
/// config on disk lags behind.
const CURRENT_SCHEMA: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaConfig {
    /// Base URL of the Ollama daemon. Default points at the official
    /// local-loopback port; user may override to a remote host they
    /// run themselves (e.g. a GPU box on their LAN).
    pub base_url: String,
    /// The model tag passed to `/api/generate`. The curated UI
    /// dropdown surfaces `qwen3.5:4b` (default — fast, enough
    /// power for handwritten notebooks) and `qwen3.5:9b` (sharper
    /// at dense cursive + math, slower), plus a "Custom…" free-
    /// text option for anything else the user has `ollama
    /// pull`ed.
    pub model: String,
    /// When true, the app kicks off a background OCR sweep at
    /// startup: every live document whose current version doesn't
    /// already have a transcript gets transcribed serially through
    /// the same `transcribe_document` pipeline a manual "Convert
    /// to text…" would use. Default off — first-run users haven't
    /// opted in to network traffic with the Ollama daemon yet.
    /// `#[serde(default)]` on the field covers existing on-disk
    /// configs without a migration step.
    #[serde(default)]
    pub auto_ocr_on_startup: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceConfig {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub xochitl_dir: String,
}

impl Default for DeviceConfig {
    fn default() -> Self {
        Self {
            host: rehydrate_device::ssh::DEFAULT_HOST.to_string(),
            port: rehydrate_device::ssh::DEFAULT_PORT,
            user: rehydrate_device::ssh::DEFAULT_USER.to_string(),
            xochitl_dir: rehydrate_device::ssh::XOCHITL_DIR.to_string(),
        }
    }
}

impl Default for OllamaConfig {
    fn default() -> Self {
        Self {
            base_url: "http://localhost:11434".to_string(),
            // Kept in sync with `rehydrate_ocr::default_model_id()`.
            // Qwen 3.5 supersedes Qwen3-VL on Ollama — same
            // multimodal family, sharper at document OCR
            // (OCRBench 93.1%, OmniDocBench1.5 90.8%). 4B is
            // the sweet-spot tier for handwritten notebooks.
            model: "qwen3.5:4b".to_string(),
            // First-run users haven't told us they want network
            // traffic with Ollama; opt-in via the Settings toggle.
            auto_ocr_on_startup: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecentLibrary {
    pub path: PathBuf,
    /// Cached display name for the library (currently the directory's
    /// final path component). Re-derived on read; persisted so the
    /// switcher can show something even if the directory has moved.
    #[serde(default)]
    pub label: String,
    /// RFC3339 timestamp of the last time this library was opened.
    #[serde(default)]
    pub last_opened: String,
}

impl AppConfig {
    /// Insert `path` at the head of `recent_libraries`, dedupe-by-path,
    /// and trim to `MAX_RECENT`. Updates `library_path` to match.
    pub fn record_open(&mut self, path: PathBuf) {
        const MAX_RECENT: usize = 10;
        let label = path
            .file_name()
            .and_then(|s| s.to_str())
            .unwrap_or("library")
            .to_string();
        let now = time::OffsetDateTime::now_utc()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap_or_default();

        self.recent_libraries.retain(|r| r.path != path);
        self.recent_libraries.insert(
            0,
            RecentLibrary {
                path: path.clone(),
                label,
                last_opened: now,
            },
        );
        self.recent_libraries.truncate(MAX_RECENT);
        self.library_path = Some(path);
    }
}

fn config_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("app", "rehydrate", "reHydrate")
        .map(|d| d.config_dir().to_path_buf())
}

fn config_path() -> Option<PathBuf> {
    config_dir().map(|d| d.join("config.json"))
}

pub fn load() -> AppConfig {
    let Some(path) = config_path() else {
        return AppConfig::default();
    };
    let mut cfg: AppConfig = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => return AppConfig::default(),
    };
    if migrate(&mut cfg) {
        // One-shot migration rewrote a field — persist so we don't
        // run the same step again on the next launch. Save errors
        // are non-fatal here: the migration ran in memory, the
        // user gets the new behaviour this session, and we'll
        // retry on next launch.
        let _ = save(&cfg);
    }
    cfg
}

/// Applies pending schema migrations to `cfg` in place. Returns
/// `true` if anything changed (so the caller can re-persist).
///
/// Migrations are append-only: bump `CURRENT_SCHEMA`, add a step
/// that reads `cfg.schema_version` and transforms forward by one,
/// and set `cfg.schema_version` to the new number at the end of
/// the step. Each step must be idempotent in case the prior
/// `save()` failed and we re-enter on the next launch.
fn migrate(cfg: &mut AppConfig) -> bool {
    let mut changed = false;

    // v0 → v1: the `qwen3.5:9b` tier ships in the curated picker
    // but is wildly slower on CPU than `qwen3.5:4b` while
    // producing comparable transcripts on most handwritten
    // notebooks. Users who picked `:9b` before we tuned the
    // streaming OCR path (May 2026) tended to do so because it
    // was the only Qwen 3.5 tier surfaced on day one; rewrite
    // those to `:4b`. Anyone who genuinely wants the heavy tier
    // can re-pick it in Settings → Ollama and the new value
    // sticks because `schema_version` already moved past 1.
    if cfg.schema_version < 1 {
        if cfg.ollama.model == "qwen3.5:9b" {
            cfg.ollama.model = "qwen3.5:4b".to_string();
        }
        cfg.schema_version = 1;
        changed = true;
    }

    debug_assert!(cfg.schema_version <= CURRENT_SCHEMA);
    changed
}

pub fn save(cfg: &AppConfig) -> Result<(), std::io::Error> {
    let Some(dir) = config_dir() else {
        return Err(std::io::Error::other("no config dir on this platform"));
    };
    fs::create_dir_all(&dir)?;
    let path = dir.join("config.json");
    let bytes = serde_json::to_vec_pretty(cfg).map_err(|e| std::io::Error::other(e.to_string()))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, &bytes)?;
    fs::rename(&tmp, &path)?;
    // Real secrets live in the keychain; this file only ever holds
    // the library path and Ollama base-URL. Still: a hostile other-
    // user account on a shared box can otherwise see which library
    // the user opened, so clamp to owner-only on POSIX. No-op
    // elsewhere (the cfg(unix) guard makes this compile on every
    // platform without conditional-compilation noise at the
    // call site).
    set_owner_only(&path);
    Ok(())
}

#[cfg(unix)]
fn set_owner_only(path: &std::path::Path) {
    use std::os::unix::fs::PermissionsExt;
    // Best-effort: a permissions error here doesn't invalidate the
    // write that already landed.
    if let Ok(meta) = fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o600);
        let _ = fs::set_permissions(path, perms);
    }
}

#[cfg(not(unix))]
fn set_owner_only(_path: &std::path::Path) {}
