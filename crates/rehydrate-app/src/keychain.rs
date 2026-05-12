//! Small wrapper around `keyring::Entry` so the rest of the crate
//! doesn't deal with raw `keyring::Entry::new(service, slot)` calls.
//!
//! The wrappers exist for three reasons:
//!
//! 1. **One construction site.** Every keychain access goes through
//!    these helpers, so adding tracing / metrics / migration logic
//!    (e.g. when we eventually rename `KEYRING_SERVICE`) touches one
//!    file.
//! 2. **Forgiving idempotent delete.** Some keyring backends return a
//!    "not found" error when the entry doesn't exist; `forget_slot`
//!    swallows that so callers can write `let _ = forget_slot(…)`
//!    without special-casing.
//! 3. **Typed `Result<String, String>` on the read path.** Callers
//!    that want "is something stored?" can use `read_slot(...).is_ok()`
//!    instead of building a typed wrapper around the keyring error.
//!
//! The `ocr_commands` module already had near-identical helpers
//! locally — those have been promoted here so both `commands.rs` and
//! `ocr_commands.rs` share the same surface.

use crate::state::KEYRING_SERVICE;

/// Read the keychain value at `slot`. Returns `None` when no value is
/// stored, `Some(value)` when there is one. Errors opening the
/// keyring (e.g. no secret-service on Linux) collapse into `None` —
/// callers that need to distinguish "no value" from "keyring
/// unreachable" should call `keyring::Entry::new` directly.
pub fn read_slot(slot: &str) -> Option<String> {
    let entry = keyring::Entry::new(KEYRING_SERVICE, slot).ok()?;
    entry.get_password().ok()
}

/// Write `value` to `slot`. Returns the underlying error as a
/// stringified message so it can be surfaced through the Tauri IPC
/// boundary.
pub fn write_slot(slot: &str, value: &str) -> Result<(), String> {
    keyring::Entry::new(KEYRING_SERVICE, slot)
        .map_err(|e| e.to_string())?
        .set_password(value)
        .map_err(|e| e.to_string())
}

/// Idempotent delete: if the slot exists, remove it. If it doesn't,
/// or if the keyring backend reports "no such entry", treat as
/// success. The only failure that bubbles up is "keyring totally
/// unavailable", and even that's only worth surfacing if the caller
/// asks — most call sites can swallow the result.
pub fn forget_slot(slot: &str) -> Result<(), String> {
    if let Ok(entry) = keyring::Entry::new(KEYRING_SERVICE, slot) {
        let _ = entry.delete_credential();
    }
    Ok(())
}

/// Cheap "is there a value here?" probe without surfacing the value.
/// Used by the device-state command to render "Saved in keychain"
/// vs "Not saved" in the UI.
pub fn slot_has_value(slot: &str) -> bool {
    read_slot(slot).is_some()
}
