import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useId,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

import { useDialogA11y } from "../dialogA11y";

export interface ConfirmOptions {
  title: string;
  body?: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  /** Marks the confirm button as destructive (red). */
  destructive?: boolean;
}

interface ConfirmContextValue {
  confirm: (opts: ConfirmOptions) => Promise<boolean>;
}

const ConfirmContext = createContext<ConfirmContextValue | null>(null);

export function useConfirm(): (opts: ConfirmOptions) => Promise<boolean> {
  const ctx = useContext(ConfirmContext);
  if (!ctx) throw new Error("useConfirm must be used inside <ConfirmHost>");
  return ctx.confirm;
}

interface PendingConfirm extends ConfirmOptions {
  resolve: (ok: boolean) => void;
}

export function ConfirmHost({ children }: { children: ReactNode }) {
  const [pending, setPending] = useState<PendingConfirm | null>(null);
  // Used to focus the confirm button after the modal mounts so Enter
  // confirms by default.
  const confirmBtnRef = useRef<HTMLButtonElement | null>(null);

  const confirm = useCallback((opts: ConfirmOptions) => {
    return new Promise<boolean>((resolve) => {
      setPending({ ...opts, resolve });
    });
  }, []);

  const close = useCallback(
    (ok: boolean) => {
      if (!pending) return;
      pending.resolve(ok);
      setPending(null);
    },
    [pending],
  );

  const ctx = useMemo<ConfirmContextValue>(() => ({ confirm }), [confirm]);

  return (
    <ConfirmContext.Provider value={ctx}>
      {children}
      {pending && (
        <ConfirmDialog
          pending={pending}
          confirmBtnRef={confirmBtnRef}
          onClose={close}
        />
      )}
    </ConfirmContext.Provider>
  );
}

function ConfirmDialog({
  pending,
  confirmBtnRef,
  onClose,
}: {
  pending: PendingConfirm;
  confirmBtnRef: React.MutableRefObject<HTMLButtonElement | null>;
  onClose: (ok: boolean) => void;
}) {
  // Destructive confirms get `alertdialog`; the rest get a regular
  // `dialog`. The two roles differ in how screen readers announce
  // them — `alertdialog` is read more assertively, matching the
  // higher-stakes nature of "delete forever / archive bulk / etc."
  const bodyId = useId();
  const role = pending.destructive ? "alertdialog" : "dialog";

  // Route Tab focus-trap, Escape, and focus-restoration through the
  // shared a11y hook. Critical for stacked-modal correctness: the
  // hook pushes this dialog onto its module-local stack so a
  // Confirm-on-top-of-Settings cascade keeps Tab confined to the
  // Confirm rather than letting the underlying Settings handler
  // yank focus back. The previous bespoke window-keydown handler
  // worked for Escape (via `stopImmediatePropagation`) but not for
  // Tab, which the audit caught as the focus-leak regression.
  const { dialogProps, rootRef, titleId } = useDialogA11y({
    onEscape: () => onClose(false),
  });

  // Enter-to-confirm is the only key the shared hook doesn't
  // handle. Stack-aware via capture-phase listening — the deeper
  // dialog's `useDialogA11y` push means we're guaranteed to be the
  // topmost when this fires. `stopImmediatePropagation` so any
  // accidental App-level Enter binding doesn't double-fire.
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key !== "Enter") return;
      // Don't grab Enter when the focus is inside a form field that
      // owns it (textarea, contenteditable). The two buttons are
      // the focused element via initialFocusRef → `confirmBtnRef`
      // (auto-focus on mount) and Tab moves focus among them; no
      // text input lives in the Confirm body today, but guard for
      // future additions.
      const active = document.activeElement as HTMLElement | null;
      if (active && (active.tagName === "TEXTAREA" || active.isContentEditable)) {
        return;
      }
      e.preventDefault();
      e.stopImmediatePropagation();
      onClose(true);
    }
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [onClose]);

  // Override the hook's auto-focus to target the confirm button
  // specifically — Enter-to-confirm is the dominant flow, so the
  // primary action gets the keyboard focus on mount. The hook's
  // default behavior would auto-focus the first focusable
  // descendant (Cancel, in our DOM order), which would be wrong.
  useEffect(() => {
    confirmBtnRef.current?.focus();
  }, [confirmBtnRef]);

  return (
    <div className="modal-backdrop" onClick={() => onClose(false)}>
      <div
        className="modal"
        onClick={(e) => e.stopPropagation()}
        ref={rootRef}
        {...dialogProps}
        // Override the `role` from dialogProps so destructive
        // confirms get `alertdialog` semantics (assertive screen-
        // reader announcement) while the rest stay as `dialog`.
        role={role}
        aria-labelledby={titleId}
        aria-describedby={pending.body ? bodyId : undefined}
      >
        <h2 id={titleId}>{pending.title}</h2>
        {pending.body && (
          <p id={bodyId} className="dialog-body">
            {pending.body}
          </p>
        )}
        <div className="actions actions-spaced">
          <button onClick={() => onClose(false)}>
            {pending.cancelLabel ?? "Cancel"}
          </button>
          <button
            ref={(el) => {
              confirmBtnRef.current = el;
            }}
            className={pending.destructive ? "primary danger" : "primary"}
            onClick={() => onClose(true)}
          >
            {pending.confirmLabel ?? "Confirm"}
          </button>
        </div>
      </div>
    </div>
  );
}
