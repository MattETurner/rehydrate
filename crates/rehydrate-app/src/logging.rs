//! File-backed logging. Tracing writes to a daily-rotated log under the
//! platform's data directory so the operation log drawer in the UI can show
//! a tail of recent activity. Stderr output is preserved alongside, so
//! running the binary from a terminal still surfaces log lines live.

use std::path::PathBuf;
use std::sync::OnceLock;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Holds the appender's worker thread guard for the lifetime of the process.
/// Dropping the guard would flush and stop the appender; we never want that.
static LOG_GUARD: OnceLock<WorkerGuard> = OnceLock::new();

/// Returns the directory where reHydrate writes log files.
pub fn log_dir() -> Option<PathBuf> {
    directories::ProjectDirs::from("app", "rehydrate", "reHydrate")
        .map(|d| d.data_local_dir().join("logs"))
}

pub fn init() {
    let env_filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,rehydrate=debug"));

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true);

    // File appender. If we can't set up the log dir, fall back to stderr-
    // only — the app is still usable, but the Logs drawer will be empty.
    let file_layer = log_dir().and_then(|dir| {
        std::fs::create_dir_all(&dir).ok()?;
        let appender = rolling::daily(&dir, "rehydrate.log");
        let (writer, guard) = tracing_appender::non_blocking(appender);
        // Stash the guard so the appender thread isn't dropped.
        let _ = LOG_GUARD.set(guard);
        Some(
            tracing_subscriber::fmt::layer()
                .with_writer(writer)
                .with_ansi(false)
                .with_target(true)
                .boxed(),
        )
    });

    let registry = tracing_subscriber::registry()
        .with(env_filter)
        .with(stderr_layer);
    if let Some(file_layer) = file_layer {
        let _ = registry.with(file_layer).try_init();
    } else {
        let _ = registry.try_init();
    }
}

/// Read up to `max_lines` most-recent lines across all rotated log files.
/// Cheap for typical log sizes (a few MB); we don't bother with reverse
/// streaming until logs grow into the tens of megabytes.
pub fn read_tail(max_lines: usize) -> std::io::Result<Vec<String>> {
    let Some(dir) = log_dir() else {
        return Ok(Vec::new());
    };
    let Ok(rd) = std::fs::read_dir(&dir) else {
        return Ok(Vec::new());
    };

    // Sort entries by filename — daily rotation uses YYYY-MM-DD suffixes
    // so lexicographic order is chronological.
    let mut files: Vec<PathBuf> = rd
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_file())
        .collect();
    files.sort();

    let mut all: Vec<String> = Vec::new();
    for path in files {
        if let Ok(content) = std::fs::read_to_string(&path) {
            for line in content.lines() {
                all.push(line.to_string());
            }
        }
    }
    let start = all.len().saturating_sub(max_lines);
    Ok(all[start..].to_vec())
}
