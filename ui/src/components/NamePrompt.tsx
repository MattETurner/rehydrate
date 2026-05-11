import { useEffect, useRef, useState } from "react";

interface Props {
  /** Modal title, e.g. "New folder". */
  title: string;
  /** Optional explanation shown below the title. */
  subtitle?: string;
  placeholder: string;
  /** Submit button label in its idle state, e.g. "Create". */
  submitLabel: string;
  /** Submit button label while the request is in flight, e.g. "Creating…". */
  submitBusyLabel: string;
  /** Pre-filled value. Empty by default. */
  initialName?: string;
  onCancel: () => void;
  onSubmit: (name: string) => Promise<void>;
}

/// A bare-bones "ask for a name" modal. Used by the new-folder flow;
/// the rename flow has its own dialog because it pre-selects the name
/// and uses different copy.
export function NamePrompt({
  title,
  subtitle,
  placeholder,
  submitLabel,
  submitBusyLabel,
  initialName = "",
  onCancel,
  onSubmit,
}: Props) {
  const [name, setName] = useState(initialName);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);

  useEffect(() => {
    inputRef.current?.focus();
    inputRef.current?.select();
  }, []);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) {
      setError("Name can't be empty.");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onSubmit(trimmed);
    } catch (err) {
      setError(String(err));
      setBusy(false);
    }
  }

  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>{title}</h2>
        {subtitle && <p className="muted">{subtitle}</p>}
        <form onSubmit={submit}>
          <input
            ref={inputRef}
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            disabled={busy}
            placeholder={placeholder}
          />
          {error && <div className="error inline">{error}</div>}
          <div className="actions">
            <button type="button" onClick={onCancel} disabled={busy}>
              Cancel
            </button>
            <button type="submit" disabled={busy || !name.trim()}>
              {busy ? submitBusyLabel : submitLabel}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
