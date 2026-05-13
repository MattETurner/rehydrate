// Translate raw error strings from the Rust backend into copy that's
// actionable for a non-developer. The backend hands us stringly-typed
// errors (`device: io: broken pipe`, `device: authentication failed`,
// `host key for 10.11.99.1 has changed since …`, etc.). `formatError`
// strips the JSON-quoting layer, but the result is still developer
// vocabulary. This module pattern-matches on the message and rewrites
// to something the user can act on.
//
// Pattern matching by substring is intentionally loose — the Rust
// side may rephrase its errors over time, and we'd rather degrade
// to the raw string than show a stale humanised one. Anything that
// doesn't match falls through to `formatError`'s output verbatim.

import { formatError } from "./formatError";

export type SyncErrorKind =
  | "auth"
  | "network"
  | "host-key"
  | "disconnect"
  | "disk-full"
  | "cancelled"
  | "other";

export function classifySyncError(raw: string): SyncErrorKind {
  const lc = raw.toLowerCase();
  if (
    lc.includes("authentication failed") ||
    lc.includes("auth failed") ||
    lc.includes("no password stored")
  ) {
    return "auth";
  }
  if (lc.includes("host key") || lc.includes("known_hosts")) {
    return "host-key";
  }
  if (
    lc.includes("unreachable") ||
    lc.includes("connection refused") ||
    lc.includes("no route") ||
    lc.includes("timed out") ||
    lc.includes("connection timeout")
  ) {
    return "network";
  }
  if (
    lc.includes("broken pipe") ||
    lc.includes("connection reset") ||
    lc.includes("eof") ||
    lc.includes("unexpected end of file")
  ) {
    return "disconnect";
  }
  if (
    lc.includes("no space left") ||
    lc.includes("disk full") ||
    lc.includes("enospc")
  ) {
    return "disk-full";
  }
  if (lc.includes("cancelled") || lc.includes("canceled")) {
    return "cancelled";
  }
  return "other";
}

/// Turn an unknown error from a sync / pull / push command into a
/// short, user-facing sentence. The contract: the result is never
/// empty, never carries Rust type names, and tells the user either
/// what to try or what is going on.
export function humanizeSyncError(e: unknown): string {
  const raw = formatError(e);
  switch (classifySyncError(raw)) {
    case "auth":
      // Recovery path: click the device pill in the toolbar → Forget
      // password, then Connect again to re-enter it. (The pill only
      // shows Forget password when a password is stored, which is
      // exactly the case the user is in when auth fails after a
      // previous successful connect.)
      return "Tablet password rejected. Click the device pill (top-left) → Forget password, then Connect again to re-enter it.";
    case "host-key":
      // Recovery path: click the device pill → Forget host key. The
      // button only appears when a pin exists, which is exactly when
      // this error can fire.
      return "Tablet's host key changed since the last connect. If this is the same tablet you've always synced with (e.g. after a factory reset), click the device pill (top-left) → Forget host key. If it isn't your tablet, do NOT proceed.";
    case "network":
      return "Couldn't reach the tablet. Check the USB cable is plugged in and that the tablet is awake.";
    case "disconnect":
      return "Lost the connection to the tablet mid-sync. The cable may have come loose or the tablet went to sleep. Try again.";
    case "disk-full":
      return "Out of disk space. Free up some room and try again.";
    case "cancelled":
      return "Sync cancelled.";
    case "other":
    default:
      // Surface the raw message so we never hide a genuine clue from
      // the user, but tag it so they know it's the raw form.
      return `Sync failed: ${raw}`;
  }
}
