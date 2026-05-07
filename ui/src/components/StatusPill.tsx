import type { DeviceState } from "../types";

export function StatusPill({ state }: { state: DeviceState | null }) {
  if (!state) {
    return <span className="pill">Loading…</span>;
  }
  if (state.connected && state.info) {
    return (
      <span className="pill pill-ok">
        Connected · {state.info.model}
        {state.info.software_version ? ` · ${state.info.software_version}` : ""}
      </span>
    );
  }
  if (state.reachable) {
    return <span className="pill pill-warn">Tablet detected</span>;
  }
  return <span className="pill">No tablet</span>;
}
