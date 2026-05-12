import { useEffect, useState } from "react";

import { formatError } from "../formatError";
import { ipc } from "../ipc";
import type {
  GhostCredentials,
  PublishCredentialStatus,
  WordpressCredentials,
} from "../types";
import { useConfirm } from "./Confirm";
import { Skeleton } from "./Skeleton";
import { validateUrlShape } from "./settingsErrors";

interface Props {
  notify: (tone: "ok" | "warn" | "err", body: string) => void;
}

/* Publishing tab of the Settings modal. Credentials live in the OS
 * keychain via Tauri's `keyring` crate; the form just collects them
 * and asks the backend to test the connection before saving. Each
 * "Forget" button confirms because removing the saved credentials
 * forces the user to paste them again. */
export function SettingsPublishingTab({ notify }: Props) {
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

      {error && (
        <div className="error inline" role="alert">
          {error}
        </div>
      )}
    </section>
  );
}
