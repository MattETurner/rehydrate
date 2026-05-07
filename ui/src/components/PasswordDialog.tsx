import { useState } from "react";

interface Props {
  onCancel: () => void;
  onSubmit: (password: string) => Promise<void>;
}

export function PasswordDialog({ onCancel, onSubmit }: Props) {
  const [password, setPassword] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    if (!password) return;
    setBusy(true);
    setError(null);
    try {
      await onSubmit(password);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>Connect to reMarkable</h2>
        <p className="muted">
          Find the password on the tablet at{" "}
          <strong>Settings → Help → Copyrights and licenses</strong>. It's
          shown at the bottom of the page. We'll store it in your OS keychain
          so you only need to enter it once.
        </p>
        <form onSubmit={submit}>
          <input
            type="password"
            autoFocus
            placeholder="Device password"
            value={password}
            onChange={(e) => setPassword(e.target.value)}
            disabled={busy}
          />
          {error && <div className="error inline">{error}</div>}
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
