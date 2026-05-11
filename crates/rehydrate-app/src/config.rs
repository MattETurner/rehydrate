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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OllamaConfig {
    /// Base URL of the Ollama daemon. Default points at the official
    /// local-loopback port; user may override to a remote host they
    /// run themselves (e.g. a GPU box on their LAN).
    pub base_url: String,
    /// The model tag passed to `/api/generate`. The curated UI
    /// dropdown surfaces `qwen2.5vl:3b` and `qwen2.5vl:7b`, plus a
    /// "Custom…" free-text option for anything else the user has
    /// `ollama pull`ed.
    pub model: String,
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
    match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).unwrap_or_default(),
        Err(_) => AppConfig::default(),
    }
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
    Ok(())
}
