// Document-list selection + keyboard-focus state, extracted from
// App.tsx.
//
// Two cohabiting concepts the hook keeps disentangled:
//
//   - **Selection** (`selectedIds` + `selectMode`). The checkbox-style
//     batch that drives "Move selected" / "Archive selected" /
//     bulk-drag. Only meaningful in select mode; entering and
//     leaving the mode clears the batch so nothing visual lingers.
//
//   - **Keyboard cursor** (`focusId`). Where the keyboard's "current
//     row" is. Moves with arrow keys, tracks the last clicked row,
//     and feeds F2-rename / Space-quicklook. Importantly does NOT
//     paint the row-selected background — the inset accent bar is
//     its own visual idiom.
//
// `anchorId` is the third leg: the original click in a shift-extend
// range. Stays put while focus follows the user's finger to the new
// endpoint so subsequent ↑/↓ feels continuous from there.
//
// The hook owns the four state slots and the canonical mutators
// (`handleRowClick`, `toggleSelectMode`, `clearSelection`,
// `clearFocus`). The raw setters are also exposed so App.tsx's
// keyboard cascade (~120 references) can keep its direct
// imperative writes without paying a wrapping cost per site.

import { useCallback, useMemo, useState } from "react";

import type { DocumentSummary } from "../types";

export interface UseSelectionResult {
  selectedIds: Set<string>;
  anchorId: string | null;
  focusId: string | null;
  selectMode: boolean;
  /// Single "currently-targeted" id used by keyboard shortcuts
  /// (rename, quick-look, archive). In select-mode falls back to
  /// the only selected entry; otherwise tracks the keyboard cursor.
  selectedId: string | null;
  /// Imperative-mode mutators. Same shape as `useState` setters so
  /// App.tsx's keyboard cascade can call them directly.
  setSelectedIds: React.Dispatch<React.SetStateAction<Set<string>>>;
  setAnchorId: React.Dispatch<React.SetStateAction<string | null>>;
  setFocusId: React.Dispatch<React.SetStateAction<string | null>>;
  setSelectMode: React.Dispatch<React.SetStateAction<boolean>>;
  /// Canonical row-click handler. Encapsulates the "Cmd-click in
  /// default mode enters select mode" shortcut and the "shift-click
  /// extends from anchor" range pattern.
  handleRowClick: (
    d: DocumentSummary,
    list: DocumentSummary[],
    e: { metaKey: boolean; ctrlKey: boolean; shiftKey: boolean },
  ) => void;
  toggleSelectMode: () => void;
  clearSelection: () => void;
  clearFocus: () => void;
}

export function useSelection(
  openInViewer: (doc: DocumentSummary) => void,
): UseSelectionResult {
  const [selectedIds, setSelectedIds] = useState<Set<string>>(new Set());
  const [anchorId, setAnchorId] = useState<string | null>(null);
  const [focusId, setFocusId] = useState<string | null>(null);
  const [selectMode, setSelectMode] = useState(false);

  const selectedId = useMemo(
    () => focusId ?? (selectedIds.size === 1 ? [...selectedIds][0] : null),
    [focusId, selectedIds],
  );

  const handleRowClick = useCallback(
    (
      d: DocumentSummary,
      list: DocumentSummary[],
      e: { metaKey: boolean; ctrlKey: boolean; shiftKey: boolean },
    ) => {
      const id = d.document_id;
      if (!selectMode) {
        // Cmd-click while NOT in select mode is the discoverable
        // shortcut for "I want to start selecting" — flips select
        // mode on and checks this row.
        if (e.metaKey || e.ctrlKey) {
          setSelectMode(true);
          setSelectedIds(new Set([id]));
          setAnchorId(id);
          setFocusId(id);
          return;
        }
        // Plain click in default mode: open the doc, but also claim
        // keyboard focus on this row. The focus ring stays subtle
        // (no row-selection background) but it makes the typical
        // "click a row, then press F2 to rename" flow work —
        // without this, F2 was a no-op until the user pressed an
        // arrow key first to seed the cursor.
        setFocusId(id);
        setAnchorId(id);
        openInViewer(d);
        return;
      }
      // Select mode: click toggles, shift extends a range.
      if (e.shiftKey && anchorId) {
        const ai = list.findIndex((x) => x.document_id === anchorId);
        const bi = list.findIndex((x) => x.document_id === id);
        if (ai >= 0 && bi >= 0) {
          const [lo, hi] = ai < bi ? [ai, bi] : [bi, ai];
          setSelectedIds(
            new Set(list.slice(lo, hi + 1).map((x) => x.document_id)),
          );
          // Anchor stays on the original click; focus follows the
          // user's finger to the new endpoint so subsequent ↑/↓
          // feels continuous from there.
          setFocusId(id);
          return;
        }
      }
      setSelectedIds((prev) => {
        const next = new Set(prev);
        if (next.has(id)) next.delete(id);
        else next.add(id);
        return next;
      });
      setAnchorId(id);
      setFocusId(id);
    },
    // `openInViewer` is captured by reference; callers should
    // pass a stable callback (e.g. a useCallback in the parent).
    // The other deps are local state we read directly.
    [selectMode, anchorId, openInViewer],
  );

  const toggleSelectMode = useCallback(() => {
    setSelectMode((m) => {
      // Leaving select mode clears any selection AND the keyboard
      // cursor so nothing visual lingers from the mode.
      if (m) {
        setSelectedIds(new Set());
        setAnchorId(null);
        setFocusId(null);
      }
      return !m;
    });
  }, []);

  const clearSelection = useCallback(() => {
    setSelectedIds(new Set());
    setAnchorId(null);
  }, []);

  // Drop the keyboard cursor (the inset accent bar). Hooked up to
  // the Esc cascade — last resort once dialogs/drawers are closed.
  const clearFocus = useCallback(() => {
    setFocusId(null);
  }, []);

  return {
    selectedIds,
    anchorId,
    focusId,
    selectMode,
    selectedId,
    setSelectedIds,
    setAnchorId,
    setFocusId,
    setSelectMode,
    handleRowClick,
    toggleSelectMode,
    clearSelection,
    clearFocus,
  };
}
