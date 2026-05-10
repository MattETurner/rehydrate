import {
  createContext,
  useCallback,
  useContext,
  useEffect,
  useMemo,
  useRef,
  useState,
  type ReactNode,
} from "react";

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

  // Capture-phase window listener so Esc/Enter handle the topmost
  // Confirm BEFORE the App's bubble-phase global cascade fires. Without
  // this, pressing Esc with a Confirm-on-top-of-Drawer would close the
  // drawer underneath as well as the Confirm. `stopImmediatePropagation`
  // also prevents anything else listening on `window` (e.g. dialog
  // local handlers) from firing — which is what we want; the Confirm
  // is unambiguously the top of the modal stack.
  useEffect(() => {
    if (!pending) return;
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") {
        e.preventDefault();
        e.stopImmediatePropagation();
        close(false);
      } else if (e.key === "Enter") {
        e.preventDefault();
        e.stopImmediatePropagation();
        close(true);
      }
    }
    window.addEventListener("keydown", onKey, true);
    return () => window.removeEventListener("keydown", onKey, true);
  }, [pending, close]);

  const ctx = useMemo<ConfirmContextValue>(() => ({ confirm }), [confirm]);

  return (
    <ConfirmContext.Provider value={ctx}>
      {children}
      {pending && (
        <div className="modal-backdrop" onClick={() => close(false)}>
          <div className="modal" onClick={(e) => e.stopPropagation()}>
            <h2>{pending.title}</h2>
            {pending.body && (
              <p style={{ marginTop: 8, color: "var(--muted)" }}>
                {pending.body}
              </p>
            )}
            <div
              className="actions"
              style={{ marginTop: "var(--space-4)" }}
            >
              <button onClick={() => close(false)}>
                {pending.cancelLabel ?? "Cancel"}
              </button>
              <button
                ref={(el) => {
                  confirmBtnRef.current = el;
                  if (el) el.focus();
                }}
                className={pending.destructive ? "primary danger" : "primary"}
                onClick={() => close(true)}
              >
                {pending.confirmLabel ?? "Confirm"}
              </button>
            </div>
          </div>
        </div>
      )}
    </ConfirmContext.Provider>
  );
}
