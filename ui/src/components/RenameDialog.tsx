import { useEffect, useRef, useState } from "react";

interface Props {
  title: string;
  /** What we're renaming, e.g. "document" or "folder". Used in the
   *  subtitle and submit button. */
  kind: "document" | "folder";
  initialName: string;
  onCancel: () => void;
  onSubmit: (newName: string) => Promise<void>;
}

export function RenameDialog({
  title,
  kind,
  initialName,
  onCancel,
  onSubmit,
}: Props) {
  const [name, setName] = useState(initialName);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const inputRef = useRef<HTMLInputElement | null>(null);

  // Pre-select the current name (without extension when there's a dot
  // before the last segment) so the user can just start typing.
  useEffect(() => {
    const el = inputRef.current;
    if (!el) return;
    el.focus();
    const dot = initialName.lastIndexOf(".");
    if (dot > 0 && dot < initialName.length - 1) {
      el.setSelectionRange(0, dot);
    } else {
      el.select();
    }
  }, [initialName]);

  async function submit(e: React.FormEvent) {
    e.preventDefault();
    const trimmed = name.trim();
    if (!trimmed) {
      setError("Name can't be empty.");
      return;
    }
    if (trimmed === initialName.trim()) {
      onCancel();
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await onSubmit(trimmed);
    } catch (err) {
      setError(String(err));
    } finally {
      setBusy(false);
    }
  }

  return (
    <div className="modal-backdrop" onClick={onCancel}>
      <div className="modal" onClick={(e) => e.stopPropagation()}>
        <h2>{title}</h2>
        <p className="muted">
          The new name syncs to the tablet on the next sync.
        </p>
        <form onSubmit={submit}>
          <input
            ref={inputRef}
            type="text"
            value={name}
            onChange={(e) => setName(e.target.value)}
            disabled={busy}
            placeholder={kind === "folder" ? "Folder name" : "Document name"}
            onKeyDown={(e) => {
              if (e.key === "Escape") {
                e.preventDefault();
                onCancel();
              }
            }}
          />
          {error && <div className="error inline">{error}</div>}
          <div className="actions">
            <button type="button" onClick={onCancel} disabled={busy}>
              Cancel
            </button>
            <button type="submit" disabled={busy || !name.trim()}>
              {busy ? "Renaming…" : "Rename"}
            </button>
          </div>
        </form>
      </div>
    </div>
  );
}
