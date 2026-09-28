// Settings → Calendar: connect Google Calendar per account (read-only), the
// separate RSVP opt-in, which calendars sync, the sync window and the status
// bar's "Next up". OWNER: calendar agent.
import { useCallback, useEffect, useState } from "react";
import { api, asCommandError } from "../../lib/api";
import { DEFAULT_SETTINGS, updateSettings, useSetting } from "../../lib/settings";
import type { CalendarSettings as Prefs, CalendarStatus } from "../../lib/types";
import { ago, num } from "../../lib/format";
import { toast } from "../../components/Toast";
import { Avatar, accountName } from "../../components/Identity";
import { meta } from "../../app/store";
import { Section, Switch } from "../settings/parts";
import { invalidateCalendar, loadCalendarStatus, useCalendarVersion } from "./state";
import "./calendar.css";
import { Select } from "../../components/Select";

const PAST = [3, 6, 12, 24, 36, 60];
const FUTURE = [3, 6, 12, 24, 36];

function months(n: number): string {
  return n % 12 === 0 ? `${n / 12} ${n === 12 ? "year" : "years"}` : `${n} months`;
}

function save(patch: Partial<Prefs>, current: Prefs) {
  updateSettings({ calendar: { ...current, ...patch } }).then(
    () => void api.calendarSyncNow(),
    (e) => toast({ tone: "error", message: asCommandError(e).message }),
  );
}

