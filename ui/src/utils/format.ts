// Shared user-facing formatters. Duplicated across App.tsx,
// HistoryDrawer.tsx, SyncDrawer.tsx, QuickLook.tsx before this file
// existed — those copies disagreed on edge cases (empty input, NaN
// dates, sub-KB sizes). Centralising means one truth.

/// Convert a doc-type string out of the device's `DocumentType.*` shape
/// into something a person would read.
export function prettyType(s: string): string {
  if (s === "Notebook") return "Notebook";
  if (s === "DocumentType.Pdf") return "PDF";
  if (s === "DocumentType.Epub") return "EPUB";
  if (s === "Folder") return "Folder";
  return s;
}

/// ISO-8601 / RFC-3339 → local time. Empty/invalid inputs yield "—"
/// rather than the raw string so the UI never shows a half-formatted
/// date.
export function prettyDate(iso: string | null | undefined): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "—";
  return d.toLocaleString();
}

/// Bytes → human-readable. Uses 1024-based units to match what the
/// macOS Finder shows; the values reHydrate displays are typically
/// PDF/EPUB sizes which users compare against Finder.
export function formatBytes(n: number | null | undefined): string {
  if (n == null || !Number.isFinite(n) || n < 0) return "—";
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(1)} ${units[i]}`;
}

/// Relative "2 minutes ago" — used in the Sync drawer status row and
/// the History timeline. Falls through to `prettyDate` for anything
/// older than a day, which is a more useful anchor than "yesterday at
/// 8 hours ago".
export function prettyRelative(iso: string | null | undefined): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return "—";
  const seconds = Math.round((Date.now() - d.getTime()) / 1000);
  if (seconds < 30) return "just now";
  if (seconds < 60) return `${seconds}s ago`;
  const minutes = Math.round(seconds / 60);
  if (minutes < 60) return `${minutes}m ago`;
  const hours = Math.round(minutes / 60);
  if (hours < 24) return `${hours}h ago`;
  return prettyDate(iso);
}
