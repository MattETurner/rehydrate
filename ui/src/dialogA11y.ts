// Shared accessibility helpers for modal/dialog components.
//
// Every modal in the app should:
//   - render its root with `role="dialog"`, `aria-modal="true"`, and an
//     `aria-labelledby` pointing at the visible title.
//   - trap focus inside the dialog (Tab/Shift+Tab cycles only within).
//   - restore focus to the element that opened the dialog when it closes.
//   - close on Escape, when the design permits.
//
// `useDialogA11y` packages all four. Components mount the hook, point
// it at their dialog root and at the initial-focus target, and the
// hook returns the aria props to spread onto the root.
//
// Why not a wrapper component? A few dialogs need to render extra
// nodes outside the dialog content (e.g. backdrops with click-to-
// close). A hook lets each component place the role props on
// whichever node is conceptually the dialog without forcing a
// particular DOM shape.

import { useEffect, useId, useRef } from "react";

export interface DialogA11yOptions {
  /** Called when the user presses Escape with focus inside the dialog. */
  onEscape?: () => void;
  /** Auto-focus this element after mount. Falls back to the first
   *  focusable descendant of the dialog root. */
  initialFocusRef?: React.RefObject<HTMLElement>;
}

export interface DialogA11yResult {
  /** Attach to the dialog root: spreads role/aria-modal/labelledby. */
  dialogProps: {
    role: "dialog";
    "aria-modal": "true";
    "aria-labelledby": string;
  };
  /** Attach to the dialog root via `ref`. */
  rootRef: React.RefObject<HTMLDivElement>;
  /** Set as the visible title's id. */
  titleId: string;
}

export function useDialogA11y(opts: DialogA11yOptions = {}): DialogA11yResult {
  const titleId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const previouslyFocused = useRef<HTMLElement | null>(null);

  useEffect(() => {
    previouslyFocused.current =
      document.activeElement instanceof HTMLElement
        ? document.activeElement
        : null;
    // Auto-focus the requested element, falling back to the first
    // focusable descendant of the dialog root.
    const explicit = opts.initialFocusRef?.current;
    if (explicit) {
      explicit.focus();
    } else if (rootRef.current) {
      const first = rootRef.current.querySelector<HTMLElement>(FOCUSABLE);
      first?.focus();
    }
    return () => {
      const el = previouslyFocused.current;
      if (el && el !== document.body && document.contains(el)) {
        el.focus();
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        if (opts.onEscape) {
          e.preventDefault();
          opts.onEscape();
        }
        return;
      }
      if (e.key !== "Tab") return;
      const root = rootRef.current;
      if (!root) return;
      const focusables = Array.from(
        root.querySelectorAll<HTMLElement>(FOCUSABLE),
      ).filter(
        (el) =>
          !el.hasAttribute("disabled") &&
          el.tabIndex !== -1 &&
          el.offsetParent !== null,
      );
      if (focusables.length === 0) return;
      const first = focusables[0];
      const last = focusables[focusables.length - 1];
      const active = document.activeElement as HTMLElement | null;
      if (!root.contains(active)) {
        // Focus escaped (browser quirk after backdrop click etc.).
        // Snap it back onto the dialog and let the next Tab proceed
        // normally.
        e.preventDefault();
        first.focus();
        return;
      }
      if (e.shiftKey && active === first) {
        e.preventDefault();
        last.focus();
      } else if (!e.shiftKey && active === last) {
        e.preventDefault();
        first.focus();
      }
    }
    document.addEventListener("keydown", onKey, true);
    return () => document.removeEventListener("keydown", onKey, true);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [opts.onEscape]);

  return {
    dialogProps: {
      role: "dialog",
      "aria-modal": "true",
      "aria-labelledby": titleId,
    },
    rootRef,
    titleId,
  };
}

const FOCUSABLE = [
  "a[href]",
  "button:not([disabled])",
  "input:not([disabled])",
  "select:not([disabled])",
  "textarea:not([disabled])",
  '[tabindex]:not([tabindex="-1"])',
].join(",");
