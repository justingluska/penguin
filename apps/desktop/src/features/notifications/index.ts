// New-mail notifications, the UI half: tell the backend what's on screen
// (so mail you're already looking at doesn't notify) and open what a
// clicked notification points at. Started once by App after the mail loads.
// The rules for what notifies live in src-tauri/src/notify.rs.
import { api, onNotificationOpen } from "../../lib/api";
import { getUi, openThread, setUi, subscribeUi } from "../../lib/ui";
import { listScope, meta } from "../../app/store";
import { switchAccount } from "../../app/shortcuts";
import type { NotificationOpen } from "../../lib/types";
import { notifyContextOf } from "./rules";

let started = false;

/** Open what a clicked notification points at: its thread, or for a grouped one the account's inbox. */
export function openFromNotification(e: NotificationOpen) {
  if (e.threadId) {
    setUi({ surface: "mail" });
    openThread({ accountId: e.accountId, threadId: e.threadId });
    return;
  }
  setUi({ surface: "mail", view: { kind: "inbox" }, overlay: null, threadOpen: false });
  switchAccount(e.accountId);
}

export function startNotifications() {
  if (started) return;
  started = true;
  let last = "";
  let timer: ReturnType<typeof setTimeout> | null = null;
  const report = () => {
    timer = null;
    const ui = getUi();
    const ctx = notifyContextOf({
      surface: ui.surface,
      view: ui.view,
      overlay: ui.overlay,
      threadOpen: ui.threadOpen,
      selected: ui.selected,
      accountFilter: ui.accountFilter,
      scope: listScope(ui),
      accountIds: meta.get().accounts.map((a) => a.id),
    });
    const key = JSON.stringify(ctx);
    if (key === last) return;
    last = key;
    // Best effort: a missed update only means one notification too many or too few.
    api.setNotifyContext(ctx).catch(() => {
      last = "";
    });
  };
  // Coalesce bursts of UI updates (arrow keys through the list) into one call.
  const schedule = () => {
    if (!timer) timer = setTimeout(report, 150);
  };
  subscribeUi(schedule);
  meta.subscribe(schedule);
  schedule();
  void onNotificationOpen(openFromNotification);
}
