import { useState } from "react";

import { useDialogA11y } from "../dialogA11y";
import { Icon } from "./Icon";
import { SettingsOllamaTab } from "./SettingsOllamaTab";
import { SettingsPublishingTab } from "./SettingsPublishingTab";

export type SettingsTab = "ollama" | "publishing";

interface Props {
  /** Which tab to open with. The OCR command flow auto-opens the
   *  modal on the "ollama" tab when transcribe fails with the
   *  `ollama_unconfigured` tagged error. */
  initialTab?: SettingsTab;
  /** Optional info banner shown above the active tab — used when
   *  the modal was auto-opened to explain why. */
  banner?: string | null;
  onClose: () => void;
  notify: (tone: "ok" | "warn" | "err", body: string) => void;
}

/* Tabbed Settings modal. The shell handles tab nav + a11y; the two
 * tab bodies (`SettingsOllamaTab`, `SettingsPublishingTab`) live in
 * their own files so each is self-contained at ~250 lines instead
 * of buried inside a 700-line wall.
 *
 * Credentials live in the OS keychain; the Ollama URL+model live in
 * `~/Library/Application Support/reHydrate/config.json`. */
export function SettingsModal({
  initialTab = "ollama",
  banner = null,
  onClose,
  notify,
}: Props) {
  const [tab, setTab] = useState<SettingsTab>(initialTab);
  const { dialogProps, rootRef, titleId } = useDialogA11y({
    onEscape: onClose,
  });
  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal modal-wide settings-modal"
        onClick={(e) => e.stopPropagation()}
        ref={rootRef}
        {...dialogProps}
      >
        <header className="modal-header">
          <h2 id={titleId}>Settings</h2>
          <button
            className="icon ghost"
            onClick={onClose}
            aria-label="Close Settings"
          >
            <Icon name="x" />
          </button>
        </header>

        <nav className="settings-tabs" role="tablist">
          <button
            role="tab"
            aria-selected={tab === "ollama"}
            className={tab === "ollama" ? "active" : ""}
            onClick={() => setTab("ollama")}
          >
            Ollama (OCR)
          </button>
          <button
            role="tab"
            aria-selected={tab === "publishing"}
            className={tab === "publishing" ? "active" : ""}
            onClick={() => setTab("publishing")}
          >
            Publishing
          </button>
        </nav>

        {banner && (
          <div className="settings-banner" role="status">
            <Icon name="info" />
            <span>{banner}</span>
          </div>
        )}

        {tab === "ollama" ? (
          <SettingsOllamaTab notify={notify} />
        ) : (
          <SettingsPublishingTab notify={notify} />
        )}
      </div>
    </div>
  );
}

// Re-export the tagged-error parsers + the URL-shape helper from
// `settingsErrors` so existing call sites (TranscriptDrawer, App.tsx)
// don't have to learn the new module path.
export {
  parseOllamaUnconfigured,
  parsePublishUnconfigured,
  validateUrlShape,
} from "./settingsErrors";
