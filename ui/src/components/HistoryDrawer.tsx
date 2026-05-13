import { useEffect, useMemo, useState } from "react";
import { ipc } from "../ipc";
import { useConfirm } from "./Confirm";
import { useToast } from "./Toast";
import { Icon } from "./Icon";
import { Skeleton } from "./Skeleton";
import type { DocumentSummary, VersionEntry } from "../types";
import { formatError } from "../formatError";
import { useDialogA11y } from "../dialogA11y";

interface Props {
  document: DocumentSummary;
  onClose: () => void;
}

export function HistoryDrawer({ document, onClose }: Props) {
  const { dialogProps, rootRef, titleId } = useDialogA11y({
    onEscape: onClose,
  });
  const [versions, setVersions] = useState<VersionEntry[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<number | null>(null);
  const confirm = useConfirm();
  const toast = useToast();

  useEffect(() => {
    let cancelled = false;
    ipc
      .getHistory(document.document_id)
      .then((v) => {
        if (!cancelled) setVersions(v);
      })
      .catch((e) => {
        if (!cancelled) setError(formatError(e));
      });
    return () => {
      cancelled = true;
    };
  }, [document.document_id]);

  // Newest first for display, but compute diffs against the *previous*
  // version (chronologically) so older→newer makes sense.
  const ordered = useMemo(() => {
    if (!versions) return [];
    const reversed = [...versions].reverse();
    return reversed.map((v, i) => {
      const prev = reversed[i + 1] ?? null;
      return { v, prev, isCurrent: i === 0, version: reversed.length - i };
    });
  }, [versions]);

  async function restoreVersion(v: VersionEntry, label: string) {
    const ok = await confirm({
      title: `Restore "${document.visible_name}" to ${label}?`,
      body: "The current version stays in history; the restored version becomes current and will sync to the tablet on the next sync.",
      confirmLabel: "Restore",
    });
    if (!ok) return;
    setError(null);
    try {
      await ipc.restoreVersion(v.id);
      const fresh = await ipc.getHistory(document.document_id);
      setVersions(fresh);
      toast.show({
        tone: "ok",
        body: `Restored to ${label}. The change will sync on the next sync.`,
      });
    } catch (e) {
      setError(formatError(e));
    }
  }

  async function exportVersion(v: VersionEntry) {
    setError(null);
    try {
      setBusy(v.id);
      // Server-side folder picker (audit fix H7) — `null` means the
      // user cancelled.
      const result = await ipc.exportVersion(v.id);
      if (!result) return;
      toast.show({
        tone: "ok",
        body: `Exported ${result.file_count} file${result.file_count === 1 ? "" : "s"} to ${result.path}`,
      });
    } catch (e) {
      setError(formatError(e));
    } finally {
      setBusy(null);
    }
  }

  async function saveNote(v: VersionEntry, note: string) {
    setError(null);
    const trimmed = note.trim();
    try {
      await ipc.setVersionNote(v.id, trimmed === "" ? null : trimmed);
      setVersions((prev) =>
        prev?.map((x) => (x.id === v.id ? { ...x, note: trimmed === "" ? null : trimmed } : x)) ??
        prev,
      );
    } catch (e) {
      setError(formatError(e));
    }
  }

  return (
    <div
      className="drawer"
      onClick={(e) => e.stopPropagation()}
      ref={rootRef}
      {...dialogProps}
    >
      <header>
        <div>
          <h2 id={titleId}>{document.visible_name}</h2>
          <div className="muted small">
            {prettyType(document.doc_type)}
            {document.page_count !== null && ` · ${document.page_count} page${document.page_count === 1 ? "" : "s"}`}
          </div>
        </div>
        <button onClick={onClose} className="close" aria-label="Close">
          ×
        </button>
      </header>

      {error && (
        <div className="error">
          <Icon name="warn" />
          {error}
        </div>
      )}

      {!versions && !error && (
        <div className="empty">
          <Skeleton width={180} height={20} mb={10} />
          <Skeleton width={240} mb={6} />
          <Skeleton width={210} />
        </div>
      )}

      {versions && versions.length === 0 && (
        <div className="empty">No versions recorded yet.</div>
      )}

      {ordered.length > 0 && (
        <ul className="timeline">
          {ordered.map(({ v, prev, isCurrent, version }) => {
            const diff = prev ? diffSummary(prev, v) : null;
            return (
              <li key={v.id} className={isCurrent ? "current" : ""}>
                <span className="timeline-dot" />
                <div className="row">
                  <span className={`badge ${isCurrent ? "badge-ok" : "badge-unchanged"}`}>
                    v{version}
                    {isCurrent ? " · current" : ""}
                  </span>
                  <span className="muted small">{prettyDate(v.observed_at)}</span>
                  <span className="muted small">{sourceLabel(v.source)}</span>
                </div>
                <div className="row">
                  {v.file_count !== null && (
                    <span className="muted small">
                      {v.file_count} file{v.file_count === 1 ? "" : "s"}
                    </span>
                  )}
                  {v.total_size_bytes !== null && (
                    <span className="muted small">{formatBytes(v.total_size_bytes)}</span>
                  )}
                  {diff && (
                    <span className="timeline-diff">
                      {diff.added > 0 && <span className="add">+{diff.added}</span>}
                      {diff.removed > 0 && <span className="rm">−{diff.removed}</span>}
                      {diff.changed > 0 && <span className="ch">~{diff.changed}</span>}
                      <span className="muted">file{diff.added + diff.removed + diff.changed === 1 ? "" : "s"}</span>
                    </span>
                  )}
                </div>
                <NoteField version={v} onSave={(note) => saveNote(v, note)} />
                <div className="timeline-actions">
                  {!isCurrent && (
                    <button
                      className="link"
                      onClick={() => restoreVersion(v, `v${version}`)}
                      disabled={busy === v.id}
                    >
                      <Icon name="restore" /> Restore
                    </button>
                  )}
                  <button
                    className="link"
                    onClick={() => exportVersion(v)}
                    disabled={busy === v.id}
                  >
                    <Icon name="import" /> {busy === v.id ? "Exporting…" : "Export"}
                  </button>
                </div>
              </li>
            );
          })}
        </ul>
      )}

      <footer>
        <span className="muted small">
          {versions
            ? `${versions.length} version${versions.length === 1 ? "" : "s"} on file`
            : ""}
        </span>
      </footer>
    </div>
  );
}

function NoteField({ version, onSave }: { version: VersionEntry; onSave: (note: string) => void }) {
  const [value, setValue] = useState(version.note ?? "");
  const [editing, setEditing] = useState(false);

  if (!editing && !value) {
    return (
      <button className="link" onClick={() => setEditing(true)} style={{ alignSelf: "start" }}>
        + Add note
      </button>
    );
  }
  if (!editing) {
    return (
      <div className="note" onClick={() => setEditing(true)}>
        {value}
      </div>
    );
  }
  return (
    <input
      type="text"
      autoFocus
      value={value}
      onChange={(e) => setValue(e.target.value)}
      onBlur={() => {
        setEditing(false);
        if (value !== (version.note ?? "")) onSave(value);
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter") (e.target as HTMLInputElement).blur();
        if (e.key === "Escape") {
          setValue(version.note ?? "");
          setEditing(false);
        }
      }}
      placeholder="Add a note for this version…"
    />
  );
}

function diffSummary(prev: VersionEntry, curr: VersionEntry): {
  added: number;
  removed: number;
  changed: number;
} {
  // We can compare file counts and sizes from VersionEntry without a
  // round-trip to the manifest. This is a coarse diff — "X files
  // appear, Y disappear, Z change" — but it's enough to tell the user
  // *something* about each version at a glance.
  const a = prev.file_count ?? 0;
  const b = curr.file_count ?? 0;
  if (a === 0 && b === 0) return { added: 0, removed: 0, changed: 0 };
  if (b > a) return { added: b - a, removed: 0, changed: 0 };
  if (a > b) return { added: 0, removed: a - b, changed: 0 };
  // Same count: bytes differ implies content edited.
  const sa = prev.total_size_bytes ?? 0;
  const sb = curr.total_size_bytes ?? 0;
  return { added: 0, removed: 0, changed: sa === sb ? 0 : 1 };
}

function formatBytes(n: number): string {
  if (n < 1024) return `${n} B`;
  const units = ["KB", "MB", "GB"];
  let v = n / 1024;
  let i = 0;
  while (v >= 1024 && i < units.length - 1) {
    v /= 1024;
    i++;
  }
  return `${v.toFixed(1)} ${units[i]}`;
}

function prettyDate(iso: string): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString();
}

function prettyType(s: string): string {
  if (s === "Notebook") return "Notebook";
  if (s === "DocumentType.Pdf") return "PDF";
  if (s === "DocumentType.Epub") return "EPUB";
  return s;
}

function sourceLabel(s: VersionEntry["source"]): string {
  switch (s) {
    case "pulled":
      return "Synced from tablet";
    case "imported":
      return "Added from disk";
    case "restored":
      return "Restored / edited";
  }
}
