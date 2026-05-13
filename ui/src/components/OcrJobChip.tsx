/* Floating progress chip shown while a background OCR job is in
 * flight. Lives at the App level so closing the OcrDialog (or
 * navigating away from the document) doesn't lose progress
 * visibility. The actual job is a single in-flight call to
 * `ipc.transcribeDocument`; per-page deltas come over `ocr:progress`
 * events handled in App.tsx.
 *
 * The "×" button is **Cancel**: it trips the shared OCR cancel
 * flag (`cancel_ocr` IPC) so the current document's transcribe
 * aborts at the next page boundary, AND invalidates the sweep
 * token so the loop between docs unwinds. Cancellation is
 * cooperative — the engine polls between pages — so the chip may
 * linger briefly while the in-flight page finishes. */
import type { OcrJob, OcrSweepProgress } from "../types";

export function OcrJobChip({
  job,
  elapsedSeconds,
  onDismiss,
  sweep,
}: {
  job: OcrJob;
  elapsedSeconds: number;
  onDismiss: () => void;
  sweep?: OcrSweepProgress | null;
}) {
  if (job.phase !== "running") return null;
  const pageLabel = job.pagesDone === 1 ? "page" : "pages";
  // We don't know the total page count until the IPC call returns,
  // so render an indeterminate-looking bar that grows as more pages
  // come in. Width is capped at 80% so the chip never looks "done"
  // before it actually is.
  const pct = Math.min(80, 8 + job.pagesDone * 6);
  const inSweep = sweep && sweep.totalAtStart > 0;
  // Cancellation is real in both modes now: the X trips the shared
  // OCR cancel flag (and in sweep mode also halts the loop between
  // docs). Bounded by the engine's per-page polling, so the chip
  // may linger a beat after the click.
  const titleClose = inSweep
    ? "Stop the auto-OCR sweep"
    : "Cancel — stops at the next page";
  const ariaClose = inSweep ? "Stop auto-OCR sweep" : "Cancel OCR";
  return (
    <div className="ocr-chip" role="status" aria-live="polite">
      <div
        className="ocr-chip-bar"
        role="progressbar"
        aria-valuemin={0}
        aria-valuemax={100}
        aria-valuenow={Math.round(pct)}
        aria-label={`OCR progress for ${job.visibleName}`}
      >
        <span className="indeterminate" style={{ width: `${pct}%` }} />
      </div>
      <div className="ocr-chip-text">
        <strong>
          {inSweep ? "Auto OCR" : "OCR"} · {job.visibleName}
          {inSweep && (
            <span className="muted small">
              {" "}
              ({sweep!.done + 1} of {sweep!.totalAtStart})
            </span>
          )}
        </strong>
        <span className="muted small">
          {job.pagesDone} {pageLabel} · {job.charCount.toLocaleString()} chars · {formatElapsed(elapsedSeconds)}
        </span>
      </div>
      <button
        type="button"
        className="ocr-chip-close"
        aria-label={ariaClose}
        onClick={onDismiss}
        title={titleClose}
      >
        {inSweep ? "Stop" : "Cancel"}
      </button>
    </div>
  );
}

function formatElapsed(seconds: number): string {
  const total = Math.max(0, Math.floor(seconds));
  const m = Math.floor(total / 60);
  const s = total % 60;
  if (m === 0) return `${s}s`;
  return `${m}:${s.toString().padStart(2, "0")}`;
}
