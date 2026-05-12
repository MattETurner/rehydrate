/* Floating progress chip shown while a background OCR job is in
 * flight. Lives at the App level so closing the OcrDialog (or
 * navigating away from the document) doesn't lose progress
 * visibility. The actual job is a single in-flight call to
 * `ipc.transcribeDocument`; per-page deltas come over `ocr:progress`
 * events handled in App.tsx.
 *
 * The "×" button is **Hide**, not Cancel — the IPC call doesn't
 * support cancellation today, so a real "stop the model now"
 * affordance would be a lie. In the sweep mode the user *can* stop
 * the *sweep* (the queue between docs) by signalling via the
 * cancellation token, which is a real action and labelled as such. */
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
  // Sweep mode: the × stops the *sweep* (real cancellation token).
  // Single-job mode: the × is Hide only — the IPC call has no
  // cancellation handle, so framing it as "cancel" would lie.
  // The label and tooltip match the actual behaviour.
  const titleClose = inSweep
    ? "Stop the auto-OCR sweep"
    : "Hide — OCR keeps running in the background";
  const ariaClose = inSweep ? "Stop auto-OCR sweep" : "Hide OCR progress";
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
        {inSweep ? "Stop" : "Hide"}
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