export function CalendarSection() {
  const prefs = useSetting("calendar") ?? DEFAULT_SETTINGS.calendar;
  const accounts = meta.use((m) => m.accounts);
  const v = useCalendarVersion();
  const [status, setStatus] = useState<CalendarStatus | null>(null);
  const [busy, setBusy] = useState<{ id: string; rsvp: boolean } | null>(null);

  const refresh = useCallback(() => {
    loadCalendarStatus(true).then(setStatus, () => setStatus(null));
  }, []);
  useEffect(refresh, [refresh, v]);
  // Show progress while a first sync runs.
  useEffect(() => {
    if (!status?.accounts.some((a) => a.syncing || (a.granted && a.syncedAt === null && !a.error))) return;
    const t = setTimeout(refresh, 2000);
    return () => clearTimeout(t);
  }, [status, refresh]);

  async function connect(accountId: string, rsvp: boolean) {
    setBusy({ id: accountId, rsvp });
    try {
      setStatus(await api.connectCalendar(accountId, rsvp));
      invalidateCalendar();
      toast({ message: rsvp ? "RSVP turned on" : "Calendar connected" });
    } catch (e) {
      const err = asCommandError(e);
      if (err.code !== "cancelled") toast({ tone: "error", message: err.message });
    } finally {
      setBusy(null);
    }
  }

  async function select(accountId: string, calendarId: string, selected: boolean) {
    try {
      setStatus(await api.setCalendarSelected(accountId, calendarId, selected));
    } catch (e) {
      toast({ tone: "error", message: asCommandError(e).message });
    }
  }

  return (
    <Section id="calendar" icon="calendar" title="Calendar" badge={<span className="badge t-gray">Read-only</span>}>
      <div className="setting-row setting-tall">
        <div className="min0 grow">
          <span className="setting-label">Google Calendar</span>
          <p className="st-muted">
            Read-only, per account, kept on this Mac. Your Google Cloud project needs the Google Calendar API turned on.
          </p>
          <ul className="sp-accounts calset-accounts">
            {accounts.map((a) => {
              const st = status?.accounts.find((x) => x.accountId === a.id);
              const connecting = busy?.id === a.id;
              return (
                <li key={a.id} className="calset-acct">
                  <div className="calset-row">
                    <Avatar person={{ name: accountName(a), email: a.email }} size="xs" photo={false} tone="gray" />
                    <span className="truncate sp-acct">{accountName(a)}</span>
                    <span className={"st-muted truncate sp-state" + (st?.error ? " calset-error" : "")}>
                      {st?.error
                        ? st.error
                        : st?.granted
                          ? st.syncing || st.syncedAt === null
                            ? "Connected · syncing…"
                            : `${num(st.events)} ${st.events === 1 ? "event" : "events"} · synced ${ago(st.syncedAt)}`
                          : "Not connected"}
                    </span>
                    {connecting ? (
                      <button className="btn btn-ghost btn-sm" onClick={() => void api.cancelSignIn()}>
                        Cancel
                      </button>
                    ) : (
                      (!st?.granted || st.error) && (
                        <button className="btn btn-secondary btn-sm" disabled={busy !== null} onClick={() => void connect(a.id, false)}>
                          {st?.granted ? "Reconnect" : "Connect calendar"}
                        </button>
                      )
                    )}
                  </div>
                  {st?.granted && (
                    <>
                      <ul className="calset-cals">
                        {st.calendars.map((c) => (
                          <li key={c.id}>
                            <label className="calset-cal">
                              <input
                                type="checkbox"
                                checked={c.selected}
                                disabled={c.accessRole === "freeBusyReader"}
                                onChange={(e) => void select(a.id, c.id, e.target.checked)}
                              />
                              <i className="calset-swatch" style={c.color ? { background: c.color } : undefined} />
                              <span className="truncate">{c.summary}</span>
                              {c.primary && <span className="faint small">primary</span>}
                              {c.accessRole === "freeBusyReader" && <span className="faint small">free/busy only</span>}
                            </label>
                          </li>
                        ))}
                      </ul>
                      <div className="calset-rsvp">
                        <div className="min0">
                          <span className="setting-label">Answer invitations from Penguin</span>
                          <p className="st-muted">
                            {st.rsvpGranted
                              ? "On. Accept, Maybe and Decline appear on invitations; Google tells the organizer."
                              : "Adds Accept / Maybe / Decline to invitation cards. Needs a second permission to edit your events, which Penguin only uses to send your answer."}
                          </p>
                        </div>
                        {st.rsvpGranted ? (
                          <span className="badge t-green">Allowed</span>
                        ) : connecting && busy?.rsvp ? (
                          <button className="btn btn-ghost btn-sm" onClick={() => void api.cancelSignIn()}>
                            Cancel
                          </button>
                        ) : (
                          <button className="btn btn-secondary btn-sm" disabled={busy !== null} onClick={() => void connect(a.id, true)}>
                            Allow RSVP
                          </button>
                        )}
                      </div>
                    </>
                  )}
                </li>
              );
            })}
          </ul>
        </div>
      </div>

      <div className="setting-row">
        <div>
          <span className="setting-label">Connect calendar when adding accounts</span>
          <p className="st-muted">
            Turn off if a Workspace admin blocks Calendar: Google then refuses the whole sign-in.
          </p>
        </div>
        <Switch
          label="Connect calendar when adding accounts"
          on={prefs.connectOnSignIn}
          onChange={(connectOnSignIn) => save({ connectOnSignIn }, prefs)}
        />
      </div>
      <div className="setting-row">
        <div>
          <span className="setting-label">Keep past events</span>
          <p className="st-muted">For "when did I last meet…" and search.</p>
        </div>
        <Select
          className="st-select"
          label="Keep past events"
          value={String(prefs.pastMonths)}
          options={PAST.map((n) => ({ value: String(n), label: months(n) }))}
          onChange={(v) => save({ pastMonths: Number(v) }, prefs)}
        />
      </div>
      <div className="setting-row">
        <div>
          <span className="setting-label">Sync upcoming events</span>
          <p className="st-muted">How far ahead recurring meetings are expanded.</p>
        </div>
        <Select
          className="st-select"
          label="Sync upcoming events"
          value={String(prefs.futureMonths)}
          options={FUTURE.map((n) => ({ value: String(n), label: months(n) }))}
          onChange={(v) => save({ futureMonths: Number(v) }, prefs)}
        />
      </div>
      <div className="setting-row">
        <div>
          <span className="setting-label">Next up in the status bar</span>
          <p className="st-muted">"Next up: 3:00 PM Weekly sync (in 25 min)", with a Join button just before it starts.</p>
        </div>
        <Switch label="Next up in the status bar" on={prefs.nextUp} onChange={(nextUp) => save({ nextUp }, prefs)} />
      </div>
    </Section>
  );
}
