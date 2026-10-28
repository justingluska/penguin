// What Settings → Accounts → Sync with iCloud says (CloudSync.tsx), kept
// pure so it can be tested.
import { ago } from "../../lib/format.ts";
import type { CloudStatus } from "../../lib/types";

export interface CloudLine {
  text: string;
  /** Something the user may need to fix. */
  warn: boolean;
}

/** One line under the switch: why it can't run, what's wrong, or when it last synced. */
export function cloudLine(s: CloudStatus, now = Date.now()): CloudLine {
  if (!s.available) return { text: s.unavailableReason ?? "Sync with iCloud isn't available in this build.", warn: false };
  if (s.problem?.kind === "dataDeleted") {
    return { text: "Penguin's data was deleted from iCloud, so sync turned off. Turn it on to upload this Mac's copy.", warn: true };
  }
  if (!s.enabled) return { text: "Off: snoozes, Follow up and settings stay on this Mac.", warn: false };
  switch (s.account) {
    case "noAccount":
      return { text: "Sign in to iCloud in System Settings to sync. Until then this Mac works on its own.", warn: true };
    case "restricted":
      return { text: "iCloud is restricted on this Mac (Screen Time or a device profile).", warn: true };
    case "temporarilyUnavailable":
      return { text: "iCloud needs attention in System Settings (a password or new terms).", warn: true };
    default:
      break;
  }
  const pending = s.pending > 0 ? ` · ${s.pending} change${s.pending === 1 ? "" : "s"} waiting` : "";
  switch (s.problem?.kind) {
    case "quotaExceeded":
      return { text: `iCloud storage is full; changes wait until there's room${pending}.`, warn: true };
    case "notSetUp":
      return { text: `iCloud isn't ready for Penguin yet (${s.problem.detail}). Changes wait here${pending}.`, warn: true };
    case "other":
      return { text: `iCloud refused the last change (${s.problem.detail}). Penguin tries again${pending}.`, warn: true };
    default:
      break;
  }
  if (s.account === "unknown") return { text: "Checking iCloud…", warn: false };
  const when = s.lastSyncAt ? `Last synced ${ago(s.lastSyncAt, now)}` : "Not synced yet";
  return { text: `${when}${pending}. Snoozes, Follow up, snippets, signatures and settings like the split inbox follow you to iPhone.`, warn: false };
}
