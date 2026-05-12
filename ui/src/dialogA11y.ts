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
//
// Stacking model
// --------------
// When two modals are open at once (typical: a Confirm-on-top-of-
// Settings cascade), only the topmost one should hear Tab and
// Escape. The hook tracks a module-local stack of mounted instances
// and gates its document-level handler on "am I the top?". Without
// this, a Tab keypress while the Confirm was open would call
// `preventDefault()` and `focus()` in *both* handlers — the lower
// modal's handler would yank focus back out of the Confirm and
// onto whatever was at the bottom of the lower dialog. The audit
// caught this exact regression.

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

// Module-local stack of dialog instance ids. Each `useDialogA11y`
// instance pushes an id at mount and pops at unmount. The
// document-level keydown handler only runs its trap/escape logic
// when its own id is at the top of the stack — i.e. it's the
// most-recently-mounted live dialog.
let dialogStack: number[] = [];
let nextDialogId = 1;

export function useDialogA11y(opts: DialogA11yOptions = {}): DialogA11yResult {
  const titleId = useId();
  const rootRef = useRef<HTMLDivElement>(null);
  const previouslyFocused = useRef<HTMLElement | null>(null);
  const myId = useRef<number>(0);
  if (myId.current === 0) {
    myId.current = nextDialogId++;
  }

  // Hold `onEscape` in a ref so the keydown effect doesn't churn
  // on every render. Callers commonly pass an inline arrow
  // (`{ onEscape: () => onClose() }`) which changes identity each
  // render; without the ref, the keydown listener was re-bound on
  // every keystroke while a dialog was open. The ref lets the
  // listener subscribe once at mount and always read the latest
  // closure when it fires.
  const onEscapeRef = useRef(opts.onEscape);
  onEscapeRef.current = opts.onEscape;

  useEffect(() => {
    // Push our id; remember the previously-focused element so we
    // can restore it on unmount.
    const id = myId.current;
    dialogStack.push(id);
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
      dialogStack = dialogStack.filter((x) => x !== id);
      const el = previouslyFocused.current;
      if (el && el !== document.body && document.contains(el)) {
        el.focus();
      }
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    function isTopmost(): boolean {
      return dialogStack[dialogStack.length - 1] === myId.current;
    }
    function onKey(e: KeyboardEvent) {
      // Only the topmost mounted dialog handles Tab / Escape. A
      // Confirm rendered on top of Settings is itself the topmost
      // dialog (assuming the Confirm uses `useDialogA11y` — which
      // it does via its own `useDialogA11y` instance inside
      // `Confirm.tsx`); the Settings instance silently ignores
      // these keys until the Confirm unmounts.
      if (!isTopmost()) return;
      if (e.key === "Escape") {
        const cb = onEscapeRef.current;
        if (cb) {
          e.preventDefault();
          cb();
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
        // Snap it back onto the dialog and let the next Tab
        // proceed normally.
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
    // The handler reads `onEscape` via the ref, and `isTopmost`
    // closes over `myId.current` (stable from mount). No deps to
    // declare — this effect runs exactly once per mounted dialog.
  }, []);

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
