import { useEffect, useMemo, useState } from "react";
import { ipc } from "../ipc";
import type {
  CuratedOllamaModel,
  GhostCredentials,
  OllamaConfig,
  PingReport,
  PublishCredentialStatus,
  WordpressCredentials,
} from "../types";
import { useDialogA11y } from "../dialogA11y";
import { Icon } from "./Icon";
import { formatError } from "../formatError";
import { useConfirm } from "./Confirm";
import { Skeleton } from "./Skeleton";

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

export type SettingsTab = "ollama" | "publishing";

/// Client-side URL shape validation. Matches the Rust side's
/// `validate_remote_url` rules at the surface (scheme + host present)
/// without trying to replicate the IP-range checks — the backend
/// remains authoritative, this just catches obvious typos so the
/// user doesn't bounce through a save → IPC round-trip just to learn
/// they forgot a "/" or a scheme.
function validateUrlShape(url: string): string | null {
  if (!url) return "URL is required.";
  let parsed: URL;
  try {
    parsed = new URL(url);
  } catch {
    return "URL doesn't look right. Did you include `https://`?";
  }
  if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
    return "URL must start with http:// or https://.";
  }
  if (!parsed.hostname) {
    return "URL must have a host.";
  }
  return null;
}

/* Single tabbed Settings modal. Replaces the standalone
 * PublishingSettings panel from the OCR feature branch and adds the
 * Ollama connection tab the v1.0 OCR pivot needs. Credentials still
 * live in the OS keychain; the Ollama URL+model live in
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
          <button className="icon ghost" onClick={onClose} aria-label="Close Settings">
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
          <OllamaTab notify={notify} />
        ) : (
          <PublishingTab notify={notify} />
        )}
      </div>
    </div>
  );
}

// =====================================================================
//   Ollama tab
// =====================================================================

function OllamaTab({
  notify,
}: {
  notify: Props["notify"];
}) {
  const [baseUrl, setBaseUrl] = useState("");
  const [model, setModel] = useState("");
  const [customModel, setCustomModel] = useState("");
  const [useCustom, setUseCustom] = useState(false);
  const [autoOcr, setAutoOcr] = useState(false);
  const [curated, setCurated] = useState<CuratedOllamaModel[]>([]);
  const [busy, setBusy] = useState<"save" | "test" | null>(null);
  const [ping, setPing] = useState<PingReport | null>(null);
  const [error, setError] = useState<string | null>(null);
  // Track initial load so the form renders a Skeleton placeholder
  // until both `getOllamaConfig` and `listCuratedOllamaModels` land
  // — otherwise the form briefly flashes empty fields and looks
  // broken on slow boots.
  const [loaded, setLoaded] = useState(false);

  // Initial load: pull the persisted config + the curated model list.
  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const [cfg, list] = await Promise.all([
          ipc.getOllamaConfig(),
          ipc.listCuratedOllamaModels(),
        ]);
        if (cancelled) return;
        setBaseUrl(cfg.base_url);
        setAutoOcr(cfg.auto_ocr_on_startup);
        setCurated(list);
        const knownIds = list.map((c) => c.id);
        if (knownIds.includes(cfg.model)) {
          setModel(cfg.model);
          setUseCustom(false);
        } else {
          setModel("__custom");
          setCustomModel(cfg.model);
          setUseCustom(true);
        }
        setLoaded(true);
      } catch (e) {
        if (!cancelled) {
          setError(formatError(e));
          setLoaded(true);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  const effectiveModel = useMemo(
    () => (useCustom ? customModel.trim() : model),
    [useCustom, model, customModel],
  );

  async function test() {
    setBusy("test");
    setPing(null);
    setError(null);
    try {
      const result = await ipc.pingOllama(baseUrl.trim());
      setPing(result);
      if (result.ok) {
        notify("ok", "Connected to Ollama.");
      } else {
        notify("warn", result.error ?? "Couldn't reach Ollama.");
      }
    } catch (e) {
      setError(formatError(e));
    } finally {
      setBusy(null);
    }
  }

  async function save() {
    // Client-side URL shape check so a typo surfaces here rather than
    // after the round-trip — matches the backend's
    // `validate_remote_url` rules (`http`/`https` only, host present).
    const trimmedUrl = baseUrl.trim();
    const urlErr = validateUrlShape(trimmedUrl);
    if (urlErr) {
      setError(urlErr);
      return;
    }
    setBusy("save");
    setError(null);
    try {
      const cfg: OllamaConfig = {
        base_url: trimmedUrl,
        model: effectiveModel,
        auto_ocr_on_startup: autoOcr,
      };
      await ipc.saveOllamaConfig(cfg);
      notify("ok", "Saved Ollama settings.");
    } catch (e) {
      setError(formatError(e));
    } finally {
      setBusy(null);
    }
  }

  const modelPulled = useMemo(() => {
    if (!ping?.ok) return null;
    return ping.models.includes(effectiveModel);
  }, [ping, effectiveModel]);

  if (!loaded) {
    return (
      <section className="settings-section">
        <Skeleton width="80%" height={14} mb={12} />
        <Skeleton width="100%" height={32} mb={12} />
        <Skeleton width="100%" height={32} mb={12} />
        <Skeleton width="60%" height={14} mb={12} />
        <Skeleton width="100%" height={32} mb={12} />
      </section>
    );
  }

  return (
    <section className="settings-section">
      <p className="muted">
        OCR runs through a vision-language model on your Ollama daemon. By
        default reHydrate looks for it on{" "}
        <code>http://localhost:11434</code>; point this at a remote box on
        your LAN if you run Ollama elsewhere. Pull a model first with{" "}
        <code>ollama pull {effectiveModel || "qwen2.5vl:3b"}</code>.
      </p>
      <label>
        Base URL
        <input
          type="url"
          placeholder="http://localhost:11434"
          value={baseUrl}
          onChange={(e) => setBaseUrl(e.target.value)}
          disabled={busy !== null}
        />
      </label>

      <label>
        Model
        <select
          value={useCustom ? "__custom" : model}
          onChange={(e) => {
            const v = e.target.value;
            if (v === "__custom") {
              setUseCustom(true);
              setModel("__custom");
            } else {
              setUseCustom(false);
              setModel(v);
            }
          }}
          disabled={busy !== null}
        >
          {curated.map((m) => (
            <option key={m.id} value={m.id}>
              {m.label} — {m.vram_hint}
            </option>
          ))}
          <option value="__custom">Custom…</option>
        </select>
      </label>

      {useCustom && (
        <label>
          Custom model tag
          <input
            type="text"
            placeholder="e.g. minicpm-v:8b"
            value={customModel}
            onChange={(e) => setCustomModel(e.target.value)}
            disabled={busy !== null}
          />
        </label>
      )}

      <label className="settings-checkbox">
        <input
          type="checkbox"
          checked={autoOcr}
          onChange={(e) => setAutoOcr(e.target.checked)}
          disabled={busy !== null}
        />
        <span>
          <strong>Auto-transcribe new notebooks at startup</strong>
          <span className="muted small">
            When you open the app, every notebook without an existing
            transcript is OCRed in the background. Off until Ollama is
            actually reachable, so a daemon-stopped launch silently
            skips the sweep instead of erroring out. Existing
            transcripts are never overwritten.
          </span>
        </span>
      </label>

      <div className="actions">
        <button
          disabled={busy !== null || !baseUrl || !effectiveModel}
          onClick={save}
        >
          {busy === "save" ? "Saving…" : "Save"}
        </button>
        <button disabled={busy !== null || !baseUrl} onClick={test}>
          {busy === "test" ? "Testing…" : "Test connection"}
        </button>
      </div>

      {ping && (
        <div
          className={`settings-ping ${ping.ok ? "ok" : "err"}`}
          role="status"
        >
          {ping.ok ? (
            <>
              <Icon name="check" />
              <span>
                {ping.models.length === 0 ? (
                  <>
                    Connected — but no models are pulled. Run{" "}
                    <code>ollama pull {effectiveModel}</code> in a
                    terminal first.
                  </>
                ) : (
                  <>
                    Connected. {ping.models.length} model
                    {ping.models.length === 1 ? "" : "s"} pulled.
                    {modelPulled === false && (
                      <>
                        {" "}
                        <strong>
                          `{effectiveModel}` isn't one of them — run{" "}
                          <code>ollama pull {effectiveModel}</code> in a
                          terminal first.
                        </strong>
                      </>
                    )}
                  </>
                )}
              </span>
            </>
          ) : (
            <>
              <Icon name="warn" />
              <span>{ping.error ?? "Couldn't reach Ollama."}</span>
            </>
          )}
        </div>
      )}

      {error && <div className="error inline">{error}</div>}
    </section>
  );
}

// =====================================================================
//   Publishing tab — lifted verbatim from the feature branch's
//   PublishingSettings.tsx body, restyled to fit inside the modal.
// =====================================================================

function PublishingTab({ notify }: { notify: Props["notify"] }) {
  const [status, setStatus] = useState<PublishCredentialStatus | null>(null);

  const [ghostUrl, setGhostUrl] = useState("");
  const [ghostKey, setGhostKey] = useState("");
  const [ghostBusy, setGhostBusy] = useState<string | null>(null);

  const [wpUrl, setWpUrl] = useState("");
  const [wpUser, setWpUser] = useState("");
  const [wpPwd, setWpPwd] = useState("");
  const [wpBusy, setWpBusy] = useState<string | null>(null);

  const [error, setError] = useState<string | null>(null);
  const [loaded, setLoaded] = useState(false);
  const confirm = useConfirm();

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const s = await ipc.publishCredentialStatus();
        if (!cancelled) {
          setStatus(s);
          setLoaded(true);
        }
      } catch (e) {
        if (!cancelled) {
          setError(formatError(e));
          setLoaded(true);
        }
      }
    })();
    return () => {
      cancelled = true;
    };
  }, []);

  async function refreshStatus() {
    try {
      setStatus(await ipc.publishCredentialStatus());
    } catch (e) {
      setError(formatError(e));
    }
  }

  async function saveGhost() {
    const trimmedUrl = ghostUrl.trim();
    const urlErr = validateUrlShape(trimmedUrl);
    if (urlErr) {
      setError(`Ghost URL: ${urlErr}`);
      return;
    }
    setGhostBusy("save");
    setError(null);
    try {
      const creds: GhostCredentials = {
        base_url: trimmedUrl,
        admin_api_key: ghostKey.trim(),
      };
      await ipc.setGhostCredentials(creds);
      await refreshStatus();
      notify("ok", "Saved Ghost credentials.");
      setGhostKey("");
    } catch (e) {
      setError(formatError(e));
    } finally {
      setGhostBusy(null);
    }
  }

  async function testGhost() {
    setGhostBusy("test");
    setError(null);
    try {
      await ipc.pingPublishTarget("ghost");
      notify("ok", "Ghost connection succeeded.");
    } catch (e) {
      setError(formatError(e));
    } finally {
      setGhostBusy(null);
    }
  }

  async function forgetGhost() {
    const ok = await confirm({
      title: "Forget Ghost credentials?",
      body: "The Admin API key will be removed from your OS keychain. You'll need to paste it again to publish.",
      confirmLabel: "Forget",
      destructive: true,
    });
    if (!ok) return;
    setGhostBusy("forget");
    setError(null);
    try {
      await ipc.forgetGhostCredentials();
      await refreshStatus();
      notify("ok", "Forgot Ghost credentials.");
    } catch (e) {
      setError(formatError(e));
    } finally {
      setGhostBusy(null);
    }
  }

  async function saveWp() {
    const trimmedUrl = wpUrl.trim();
    const urlErr = validateUrlShape(trimmedUrl);
    if (urlErr) {
      setError(`WordPress URL: ${urlErr}`);
      return;
    }
    setWpBusy("save");
    setError(null);
    try {
      const creds: WordpressCredentials = {
        base_url: trimmedUrl,
        username: wpUser.trim(),
        application_password: wpPwd.trim(),
      };
      await ipc.setWordpressCredentials(creds);
      await refreshStatus();
      notify("ok", "Saved WordPress credentials.");
      setWpPwd("");
    } catch (e) {
      setError(formatError(e));
    } finally {
      setWpBusy(null);
    }
  }

  async function testWp() {
    setWpBusy("test");
    setError(null);
    try {
      await ipc.pingPublishTarget("wordpress");
      notify("ok", "WordPress connection succeeded.");
    } catch (e) {
      setError(formatError(e));
    } finally {
      setWpBusy(null);
    }
  }

  async function forgetWp() {
    const ok = await confirm({
      title: "Forget WordPress credentials?",
      body: "The application password will be removed from your OS keychain. You'll need to paste it again to publish.",
      confirmLabel: "Forget",
      destructive: true,
    });
    if (!ok) return;
    setWpBusy("forget");
    setError(null);
    try {
      await ipc.forgetWordpressCredentials();
      await refreshStatus();
      notify("ok", "Forgot WordPress credentials.");
    } catch (e) {
      setError(formatError(e));
    } finally {
      setWpBusy(null);
    }
  }

  if (!loaded) {
    return (
      <section className="settings-section">
        <Skeleton width="80%" height={14} mb={12} />
        <Skeleton width="100%" height={32} mb={12} />
        <Skeleton width="100%" height={32} mb={12} />
        <Skeleton width="60%" height={14} mb={12} />
      </section>
    );
  }

  return (
    <section className="settings-section">
      <p className="muted">
        Credentials are stored in your OS keychain. reHydrate only contacts
        the host you enter — no third-party services see your transcripts.
      </p>

      <h3>
        Ghost{" "}
        {status?.ghost && <span className="muted small">configured</span>}
      </h3>
      <label>
        Site URL
        <input
          type="url"
          placeholder="https://blog.example.com"
          value={ghostUrl}
          onChange={(e) => setGhostUrl(e.target.value)}
          disabled={ghostBusy !== null}
        />
      </label>
      <label>
        Admin API key
        <input
          type="password"
          placeholder="<id>:<hex_secret> from Settings → Integrations"
          value={ghostKey}
          onChange={(e) => setGhostKey(e.target.value)}
          disabled={ghostBusy !== null}
        />
      </label>
      <div className="actions">
        <button
          disabled={ghostBusy !== null || !ghostUrl || !ghostKey}
          onClick={saveGhost}
        >
          {ghostBusy === "save" ? "Saving…" : "Save"}
        </button>
        <button
          disabled={ghostBusy !== null || !status?.ghost}
          onClick={testGhost}
        >
          {ghostBusy === "test" ? "Testing…" : "Test connection"}
        </button>
        <button
          disabled={ghostBusy !== null || !status?.ghost}
          onClick={forgetGhost}
        >
          {ghostBusy === "forget" ? "Forgetting…" : "Forget"}
        </button>
      </div>

      <h3>
        WordPress{" "}
        {status?.wordpress && (
          <span className="muted small">configured</span>
        )}
      </h3>
      <label>
        Site URL
        <input
          type="url"
          placeholder="https://example.com"
          value={wpUrl}
          onChange={(e) => setWpUrl(e.target.value)}
          disabled={wpBusy !== null}
        />
      </label>
      <label>
        Username
        <input
          type="text"
          value={wpUser}
          onChange={(e) => setWpUser(e.target.value)}
          disabled={wpBusy !== null}
        />
      </label>
      <label>
        Application password
        <input
          type="password"
          placeholder="abcd efgh ijkl mnop (Users → Profile → Application Passwords)"
          value={wpPwd}
          onChange={(e) => setWpPwd(e.target.value)}
          disabled={wpBusy !== null}
        />
      </label>
      <div className="actions">
        <button
          disabled={wpBusy !== null || !wpUrl || !wpUser || !wpPwd}
          onClick={saveWp}
        >
          {wpBusy === "save" ? "Saving…" : "Save"}
        </button>
        <button
          disabled={wpBusy !== null || !status?.wordpress}
          onClick={testWp}
        >
          {wpBusy === "test" ? "Testing…" : "Test connection"}
        </button>
        <button
          disabled={wpBusy !== null || !status?.wordpress}
          onClick={forgetWp}
        >
          {wpBusy === "forget" ? "Forgetting…" : "Forget"}
        </button>
      </div>

      {error && <div className="error inline">{error}</div>}
    </section>
  );
}

/** Helper the OCR error router uses to decide whether a thrown
 *  error from `transcribe_document` should auto-open the modal. */
