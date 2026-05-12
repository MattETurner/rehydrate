import { useEffect, useMemo, useState } from "react";

import { formatError } from "../formatError";
import { ipc } from "../ipc";
import type { CuratedOllamaModel, OllamaConfig, PingReport } from "../types";
import { Icon } from "./Icon";
import { Skeleton } from "./Skeleton";
import { validateUrlShape } from "./settingsErrors";

interface Props {
  notify: (tone: "ok" | "warn" | "err", body: string) => void;
}

/* Ollama tab of the Settings modal. OCR points at the Ollama daemon
 * the user runs locally (default `http://localhost:11434`); the form
 * collects the base URL, the model id, and the auto-OCR-at-startup
 * opt-in. The "Test connection" button consults `/api/tags` so the
 * user can confirm the daemon answers and the configured model is
 * actually pulled before saving. */
export function SettingsOllamaTab({ notify }: Props) {
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
  // until both `getOllamaConfig` and `listCuratedOllamaModels` land —
  // otherwise the form briefly flashes empty fields and looks
  // broken on slow boots.
  const [loaded, setLoaded] = useState(false);

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
    // Client-side URL shape check so a typo surfaces here rather
    // than after the round-trip — matches the backend's
    // `validate_remote_url` rules (`http`/`https` only, host
    // present).
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
        <code>ollama pull {effectiveModel || "qwen3.5:4b"}</code>.
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

      {error && (
        <div className="error inline" role="alert">
          {error}
        </div>
      )}
    </section>
  );
}
