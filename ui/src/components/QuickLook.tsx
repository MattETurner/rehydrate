import { useEffect } from "react";

import { useDialogA11y } from "../dialogA11y";
import type { DocumentSummary } from "../types";
import { formatBytes, prettyDate, prettyType } from "../utils/format";
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
  const { dialogProps, rootRef, titleId } = useDialogA11y({
    onEscape: onClose,
  });
  // Quick Look reuses Space as a toggle and Enter to confirm — these
  // aren't covered by `useDialogA11y` (it only knows Escape).
  useEffect(() => {
    function onKey(e: KeyboardEvent) {
      if (e.key === " ") {
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
      <div
        className="quicklook"
        onClick={(e) => e.stopPropagation()}
        ref={rootRef}
        {...dialogProps}
      >
        <div className="preview">
          <Thumbnail
            documentId={document.document_id}
            docType={document.doc_type}
            size="preview"
          />
        </div>
        <h2 id={titleId}>{document.visible_name}</h2>
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
        <div className="actions actions-row-end">
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
