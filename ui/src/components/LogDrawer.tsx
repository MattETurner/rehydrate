import { useCallback, useEffect, useState } from "react";
import { ipc } from "../ipc";
import { formatError } from "../formatError";

interface Props {
  onClose: () => void;
}

export function LogDrawer({ onClose }: Props) {
  const [lines, setLines] = useState<string[]>([]);
  const [logDir, setLogDir] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);

  const refresh = useCallback(async () => {
    setLoading(true);
    setError(null);
    try {
      const t = await ipc.getRecentLogs(500);
      setLines(t.lines);
      setLogDir(t.log_dir);
    } catch (e) {
      setError(formatError(e));
    } finally {
      setLoading(false);
    }
  }, []);

  useEffect(() => {
    refresh();
  }, [refresh]);

  const revealInFinder = useCallback(async () => {
    try {
      await ipc.revealLogDir();
    } catch (e) {
      // Most common failure: the log dir doesn't exist yet because
      // no rotating-log file has been written this session. Fall
      // back to copying the path so support flows still work.
      const msg = formatError(e);
      setError(`Couldn't open log folder (${msg}). Path: ${logDir ?? "unknown"}`);
    }
  }, [logDir]);

  const copyPath = useCallback(async () => {
    if (!logDir) return;
    try {
      await navigator.clipboard.writeText(logDir);
    } catch {
      // Webview might not have clipboard access; show the path
      // inline so the user can manually copy.
      setError(`Couldn't copy. Path: ${logDir}`);
    }
  }, [logDir]);

  return (
    <div className="drawer" onClick={(e) => e.stopPropagation()}>
      <header>
        <div>
          <h2>Activity log</h2>
          {logDir && (
            <div className="muted small mono" title={logDir}>
              {logDir}
            </div>
          )}
        </div>
        <button onClick={revealInFinder} disabled={!logDir} className="link">
          Reveal in Finder
        </button>
        <button onClick={copyPath} disabled={!logDir} className="link">
          Copy path
        </button>
        <button onClick={refresh} disabled={loading} className="link">
          {loading ? "Refreshing…" : "Refresh"}
        </button>
        <button onClick={onClose} className="close" aria-label="Close logs">
          ×
        </button>
      </header>

      {error && (
        <div className="error" role="alert">
          {error}
        </div>
      )}

      {lines.length === 0 && !error ? (
        <div className="empty">No log entries yet.</div>
      ) : (
        <div className="log-tail">
          {lines.map((line, i) => (
            <div key={i} className={lineClass(line)}>
              {line}
            </div>
          ))}
        </div>
      )}
    </div>
  );
}

function lineClass(line: string): string {
  if (line.includes(" ERROR ") || line.includes(" error ")) return "log-line err";
  if (line.includes(" WARN ") || line.includes(" warn ")) return "log-line warn";
  return "log-line";
}
