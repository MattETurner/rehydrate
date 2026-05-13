import { useEffect, useState } from "react";

import { useDialogA11y } from "../dialogA11y";
import { ipc } from "../ipc";
import { Icon } from "./Icon";

interface Props {
  onClose: () => void;
}

const SUPPORT_URL = "https://github.com/dm807cam/rehydrate/issues";
const SECURITY_URL =
  "https://github.com/dm807cam/rehydrate/security/advisories/new";
const RELEASES_URL = "https://github.com/dm807cam/rehydrate/releases";

// v1.0 has no auto-update flow. The About dialog is the user's only
// entry point from inside the app to (a) confirm which version
// they're running, (b) check for newer releases, and (c) reach a
// support / vulnerability-report channel. Without this dialog a
// customer with a broken install has no path from the app to help.
export function AboutDialog({ onClose }: Props) {
  const [version, setVersion] = useState<string | null>(null);
  const { dialogProps, rootRef, titleId } = useDialogA11y({
    onEscape: onClose,
  });

  useEffect(() => {
    let cancelled = false;
    ipc
      .appVersion()
      .then((v) => {
        if (!cancelled) setVersion(v);
      })
      .catch(() => {
        if (!cancelled) setVersion("unknown");
      });
    return () => {
      cancelled = true;
    };
  }, []);

  function openUrl(url: string) {
    ipc.openSupportUrl(url).catch(() => {
      // The Rust side guards the URL prefix; if a future hard-coded
      // string violates that guard we still want the user to be able
      // to copy the URL out, so the next step is to fall back to
      // copying to clipboard. Best-effort: navigator.clipboard may
      // not be granted in the webview, in which case we just log.
      navigator.clipboard?.writeText(url).catch(() => {});
    });
  }

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal"
        onClick={(e) => e.stopPropagation()}
        ref={rootRef}
        {...dialogProps}
      >
        <h2 id={titleId}>
          <Icon name="info" /> About reHydrate
        </h2>
        <p>
          Version{" "}
          <strong>{version ?? "…"}</strong>
          {" "}— macOS (Apple Silicon).
        </p>
        <p className="muted">
          reHydrate runs entirely on your machine. There is no
          telemetry; the threat model and disclosure channel are
          documented in <code>SECURITY.md</code>.
        </p>
        <p className="muted">
          © {new Date().getFullYear()} Dennis Mayk.
        </p>
        <div className="actions">
          <button onClick={() => openUrl(SUPPORT_URL)}>
            Report an issue…
          </button>
          <button onClick={() => openUrl(SECURITY_URL)}>
            Report a vulnerability…
          </button>
          <button onClick={() => openUrl(RELEASES_URL)}>
            Check for updates
          </button>
          <button onClick={onClose}>Close</button>
        </div>
      </div>
    </div>
  );
}
