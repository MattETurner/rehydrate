import { useEffect } from "react";
import type { DocumentSummary } from "../types";
import { Icon } from "./Icon";
import { Thumbnail } from "./Thumbnail";

interface Props {
  document: DocumentSummary;
  onClose: () => void;
  onOpen: () => void;
}

/* Lightweight "Quick Look" popover triggered by Space. Shows the
 * first-page thumbnail plus the metadata most likely to inform the
 * "should I open this?" decision. */
export function QuickLook({ document, onClose, onOpen }: Props) {
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === "Escape" || e.key === " ") {
        e.preventDefault();
        onClose();
      } else if (e.key === "Enter") {
        e.preventDefault();
        onOpen();
      }
    }
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, onOpen]);

  return (
    <div className="quicklook-backdrop" onClick={onClose}>
      <div className="quicklook" onClick={(e) => e.stopPropagation()}>
        <div className="preview">
          <Thumbnail
            documentId={document.document_id}
            docType={document.doc_type}
            size="preview"
          />
        </div>
        <h2>{document.visible_name}</h2>
        <dl className="kv">
          <dt>Type</dt>
          <dd>{prettyType(document.doc_type)}</dd>
          <dt>Size</dt>
          <dd>{formatBytes(document.size_bytes)}</dd>
          {document.page_count !== null && (
            <>
              <dt>Pages</dt>
              <dd>{document.page_count}</dd>
            </>
          )}
          <dt>Synced</dt>
          <dd>{prettyDate(document.last_observed_at)}</dd>
          {document.has_unpushed_changes && (
            <>
              <dt>Status</dt>
              <dd>
                <span className="row-pill">Unsynced</span>
              </dd>
            </>
          )}
        </dl>
        <div className="actions" style={{ display: "flex", gap: 8, justifyContent: "flex-end" }}>
          <button onClick={onClose}>
            <span className="kbd">Esc</span> Close
          </button>
          <button className="primary" onClick={onOpen}>
            <Icon name="library" /> Open
          </button>
        </div>
      </div>
    </div>
  );
}

function prettyType(s: string): string {
  if (s === "Notebook") return "Notebook";
  if (s === "DocumentType.Pdf") return "PDF";
  if (s === "DocumentType.Epub") return "EPUB";
  return s;
}
function prettyDate(iso: string): string {
  if (!iso) return "—";
  const d = new Date(iso);
  if (Number.isNaN(d.getTime())) return iso;
  return d.toLocaleString();
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
