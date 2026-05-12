//! Tiny helpers shared between the two command modules.
//!
//! The goal is not abstraction — it's eliminating drift. Before this
//! module existed, both `commands.rs` and `ocr_commands.rs` defined
//! identical `err` and `lib_arc` helpers, and their tiny copies had
//! already started to diverge in trivial ways. Centralising lets a
//! future change (e.g. routing all error stringification through a
//! richer formatter, or instrumenting `lib_arc` with tracing) touch
//! one file.

use std::sync::Arc;

use rehydrate_core::Library;
use tauri::State;

use crate::state::AppState;

/// Convert any `Display` error into the `String` shape Tauri IPC
/// commands return. Both `commands.rs` and `ocr_commands.rs` route
/// every `.map_err` through this.
pub fn err<E: std::fmt::Display>(e: E) -> String {
    e.to_string()
}

/// Clone the `Arc<Library>` out of the AppState mutex briefly and
/// drop the guard. The returned `Arc` keeps the library alive; long
/// operations against the library can then run without holding the
/// AppState lock, so other commands aren't blocked while a sync or
/// `open_document` is in flight.
///
/// The mutex is held only for the duration of one Arc clone (a few
/// instructions); the comment is the contract: callers should not
/// hold this Arc through a `.await` on user-bound I/O without first
/// considering whether a concurrent library swap is OK.
pub async fn lib_arc(state: &State<'_, AppState>) -> Result<Arc<Library>, String> {
    state
        .library
        .lock()
        .await
        .as_ref()
        .map(Arc::clone)
        .ok_or_else(|| "no library is open".to_string())
}
