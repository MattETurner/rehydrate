import { useId, useRef, useState } from "react";

import { useDialogA11y } from "../dialogA11y";
import { formatError } from "../formatError";
import { classifySyncError, type SyncErrorKind } from "../humanizeError";

interface Props {
  onCancel: () => void;
  onSubmit: (password: string, remember: boolean) => Promise<void>;
}

// The dialog's headline copy only distinguishes three buckets, so we
// collapse the broader SyncErrorKind taxonomy at the use site.
type DialogErrorKind = "auth" | "network" | "other";

function dialogKindFromSync(k: SyncErrorKind): DialogErrorKind {
  if (k === "auth") return "auth";
  if (k === "network" || k === "disconnect") return "network";
  return "other";
}

export function PasswordDialog({ onCancel, onSubmit }: Props) {
  const [password, setPassword] = useState("");
  const [remember, setRemember] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [errorKind, setErrorKind] = useState<DialogErrorKind | null>(null);

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
      setErrorKind(dialogKindFromSync(classifySyncError(msg)));
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

function errorHeadline(kind: DialogErrorKind | null): string {
  if (kind === "auth") return "That password didn't work.";
  if (kind === "network") return "Couldn't reach the tablet.";
  return "Couldn't connect.";
}
