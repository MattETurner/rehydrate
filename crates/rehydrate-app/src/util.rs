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
use secrecy::{ExposeSecret, SecretString};
use serde::Deserialize;
use tauri::State;

use crate::state::AppState;

/// Maximum byte length accepted from the renderer for any IPC
/// secret (tablet password, admin API key, application password,
/// …). Real-world values are dozens of bytes; capping at 256 stops
/// a hostile or buggy renderer from pushing a multi-MB payload
/// into the keychain or across the IPC bus.
pub const IPC_SECRET_MAX_BYTES: usize = 256;

/// Newtype wrapper for secret values that arrive via Tauri IPC.
/// Two guarantees over a bare `String` parameter:
///
/// 1. The Rust type system flags this argument as a secret. A future
///    Debug-derive on a containing struct or a tracing instrument
///    that prints command arguments produces `IpcSecret([REDACTED])`
///    instead of the plaintext.
/// 2. Deserialization length-caps the input at `IPC_SECRET_MAX_BYTES`
///    so a malformed payload can't fill the keychain or pump a
///    multi-MB string through the IPC layer.
///
/// The bytes are still allocated in a plain `String` *inside* the
/// deserializer (serde's contract), but the lifetime is tightly
/// scoped — the caller of the command sees only `IpcSecret`, so it
/// can't accidentally print or persist the plaintext outside the
/// explicit `into_secret()` / `expose()` path.
#[derive(Clone)]
pub struct IpcSecret(SecretString);

impl IpcSecret {
    pub fn into_secret(self) -> SecretString {
        self.0
    }
    pub fn expose(&self) -> &str {
        self.0.expose_secret()
    }
}

impl std::fmt::Debug for IpcSecret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("IpcSecret([REDACTED])")
    }
}

impl<'de> Deserialize<'de> for IpcSecret {
    fn deserialize<D>(d: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        use serde::de::Error as _;
        let s = String::deserialize(d)?;
        if s.len() > IPC_SECRET_MAX_BYTES {
            return Err(D::Error::custom(format!(
                "secret value too long ({} bytes, max {IPC_SECRET_MAX_BYTES})",
                s.len()
            )));
        }
        Ok(IpcSecret(SecretString::from(s)))
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ipc_secret_debug_is_redacted() {
        let s: IpcSecret = serde_json::from_str(r#""hunter2""#).unwrap();
        let debug = format!("{:?}", s);
        assert!(
            !debug.contains("hunter2"),
            "Debug must NOT contain the plaintext, got: {debug}",
        );
        assert!(debug.contains("REDACTED"));
        // Sanity: the value is still accessible via the explicit
        // `expose()` path.
        assert_eq!(s.expose(), "hunter2");
    }

    #[test]
    fn ipc_secret_rejects_oversize_input() {
        let oversize = format!(r#""{}""#, "a".repeat(IPC_SECRET_MAX_BYTES + 1));
        let r: Result<IpcSecret, _> = serde_json::from_str(&oversize);
        assert!(
            r.is_err(),
            "deserialization must reject inputs over the cap",
        );
    }

    #[test]
    fn ipc_secret_accepts_at_cap() {
        let at_cap = format!(r#""{}""#, "a".repeat(IPC_SECRET_MAX_BYTES));
        let r: Result<IpcSecret, _> = serde_json::from_str(&at_cap);
        assert!(r.is_ok());
    }
}