export function parseOllamaUnconfigured(
  raw: unknown,
): { kind: "ollama_unconfigured"; message: string } | null {
  const text = typeof raw === "string" ? raw : raw instanceof Error ? raw.message : null;
  if (!text) return null;
  // The Rust side returns a JSON-encoded `{ kind, base_url, model, message }`
  // payload. Tauri wraps the Err(String) in its own quoting; the
  // payload may arrive as the JSON itself or as a quoted JSON
  // string. Be tolerant of both shapes.
  for (const candidate of [text, safeUnquote(text)]) {
    try {
      const v = JSON.parse(candidate);
      if (v && v.kind === "ollama_unconfigured") {
        return { kind: "ollama_unconfigured", message: v.message ?? text };
      }
    } catch {
      // not JSON; fall through
    }
  }
  return null;
}

function safeUnquote(s: string): string {
  if (s.length >= 2 && s.startsWith('"') && s.endsWith('"')) {
    try {
      return JSON.parse(s) as string;
    } catch {
      return s;
    }
  }
  return s;
}

/** Helper to detect the "no credentials saved" error from
 *  `publish_transcript`. */
export function parsePublishUnconfigured(
  raw: unknown,
): { kind: "publish_unconfigured"; target: string; message: string } | null {
  const text = typeof raw === "string" ? raw : raw instanceof Error ? raw.message : null;
  if (!text) return null;
  for (const candidate of [text, safeUnquote(text)]) {
    try {
      const v = JSON.parse(candidate);
      if (v && v.kind === "publish_unconfigured") {
        return {
          kind: "publish_unconfigured",
          target: v.target ?? "",
          message: v.message ?? text,
        };
      }
    } catch {
      // not JSON
    }
  }
  return null;
}
