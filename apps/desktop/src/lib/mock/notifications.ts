// Mock new-mail notifications (src-tauri/src/notify.rs): the permission
// answers and the screen context. ?notifyPermission=denied (or localStorage
// penguin.mock.notifyDenied=1) shows the "turned off in System Settings"
// state; a test notification becomes a toast.
import type { NotificationPermission, NotifyContext } from "../types";
import type { MockHandler } from "./index";

function permission(): NotificationPermission {
  const q = typeof location === "undefined" ? null : new URLSearchParams(location.search).get("notifyPermission");
  let stored = false;
  try {
    stored = localStorage.getItem("penguin.mock.notifyDenied") === "1";
  } catch {
    // Storage unavailable: granted.
  }
  return q === "denied" || stored ? "denied" : "granted";
}

/** The last context the UI reported (for poking at in the console). */
export let mockNotifyContext: NotifyContext = { inboxAccounts: [], thread: null };

export const notificationHandlers: Record<string, MockHandler> = {
  set_notify_context: ({ context }) => {
    mockNotifyContext = context as NotifyContext;
  },
  notification_permission: () => permission(),
  request_notification_permission: () => permission(),
  test_notification: async () => {
    const { toast } = await import("../../components/Toast");
    toast({ message: "Penguin", detail: "New mail will show up like this. (A notification in the app.)" });
  },
};
