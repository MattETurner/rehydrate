import { useEffect, useState } from "react";
import { ipc, onDeviceReachable } from "../ipc";
import { Icon } from "./Icon";
import type { DeviceState } from "../types";

interface Props {
  defaultPath: string | null;
  initialDevice: DeviceState | null;
  onOpenLibrary: () => void;
  onConnect: () => void;
  onSkip: () => void;
}

/* Three-step first-run flow. The user can skip out of it at any
 * time; once a library is open the host falls through to the regular
 * empty state. */
export function Onboarding({
  defaultPath,
  initialDevice,
  onOpenLibrary,
  onConnect,
  onSkip,
}: Props) {
  const [step, setStep] = useState(0);
  const [device, setDevice] = useState<DeviceState | null>(initialDevice);

  useEffect(() => {
    let unlisten: (() => void) | undefined;
    onDeviceReachable(() => {
      ipc.deviceState().then(setDevice).catch(() => {});
    }).then((u) => {
      unlisten = u;
    });
    return () => {
      if (unlisten) unlisten();
    };
  }, []);

  // Auto-advance from "plug in tablet" to "open library" once the
  // tablet is detected. The user gets visible feedback that the app
  // saw their plug-in.
  useEffect(() => {
    if (step === 0 && device?.reachable) {
      const t = setTimeout(() => setStep(1), 500);
      return () => clearTimeout(t);
    }
  }, [step, device?.reachable]);

  if (step === 0) {
    return (
      <div className="onboarding">
        <div className="step-art">
          <Icon name="plug" size={56} />
        </div>
        <h1>Plug in your reMarkable</h1>
        <p>
          Connect the tablet to this Mac with its USB-C cable. Marginalia
          watches for it automatically — you don't need to do anything else.
        </p>
        <div className="actions">
          <button onClick={onSkip}>Skip for now</button>
          <button className="primary" onClick={() => setStep(1)}>
            I'll plug it in later
          </button>
        </div>
        <Dots step={step} />
        <p className="muted small">
          {device?.reachable
            ? "Tablet detected — moving on…"
            : "Waiting for tablet…"}
        </p>
      </div>
    );
  }

  if (step === 1) {
    return (
      <div className="onboarding">
        <div className="step-art">
          <Icon name="library" size={56} />
        </div>
        <h1>Open a library</h1>
        <p>
          Your library is a single folder on this machine that holds every
          version of every document. No cloud, no telemetry.
        </p>
        {defaultPath && (
          <p className="muted small">
            Default location: <code>{defaultPath}</code>
          </p>
        )}
        <div className="actions">
          <button onClick={() => setStep(0)}>Back</button>
          <button
            className="primary"
            onClick={onOpenLibrary}
            disabled={!defaultPath}
          >
            <Icon name="library" /> Open default library
          </button>
        </div>
        <Dots step={step} />
      </div>
    );
  }

  return (
    <div className="onboarding">
      <div className="step-art">
        <Icon name="tablet" size={56} />
      </div>
      <h1>Pair your tablet</h1>
      <p>
        Find the SSH password on your tablet at{" "}
        <strong>Settings → Help → Copyrights and licenses</strong> — it's at
        the bottom of the page. We'll store it in your OS keychain so you
        only enter it once.
      </p>
      <div className="actions">
        <button onClick={() => setStep(1)}>Back</button>
        <button className="primary" onClick={onConnect}>
          <Icon name="plug" /> Connect now
        </button>
      </div>
      <Dots step={step} />
    </div>
  );
}

function Dots({ step }: { step: number }) {
  return (
    <div className="dots">
      {[0, 1, 2].map((i) => (
        <span key={i} className={`dot${i === step ? " active" : ""}`} />
      ))}
    </div>
  );
}
