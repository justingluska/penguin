// Settings → Accounts → Sync with iCloud, simulated: on, an iCloud account,
// last synced a few minutes ago. ?mockCloud=off|noAccount|quota|unavailable
// shows the other states.
import type { CloudStatus } from "../types";
import { mockBackend, type MockHandler } from "./index";

const EVENT = "penguin://cloud-status";

function initial(): CloudStatus {
  const q = typeof location !== "undefined" ? new URLSearchParams(location.search).get("mockCloud") : null;
  const base: CloudStatus = {
    available: true,
    enabled: true,
    active: true,
    account: "available",
    lastSyncAt: Date.now() - 4 * 60_000,
    pending: 0,
  };
  switch (q) {
    case "off":
      return { ...base, enabled: false, active: false, account: "unknown" };
    case "noAccount":
      return { ...base, active: false, account: "noAccount" };
    case "quota":
      return { ...base, pending: 3, problem: { kind: "quotaExceeded" } };
    case "unavailable":
      return {
        ...base,
        available: false,
        unavailableReason: "This build of Penguin has no iCloud container.",
        enabled: true,
        active: false,
        account: "unknown",
        lastSyncAt: undefined,
      };
    default:
      return base;
  }
}

let status: CloudStatus | null = null;
const current = () => (status ??= initial());

function set(next: CloudStatus): CloudStatus {
  status = next;
  mockBackend.emit(EVENT, next);
  return next;
}

export const cloudHandlers: Record<string, MockHandler> = {
  cloud_status: async () => current(),
  set_cloud_sync: async (args) => {
    const enabled = Boolean((args as { enabled?: boolean }).enabled);
    const s = current();
    const running = s.available && enabled;
    return set({
      ...s,
      enabled,
      active: running && s.account !== "noAccount",
      account: running ? (s.account === "unknown" ? "available" : s.account) : "unknown",
      problem: enabled && s.problem?.kind === "dataDeleted" ? undefined : s.problem,
    });
  },
  cloud_sync_now: async () => {
    const s = current();
    if (!s.available) throw { code: "invalidInput", message: s.unavailableReason ?? "Unavailable" };
    return set({ ...s, lastSyncAt: Date.now(), pending: 0 });
  },
};
