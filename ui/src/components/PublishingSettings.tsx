import { useEffect, useState } from "react";
import { ipc } from "../ipc";
import type {
  GhostCredentials,
  PublishCredentialStatus,
  WordpressCredentials,
} from "../types";
import { Icon } from "./Icon";

interface Props {
  onClose: () => void;
  notify: (tone: "ok" | "err", body: string) => void;
}

/* Modal panel for entering Ghost + WordPress credentials. Both
 * credential blobs are stored in the OS keychain by the Rust side
 * (KEYRING_GHOST_CREDS / KEYRING_WORDPRESS_CREDS) — this UI only
 * carries plaintext during the panel's lifetime. */
export function PublishingSettings({ onClose, notify }: Props) {
  const [status, setStatus] = useState<PublishCredentialStatus | null>(null);

  // Ghost form fields. Pre-populated as empty even when creds exist
  // so we don't display the secret back to the user.
  const [ghostUrl, setGhostUrl] = useState("");
  const [ghostKey, setGhostKey] = useState("");
  const [ghostBusy, setGhostBusy] = useState<string | null>(null);

  const [wpUrl, setWpUrl] = useState("");
  const [wpUser, setWpUser] = useState("");
  const [wpPwd, setWpPwd] = useState("");
  const [wpBusy, setWpBusy] = useState<string | null>(null);

  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    (async () => {
      try {
        const s = await ipc.publishCredentialStatus();
        if (!cancelled) setStatus(s);
      } catch (e) {
        if (!cancelled) setError(String(e));
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
      setError(String(e));
    }
  }

  async function saveGhost() {
    setGhostBusy("save");
    setError(null);
    try {
      const creds: GhostCredentials = {
        base_url: ghostUrl.trim(),
        admin_api_key: ghostKey.trim(),
      };
      await ipc.setGhostCredentials(creds);
      await refreshStatus();
      notify("ok", "Saved Ghost credentials.");
      setGhostKey("");
    } catch (e) {
      setError(String(e));
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
      setError(String(e));
    } finally {
      setGhostBusy(null);
    }
  }

  async function forgetGhost() {
    setGhostBusy("forget");
    setError(null);
    try {
      await ipc.forgetGhostCredentials();
      await refreshStatus();
      notify("ok", "Forgot Ghost credentials.");
    } catch (e) {
      setError(String(e));
    } finally {
      setGhostBusy(null);
    }
  }

  async function saveWp() {
    setWpBusy("save");
    setError(null);
    try {
      const creds: WordpressCredentials = {
        base_url: wpUrl.trim(),
        username: wpUser.trim(),
        application_password: wpPwd.trim(),
      };
      await ipc.setWordpressCredentials(creds);
      await refreshStatus();
      notify("ok", "Saved WordPress credentials.");
      setWpPwd("");
    } catch (e) {
      setError(String(e));
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
      setError(String(e));
    } finally {
      setWpBusy(null);
    }
  }

  async function forgetWp() {
    setWpBusy("forget");
    setError(null);
    try {
      await ipc.forgetWordpressCredentials();
      await refreshStatus();
      notify("ok", "Forgot WordPress credentials.");
    } catch (e) {
      setError(String(e));
    } finally {
      setWpBusy(null);
    }
  }

  return (
    <div className="modal-backdrop" onClick={onClose}>
      <div
        className="modal modal-wide"
        onClick={(e) => e.stopPropagation()}
        aria-label="Publishing settings"
      >
        <header className="modal-header">
          <h2>Publishing</h2>
          <button className="icon ghost" onClick={onClose} aria-label="Close">
            <Icon name="x" />
          </button>
        </header>

        <p className="muted">
          Credentials are stored in your OS keychain. reHydrate only contacts
          the host you enter — no third-party services see your transcripts.
        </p>

        <section className="publish-section">
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
        </section>

        <section className="publish-section">
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
        </section>

        {error && <div className="error inline">{error}</div>}
      </div>
    </div>
  );
}
