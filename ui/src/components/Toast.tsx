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

export type ToastTone = "info" | "ok" | "warn" | "err";

export interface ToastInput {
  body: ReactNode;
  tone?: ToastTone;
  /** ms — 0 disables auto-dismiss. */
  duration?: number;
  /** Optional action button. Click resolves and dismisses the toast. */
  action?: {
    label: string;
    onClick: () => void | Promise<void>;
  };
}

interface ToastInstance extends ToastInput {
  id: number;
  leaving: boolean;
}

interface ToastContextValue {
  show: (input: ToastInput) => number;
  dismiss: (id: number) => void;
}

const ToastContext = createContext<ToastContextValue | null>(null);

export function useToast(): ToastContextValue {
  const ctx = useContext(ToastContext);
  if (!ctx) throw new Error("useToast must be used inside <Toaster>");
  return ctx;
}

export function Toaster({ children }: { children: ReactNode }) {
  const [toasts, setToasts] = useState<ToastInstance[]>([]);
  const idRef = useRef(0);
  const timersRef = useRef<Map<number, ReturnType<typeof setTimeout>>>(new Map());

  const dismiss = useCallback((id: number) => {
    // Two-phase removal: mark leaving, then drop after the slide-out
    // animation completes. Keeps the unmount visually smooth.
    setToasts((prev) => prev.map((t) => (t.id === id ? { ...t, leaving: true } : t)));
    const t = timersRef.current.get(id);
    if (t) {
      clearTimeout(t);
      timersRef.current.delete(id);
    }
    setTimeout(() => {
      setToasts((prev) => prev.filter((t) => t.id !== id));
    }, 220);
  }, []);

  const show = useCallback(
    (input: ToastInput) => {
      const id = ++idRef.current;
      const tone = input.tone ?? "info";
      const duration = input.duration ?? 4200;
      setToasts((prev) => [...prev, { ...input, id, tone, leaving: false }]);
      if (duration > 0) {
        const handle = setTimeout(() => dismiss(id), duration);
        timersRef.current.set(id, handle);
      }
      return id;
    },
    [dismiss],
  );

  useEffect(() => {
    // Snapshot the Map ref into a local at effect-mount time so the
    // cleanup function references the same instance it observed
    // (lint flags reading `.current` directly in cleanup because the
    // ref may have been swapped between mount and unmount). The
    // Toaster only ever stores `Map`s in this ref, so the snapshot
    // is safe.
    const timers = timersRef.current;
    return () => {
      for (const t of timers.values()) clearTimeout(t);
      timers.clear();
    };
  }, []);

  const ctx = useMemo<ToastContextValue>(() => ({ show, dismiss }), [show, dismiss]);

  // Two live regions: a polite one for `info` / `ok` / `warn`
  // (announced when the user is idle), and an assertive one for
  // `err` (announced immediately, even mid-typing). Error toasts
  // auto-dismiss in 4.2 s, so without `aria-live="assertive"` screen
  // reader users miss them entirely.
  const polite = toasts.filter((t) => t.tone !== "err");
  const assertive = toasts.filter((t) => t.tone === "err");

  const renderToast = (t: ToastInstance) => (
    <div
      key={t.id}
      className={`toast toast-${t.tone}${t.leaving ? " leaving" : ""}`}
    >
      <div className="toast-body">{t.body}</div>
      {t.action && (
        <button
          className="toast-action"
          onClick={async () => {
            try {
              await t.action!.onClick();
            } finally {
              dismiss(t.id);
            }
          }}
        >
          {t.action.label}
        </button>
      )}
      <button
        className="close"
        aria-label="Dismiss notification"
        onClick={() => dismiss(t.id)}
      >
        ×
      </button>
    </div>
  );

  return (
    <ToastContext.Provider value={ctx}>
      {children}
      {/*
        Single outer anchor so the polite + assertive stacks
        visually merge into one column. Both were `.toaster`
        previously and shared the same `position: fixed`
        coordinates, which painted them on top of each other.
        The wrapper owns the position; each child stack flows
        normally inside.
      */}
      <div className="toaster-anchor">
        <div
          className="toaster"
          role="status"
          aria-live="polite"
          aria-atomic="false"
        >
          {polite.map(renderToast)}
        </div>
        <div
          className="toaster toaster-assertive"
          role="alert"
          aria-live="assertive"
          aria-atomic="false"
        >
          {assertive.map(renderToast)}
        </div>
      </div>
    </ToastContext.Provider>
  );
}
