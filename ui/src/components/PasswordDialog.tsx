import { useId, useRef, useState } from "react";

import { useDialogA11y } from "../dialogA11y";
import { formatError } from "../formatError";

interface Props {
  onCancel: () => void;
  onSubmit: (password: string, remember: boolean) => Promise<void>;
}

export function PasswordDialog({ onCancel, onSubmit }: Props) {
  const [password, setPassword] = useState("");
  const [remember, setRemember] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [errorKind, setErrorKind] = useState<"auth" | "network" | "other" | null>(null);

  const descId = useId();
  const errId = useId();
  const passwordInputRef = useRef<HTMLInputElement>(null);
  const { dialogProps, rootRef, titleId } = useDialogA11y({
    onEscape: () => {
      if (!busy) onCancel();
    },
    initialFocusRef: passwordInputRef,
  });

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!password) return;
    setBusy(true);
    setError(null);
    setErrorKind(null);
    try {
      await onSubmit(password, remember);
    } catch (e) {
      const msg = formatError(e);
      setError(msg);
      setErrorKind(classifyConnectError(msg));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="modal-backdrop" onClick={() => !busy && onCancel()}>
      <div
        className="modal"
        onClick={(e) => e.stopPropagation()}
        ref={rootRef}
        {...dialogProps}
        aria-describedby={descId}
      >
        <h2 id={titleId}>Connect to reMarkable</h2>
        <p id={descId} className="muted">
          Find the password on the tablet at{" "}
          <strong>Settings → Help → Copyrights and licenses</strong>. It's
          shown at the bottom of the page.
        </p>
        <form onSubmit={submit}>
          <input
            ref={passwordInputRef}
            type="password"
            aria-label="Device password"
            placeholder="Device password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            disabled={busy}
            aria-invalid={error ? "true" : "false"}
            aria-describedby={error ? errId : undefined}
          />
          <label className="checkbox-row">
            <input
              type="checkbox"
              checked={remember}
              onChange={(e) => setRemember(e.target.checked)}
              disabled={busy}
            />
            <span>Remember in keychain</span>
          </label>
          {error && (
            <div id={errId} className="error inline" role="alert">
              <strong>{errorHeadline(errorKind)}</strong>
              <span> {error}</span>
              {errorKind === "network" && (
                <div className="muted" style={{ marginTop: 4 }}>
                  Check the USB cable, then try again.
                </div>
              )}
              {errorKind === "auth" && (
                <div className="muted" style={{ marginTop: 4 }}>
                  Verify the password on the tablet at{" "}
                  <strong>
                    Settings → Help → Copyrights and licenses
                  </strong>
                  .
                </div>
              )}
            </div>
          )}
          <div className="actions">
            <button type="button" onClick={onCancel} disabled={busy}>
              Cancel
            </button>
            <button type="submit" disabled={busy || !password}>
              {busy ? "Connecting…" : "Connect"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}

/// Distinguish the three commonly-confused failure modes so the dialog can
/// offer a recovery hint specific to each. The backend hands us a single
/// stringly-typed error (`SshDevice::connect` → `DeviceError` → `String`);
/// matching on the message text keeps the IPC contract loose.
function classifyConnectError(msg: string): "auth" | "network" | "other" {
  const lc = msg.toLowerCase();
  if (
    lc.includes("authentication failed") ||
    lc.includes("auth failed") ||
    lc.includes("no password stored")
  ) {
    return "auth";
  }
  if (
    lc.includes("unreachable") ||
    lc.includes("connection refused") ||
    lc.includes("no route") ||
    lc.includes("timed out") ||
    lc.includes("network")
  ) {
    return "network";
  }
  if (lc.includes("host key") || lc.includes("known_hosts")) {
    return "other";
  }
  return "other";
}

function errorHeadline(kind: "auth" | "network" | "other" | null): string {
  if (kind === "auth") return "That password didn't work.";
  if (kind === "network") return "Couldn't reach the tablet.";
  return "Couldn't connect.";
}
