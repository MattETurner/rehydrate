// Tauri-side error strings reach the renderer as JSON-encoded strings
// — `String(e)` on the renderer side leaves them double-quoted (e.g.
// `"\"library locked: …\""`) or with tag JSON glommed onto the front
// when the Rust side uses `serde_json::to_string`. Worse, raw Rust
// `Display` messages can leak typed-error noise (`"NotFound(\"version
// 42\")"`) that nobody outside the crate should see.
//
// `formatError` is the one place we turn an unknown `e` into something
// the user should read. It:
//
// 1. Unwraps a value through one or two layers of JSON quoting.
// 2. Strips a leading `Some(...)` / `Err(...)` wrapper if a Rust
//    debug string slipped through.
// 3. Hands the result back as a single-line string.
//
// It deliberately doesn't try to map error tags to friendlier copy;
// that belongs in component-specific error handlers
// (`SettingsModal.tsx::parseOllamaUnconfigured`, etc.) and would tie
// this helper to every Rust error variant. The contract here is
// only: "make the raw thing readable."

export function formatError(e: unknown): string {
  if (e == null) return "Unknown error";
  if (e instanceof Error) return e.message;
  let s = typeof e === "string" ? e : safeStringify(e);
  s = unwrapJsonString(s);
  // Strip one layer of `Some("…")` / `Err("…")` if a Rust Debug
  // string sneaked through (this happens when a Rust command maps an
  // `Option` to its `Debug` representation by accident).
  const wrapped = /^(?:Some|Err)\((.*)\)$/.exec(s);
  if (wrapped) {
    s = unwrapJsonString(wrapped[1] ?? s);
  }
  return s.trim() || "Unknown error";
}

function safeStringify(v: unknown): string {
  try {
    return JSON.stringify(v);
  } catch {
    return String(v);
  }
}

function unwrapJsonString(s: string): string {
  // Try up to two passes of JSON unquoting. Tauri serialises the
  // command's `Err(String)` once, and a wrapping `.toString()` on a
  // JS Error often serialises again.
  for (let i = 0; i < 2; i++) {
    if (s.length >= 2 && s.startsWith('"') && s.endsWith('"')) {
      try {
        const parsed = JSON.parse(s);
        if (typeof parsed === "string") {
          s = parsed;
          continue;
        }
      } catch {
        // fall through
      }
    }
    break;
  }
  return s;
}
