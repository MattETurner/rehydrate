//! Persistent app config. A tiny JSON file in the platform's config dir
//! that remembers the last-opened library so the user doesn't have to
//! click "Open library" on every launch.
//!
//! macOS:   ~/Library/Application Support/Marginalia/config.json
//! Linux:   ~/.config/marginalia/config.json
//! Windows: %APPDATA%\Marginalia\config.json

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
    directories::ProjectDirs::from("app", "marginalia", "Marginalia")
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
