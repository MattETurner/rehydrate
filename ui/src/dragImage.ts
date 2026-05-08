/* Custom drag preview helper.
 *
 * The browser's default drag image is a translucent slice of the
 * source element — usually a half-cropped table row. We render our own
 * card off-screen, hand it to `setDragImage`, and clear it after the
 * browser has snapshotted. */

import type { DragEvent as ReactDragEvent } from "react";

let host: HTMLDivElement | null = null;

function ensureHost(): HTMLDivElement {
  if (host && host.isConnected) return host;
  const el = document.createElement("div");
  // Lives at -1000,-1000 via CSS .drag-preview; the browser still
  // captures the rendered bitmap before the native drag begins.
  el.className = "drag-preview";
  document.body.appendChild(el);
  host = el;
  return el;
}

export function setCustomDragImage(
  e: ReactDragEvent,
  label: string,
  count: number,
  iconHtml: string,
) {
  const node = ensureHost();
  // Inline an SVG plus a label. Keep it markup-light: this is the drag
  // image, not a live component.
  node.innerHTML = `${iconHtml}<span style="overflow:hidden;text-overflow:ellipsis">${escapeHtml(label)}</span>${
    count > 1
      ? `<span style="margin-left:8px;padding:1px 8px;border-radius:999px;background:var(--accent-bg);color:var(--accent);font-weight:600">×${count}</span>`
      : ""
  }`;
  e.dataTransfer.setDragImage(node, 14, 14);
  // Drop preview content at the next paint so it doesn't linger.
  requestAnimationFrame(() => {
    if (host) host.innerHTML = "";
  });
}

function escapeHtml(s: string): string {
  return s
    .replace(/&/g, "&amp;")
    .replace(/</g, "&lt;")
    .replace(/>/g, "&gt;")
    .replace(/"/g, "&quot;");
}

/** Inline SVG for the drag preview. Kept in a string so we don't
 *  re-render React for the off-screen node. */
export const DRAG_ICON_SVG = `<svg class="icon-svg" viewBox="0 0 16 16" width="14" height="14" aria-hidden="true"><rect x="2" y="3" width="3" height="10" rx="0.6"/><rect x="6.5" y="3" width="3" height="10" rx="0.6"/><path d="M11 4.4 L13.6 4 L14 13 L11.4 13.4 Z"/></svg>`;
