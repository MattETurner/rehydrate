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
