import { useEffect, useRef, useState } from "react";
import type { DeviceState } from "../types";
import { ipc } from "../ipc";
import { Icon } from "./Icon";

type Phase = "idle" | "syncing" | "failed";

interface Props {
  state: DeviceState | null;
  /** External signal — toolbar tells us when a sync is running. */
  phase?: Phase;
  onConnect?: () => void;
  onDisconnect?: () => Promise<void> | void;
}

/* Compact device status. Click opens a small popover with details and
 * a Disconnect button. The pill animates between states with a tinted
 * dot — green when connected, amber when reachable, blue+pulse during
 * sync, red on failure, grey when offline. */
export function StatusPill({ state, phase = "idle", onConnect, onDisconnect }: Props) {
  const [open, setOpen] = useState(false);
  const hostRef = useRef<HTMLSpanElement | null>(null);

  // Click-outside / escape close.
  useEffect(() => {
    if (!open) return;
    function onDown(e: MouseEvent) {
      if (!hostRef.current) return;
      if (!hostRef.current.contains(e.target as Node)) setOpen(false);
    }
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape") setOpen(false);
    }
    window.addEventListener("mousedown", onDown);
    window.addEventListener("keydown", onKey);
    return () => {
      window.removeEventListener("mousedown", onDown);
      window.removeEventListener("keydown", onKey);
    };
  }, [open]);

  const label = pickLabel(state, phase);

  return (
    <span ref={hostRef} className="popover-host">
      {/*
        Real `<button>` rather than a `role="button"` span so screen
        readers, focus rings, and Enter/Space handling all come for
        free. The visual is identical because `.pill.clickable`
        already styles a clickable pill.
      */}
      <button
        type="button"
        className={`pill ${label.toneClass} clickable pill-button`}
        onClick={() => state && setOpen((v) => !v)}
        aria-haspopup="dialog"
        aria-expanded={open ? "true" : "false"}
        title={label.title}
        disabled={!state}
      >
        <span className={`pill-dot${label.live ? " live" : ""}`} />
        {label.text}
      </button>
      {open && state && (
        <DevicePopover
          state={state}
          phase={phase}
          onConnect={() => {
            setOpen(false);
            onConnect?.();
          }}
          onDisconnect={async () => {
            setOpen(false);
            await onDisconnect?.();
          }}
          onForgetPassword={async () => {
            await ipc.forgetDevicePassword();
            setOpen(false);
          }}
        />
      )}
    </span>
  );
}

function DevicePopover({
  state,
  phase,
  onConnect,
  onDisconnect,
  onForgetPassword,
}: {
  state: DeviceState;
  phase: Phase;
  onConnect: () => void;
  onDisconnect: () => Promise<void>;
  onForgetPassword: () => Promise<void>;
}) {
  const [busy, setBusy] = useState(false);
  return (
    <div className="popover" onClick={(e) => e.stopPropagation()}>
      <div style={{ marginBottom: "var(--space-3)" }}>
        <div style={{ fontWeight: 600, marginBottom: 2 }}>
          {state.info?.model ?? "reMarkable"}
        </div>
        <div className="muted small">{statusSentence(state, phase)}</div>
      </div>
      <dl className="kv">
        {state.info?.serial && (
          <>
            <dt>Serial</dt>
            <dd className="mono small">{state.info.serial}</dd>
          </>
        )}
        {state.info?.software_version && (
          <>
            <dt>Software</dt>
            <dd className="small">{state.info.software_version}</dd>
          </>
        )}
        <dt>Endpoint</dt>
        <dd className="mono small">10.11.99.1</dd>
        <dt>Password</dt>
        <dd className="small">
          {state.has_stored_password ? "Saved in keychain" : "Not saved"}
        </dd>
      </dl>
      <div
        className="menu-divider"
        style={{ margin: "var(--space-3) 0", height: 1, background: "var(--border)" }}
      />
      <div style={{ display: "flex", gap: "var(--space-2)", flexWrap: "wrap" }}>
        {state.connected ? (
          <button
            onClick={async () => {
              setBusy(true);
              try {
                await onDisconnect();
              } finally {
                setBusy(false);
              }
            }}
            disabled={busy}
          >
            <Icon name="unplug" /> Disconnect
          </button>
        ) : (
          <button
            className="primary"
            onClick={onConnect}
            disabled={!state.reachable}
          >
            <Icon name="plug" /> Connect
          </button>
        )}
        {state.has_stored_password && (
          <button onClick={onForgetPassword}>
            Forget password
          </button>
        )}
      </div>
    </div>
  );
}

function pickLabel(
  state: DeviceState | null,
  phase: Phase,
): { text: string; title: string; toneClass: string; live: boolean } {
  if (!state) {
    return { text: "Loading…", title: "Loading device state", toneClass: "", live: false };
  }
  if (phase === "syncing") {
    return {
      text: "Syncing…",
      title: "Sync in progress",
      toneClass: "pill-accent",
      live: true,
    };
  }
  if (phase === "failed") {
    return {
      text: "Sync failed",
      title: "Last sync failed",
      toneClass: "pill-err",
      live: false,
    };
  }
  if (state.connected && state.info) {
    return {
      text: `Connected · ${state.info.model}`,
      title: state.info.serial ? `Serial ${state.info.serial}` : "Connected",
      toneClass: "pill-ok",
      live: false,
    };
  }
  if (state.reachable) {
    return {
      text: "Tablet detected",
      title: "Tablet plugged in but not connected",
      toneClass: "pill-warn",
      live: false,
    };
  }
  return { text: "No tablet", title: "No tablet plugged in", toneClass: "", live: false };
}

function statusSentence(state: DeviceState, phase: Phase): string {
  if (phase === "syncing") return "Syncing now";
  if (phase === "failed") return "The last sync did not finish";
  if (state.connected) return "Connected over USB";
  if (state.reachable) return "Tablet detected — not connected";
  return "Plug in your tablet over USB";
}
