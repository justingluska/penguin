// Settings → General → Notifications: "Notify me about new mail" (off by
// default), then per-account switches, "Only people I've emailed before",
// and a test notification. Turning it on asks for permission; macOS can't
// report a refusal for these notifications, so the section always says
// where to allow them if nothing shows up.
import { useState } from "react";
import { api, asCommandError } from "../../lib/api";
import { updateSettings, useSettings } from "../../lib/settings";
import type { NotificationPermission, NotificationSettings as Section } from "../../lib/types";
import { toast } from "../../components/Toast";
import { AccountAvatar, accountName } from "../../components/Identity";
import { meta } from "../../app/store";
import { Switch } from "../settings/parts";
import { SYSTEM_SETTINGS_HELP, accountNotifies, withAccount } from "./rules";
import "./notifications.css";

function save(notifications: Section) {
  updateSettings({ notifications }).catch((e) => toast({ tone: "error", message: asCommandError(e).message }));
}

export function NotificationSettings() {
  const s = useSettings();
  const n = s.notifications;
  const accounts = meta.use((m) => m.accounts);
  const [permission, setPermission] = useState<NotificationPermission | null>(null);

  async function turn(on: boolean) {
    save({ ...n, enabled: on });
    if (!on) return;
    try {
      setPermission(await api.requestNotificationPermission());
    } catch (e) {
      toast({ tone: "error", message: `Couldn't ask for permission: ${asCommandError(e).message}` });
    }
  }

  async function test() {
    try {
      await api.testNotification();
    } catch (e) {
      toast({ tone: "error", message: `Couldn't show a notification: ${asCommandError(e).message}` });
    }
  }

  return (
    <div className="setting-row setting-tall nt-block">
      <div className="min0 grow">
        <div className="nt-head">
          <div className="min0">
            <span className="setting-label">Notify me about new mail</span>
            <p className="st-muted">
              A notification when new mail reaches your inbox, with the sender and subject (never the message). Nothing while
              you're looking at it, and several at once come as one. Click one to open it.
            </p>
          </div>
          <Switch label="Notify me about new mail" on={n.enabled} onChange={(on) => void turn(on)} />
        </div>

        {n.enabled && (
          <>
            {permission === "denied" ? (
              <p className="nt-warn" role="alert">
                Notifications are turned off for Penguin. To see them, open {SYSTEM_SETTINGS_HELP}.
              </p>
            ) : (
              <p className="st-muted nt-help">
                Nothing showing up? Check {SYSTEM_SETTINGS_HELP}.{" "}
                <button className="btn btn-ghost btn-sm nt-test" onClick={() => void test()}>
                  Send a test notification
                </button>
              </p>
            )}

            <div className="nt-option">
              <div className="min0">
                <span className="setting-label">Only people I've emailed before</span>
                <p className="st-muted">Quiet for newsletters and strangers: only senders you've written to, from any account.</p>
              </div>
              <Switch
                label="Only people I've emailed before"
                on={n.knownSendersOnly}
                onChange={(knownSendersOnly) => save({ ...n, knownSendersOnly })}
              />
            </div>

            {accounts.length > 0 && (
              <ul className="nt-accounts" aria-label="Accounts that notify">
                {accounts.map((a) => {
                  const on = accountNotifies(n, s.hiddenFromAll, a.id);
                  const hidden = s.hiddenFromAll.includes(a.id);
                  return (
                    <li key={a.id}>
                      <AccountAvatar account={a} accounts={accounts} />
                      <span className="truncate nt-acct">{accountName(a, accounts)}</span>
                      <span className="st-muted truncate nt-state">
                        {a.email}
                        {hidden && " · hidden from All Inboxes, so off unless you turn it on"}
                      </span>
                      <Switch
                        label={`Notify for ${accountName(a, accounts)}`}
                        on={on}
                        onChange={(v) => save(withAccount(n, s.hiddenFromAll, a.id, v))}
                      />
                    </li>
                  );
                })}
              </ul>
            )}
          </>
        )}
      </div>
    </div>
  );
}
