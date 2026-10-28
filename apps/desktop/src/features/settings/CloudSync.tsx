// Settings → Accounts → Sync with iCloud (docs/CROSS-DEVICE-STATE.md):
// snoozes, Follow up, snippets, signatures and the synced settings move
// between this Mac and Penguin for iPhone through the user's own iCloud
// (src-tauri/src/cloud.rs). On by default; the line under the switch says
// when it last synced or what's in the way.
import { useEffect, useState } from "react";
import { api, asCommandError, onCloudStatus } from "../../lib/api";
import type { CloudStatus } from "../../lib/types";
import { Switch } from "./parts";
import { cloudLine } from "./cloudModel";

export function CloudSyncRow() {
  const [status, setStatus] = useState<CloudStatus | null>(null);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [, tick] = useState(0);

  useEffect(() => {
    let live = true;
    let unlisten: (() => void) | null = null;
    api.cloudStatus().then(
      (s) => live && setStatus(s),
      (e) => live && setError(asCommandError(e).message),
    );
    void onCloudStatus((s) => setStatus(s)).then((u) => (live ? (unlisten = u) : u()));
    // "Last synced 3 min ago" stays true.
    const timer = setInterval(() => tick((n) => n + 1), 30_000);
    return () => {
      live = false;
      unlisten?.();
      clearInterval(timer);
    };
  }, []);

  const toggle = (on: boolean) => {
    setBusy(true);
    setError(null);
    api
      .setCloudSync(on)
      .then(setStatus, (e) => setError(asCommandError(e).message))
      .finally(() => setBusy(false));
  };

  const syncNow = () => {
    setError(null);
    api.cloudSyncNow().then(setStatus, (e) => setError(asCommandError(e).message));
  };

  if (!status) return error ? <p className="st-muted">{error}</p> : null;
  const line = cloudLine(status);
  return (
    <div className="setting-row" data-setting="icloud-sync">
      <div className="min0 grow">
        <span className="setting-label">Sync with iCloud</span>
        <p className={line.warn ? "st-warn" : "st-muted"}>{error ?? line.text}</p>
      </div>
      {status.available && status.enabled && status.active ? (
        <button className="btn btn-ghost btn-sm" onClick={syncNow}>
          Sync now
        </button>
      ) : null}
      <Switch
        on={status.enabled && status.available}
        disabled={busy || !status.available}
        onChange={toggle}
        label="Sync with iCloud"
      />
    </div>
  );
}
