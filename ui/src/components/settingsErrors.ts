// Helpers for parsing tagged-error JSON shapes the backend returns
// from OCR + publish commands.
//
// The Rust side wraps these as `Err(serde_json::to_string(&payload)?)`
// so the renderer can route the user to the right place (Settings →
// Ollama tab on `ollama_unconfigured`, Settings → Publishing tab on
// `publish_unconfigured`) instead of toasting an opaque message.
// Tauri's IPC machinery wraps `Err(String)` in another layer of JSON
// quoting, so the helpers below try both forms.

/// Client-side URL shape validation. Matches the Rust side's
/// `validate_remote_url` rules at the surface (scheme + host present)
/// without trying to replicate the IP-range checks — the backend
/// remains authoritative, this just catches obvious typos so the
/// user doesn't bounce through a save → IPC round-trip just to learn
/// they forgot a "/" or a scheme.
export function validateUrlShape(url: string): string | null {
  if (!url) return "URL is required.";
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return "URL doesn't look right. Did you include `https://`?";
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    return "URL must start with http:// or https://.";
  }
  if (!parsed.hostname) {
    return "URL must have a host.";
  }
  return null;
}

/// Parse `{ kind: "ollama_unconfigured", base_url, model, message }`
/// from a `transcribe_document` error. Returns `null` when the error
/// is anything else — callers should fall back to a generic toast.
export function parseOllamaUnconfigured(
  raw: unknown,
): { kind: "ollama_unconfigured"; message: string } | null {
  const text =
    typeof raw === "string"
      ? raw
      : raw instanceof Error
        ? raw.message
        : null;
  if (!text) return null;
  for (const candidate of [text, safeUnquote(text)]) {
    try {
      const v = JSON.parse(candidate);
      if (v && v.kind === "ollama_unconfigured") {
        return { kind: "ollama_unconfigured", message: v.message ?? text };
      }
    } catch {
      // not JSON; fall through
    }
  }
  return null;
}

/// As above for `publish_unconfigured`.
export function parsePublishUnconfigured(
  raw: unknown,
): { kind: "publish_unconfigured"; target: string; message: string } | null {
  const text =
    typeof raw === "string"
      ? raw
      : raw instanceof Error
        ? raw.message
        : null;
  if (!text) return null;
  for (const candidate of [text, safeUnquote(text)]) {
    try {
      const v = JSON.parse(candidate);
      if (v && v.kind === "publish_unconfigured") {
        return {
          kind: "publish_unconfigured",
          target: v.target ?? "",
          message: v.message ?? text,
        };
      }
    } catch {
      // not JSON
    }
  }
  return null;
}

function safeUnquote(s: string): string {
  if (s.length >= 2 && s.startsWith('"') && s.endsWith('"')) {
    try {
      return JSON.parse(s) as string;
    } catch {
      return s;
    }
  }
  return s;
}
