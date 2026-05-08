import {
  createContext,
  useCallback,
  useContext,
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

  const close = (ok: boolean) => {
    if (!pending) return;
    pending.resolve(ok);
    setPending(null);
  };

  const ctx = useMemo<ConfirmContextValue>(() => ({ confirm }), [confirm]);

  return (
    <ConfirmContext.Provider value={ctx}>
      {children}
      {pending && (
        <div className="modal-backdrop" onClick={() => close(false)}>
          <div
            className="modal"
            onClick={(e) => e.stopPropagation()}
            onKeyDown={(e) => {
              if (e.key === "Escape") close(false);
              if (e.key === "Enter") close(true);
            }}
          >
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
