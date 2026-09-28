// Event details: a popover anchored to the event (agenda row, week block,
// search result, invite card, person meetings), or centered without an
// anchor. Text only: the description is rendered as text nodes with https
// links, never as HTML. OWNER: calendar agent.
//
//   openEvent(event, anchor?)  (state.ts)  →  <EventPopoverHost/> (App)
import { useEffect, useLayoutEffect, useRef, useState } from "react";
import type { CalendarEvent, EventAttendee, EventDetail } from "../../lib/types";
import { api, asCommandError } from "../../lib/api";
import { openThread } from "../../lib/ui";
import { registerShortcuts } from "../../lib/keyboard";
import { useDismiss } from "../../lib/dismiss";
import { Icon } from "../../components/Icon";
import { AccountBadge, Avatar } from "../../components/Identity";
import { accountById, isMe } from "../../app/store";
import { closeEvent, isEventOpen, useCalendarVersion, useOpenEvent, type OpenEvent } from "./state";
import { duration, isUrl, joinLabel, linkify, mapUrl, relative, whenLabel } from "./time";
import "./calendar.css";

const W = 380;

export function EventPopoverHost() {
  const open = useOpenEvent();
  if (!open) return null;
  return <Popover key={`${open.event.accountId}/${open.event.id}`} open={open} />;
}

function Popover({ open }: { open: OpenEvent }) {
  const ref = useRef<HTMLDivElement>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const detail = useDetail(open.event);
  const event = detail?.event ?? open.event;

  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const h = el.offsetHeight;
    const m = 12;
    if (!open.rect) {
      setPos({ left: (window.innerWidth - W) / 2, top: Math.max(m, (window.innerHeight - h) / 3) });
      return;
    }
    const r = open.rect;
    // Beside the anchor when there's room (week blocks), else below/above it.
    let left = r.right + 8;
    if (left + W + m > window.innerWidth) left = r.left - W - 8;
    if (left < m) left = Math.min(Math.max(m, r.left), window.innerWidth - W - m);
    const beside = left !== Math.min(Math.max(m, r.left), window.innerWidth - W - m);
    let top = beside ? r.top : r.bottom + 6;
    if (top + h + m > window.innerHeight) top = beside ? window.innerHeight - h - m : r.top - h - 6;
    setPos({ left, top: Math.max(m, top) });
  }, [open.rect, detail]);

  // The row or block it hangs from toggles it (openEvent).
  useDismiss(true, closeEvent, [ref, () => open.anchor]);
  useEffect(() => {
    ref.current?.focus({ preventScroll: true });
    return registerShortcuts([
      { id: "event.close", keys: "escape", label: "Close event", group: "Calendar", hidden: true, allowInInput: true, when: isEventOpen, run: closeEvent },
    ]);
  }, []);

  return (
    <div
      ref={ref}
      className="panel ev-pop"
      role="dialog"
      aria-label={event.summary || "Event"}
      tabIndex={-1}
      style={{ left: pos?.left ?? -9999, top: pos?.top ?? -9999, width: W }}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.preventDefault();
          closeEvent();
        } else if (e.key !== "Tab") e.stopPropagation();
      }}
    >
      <EventDetails event={event} detail={detail} />
    </div>
  );
}

function useDetail(event: CalendarEvent): EventDetail | null {
  const v = useCalendarVersion();
  const [detail, setDetail] = useState<EventDetail | null>(null);
  useEffect(() => {
    if (!event.id || !event.calendarId) return;
    let live = true;
    api.getEvent(event.accountId, event.calendarId, event.id).then(
      (d) => live && setDetail(d),
      () => {},
    );
    return () => {
      live = false;
    };
  }, [event.accountId, event.calendarId, event.id, v]);
  return detail;
}

const RESPONSE: Record<EventAttendee["response"], { label: string; icon: "check" | "x" | "clock" | null; cls: string }> = {
  accepted: { label: "Going", icon: "check", cls: "is-yes" },
  declined: { label: "Declined", icon: "x", cls: "is-no" },
  tentative: { label: "Maybe", icon: "clock", cls: "is-maybe" },
  needsAction: { label: "No reply", icon: null, cls: "is-wait" },
};

export function responseLabel(r: CalendarEvent["myResponse"]): string | null {
  return r ? RESPONSE[r].label : null;
}

/** Plain text with https links as buttons that open in the browser. */
export function LinkedText({ text }: { text: string }) {
  return (
    <>
      {linkify(text).map((s, i) =>
        s.href ? (
          <a key={i} className="ev-link" href={s.href} onClick={(e) => (e.preventDefault(), void api.openExternal(s.href!))}>
            {s.text}
          </a>
        ) : (
          <span key={i}>{s.text}</span>
        ),
      )}
    </>
  );
}

function EventDetails({ event, detail }: { event: CalendarEvent; detail: EventDetail | null }) {
  const now = Date.now();
  const acct = accountById(event.accountId);
  const people = event.attendees.filter((a) => !a.resource);
  const rooms = event.attendees.filter((a) => a.resource);
  const [showAll, setShowAll] = useState(false);
  const shown = showAll ? people : people.slice(0, 8);
  const counts = people.reduce<Record<string, number>>((m, a) => ((m[a.response] = (m[a.response] ?? 0) + 1), m), {});
  const running = event.start <= now && event.end > now;
  const color = detail?.calendar?.color ?? null;

  return (
    <>
      <div className="ev-head">
        <i className="ev-swatch" style={color ? { background: color } : undefined} />
        <div className="grow min0">
          <div className={"ev-title" + (event.myResponse === "declined" ? " is-declined" : "")}>{event.summary || "(no title)"}</div>
          <div className="ev-when">
            {whenLabel(event, now)}
            <span className="faint"> · {duration(event)}</span>
          </div>
          {!event.allDay && (
            <div className="faint small">{running ? `Started ${relative(event.start, now)}, ends ${relative(event.end, now)}` : event.end < now ? `Ended ${relative(event.end, now)}` : `Starts ${relative(event.start, now)}`}</div>
          )}
        </div>
        <button className="btn btn-ghost btn-sm btn-icon" title="Close" aria-label="Close" onClick={closeEvent}>
          <Icon name="x" size="xs" />
        </button>
      </div>

      {event.conferenceUrl && (
        <button className="btn btn-primary btn-sm ev-join" onClick={() => void api.openExternal(event.conferenceUrl!)}>
          <Icon name="external" size="xs" />
          {joinLabel(event.conferenceKind)}
        </button>
      )}

      <div className="ev-facts">
        {event.location && (
          <div className="pc-fact">
            <Icon name="pin" size="xs" />
            <span className="grow min0 ev-wrap">{isUrl(event.location) ? <LinkedText text={event.location} /> : event.location}</span>
            {!isUrl(event.location) && (
              <button className="btn btn-ghost btn-sm" title="Open in Google Maps" onClick={() => void api.openExternal(mapUrl(event.location))}>
                Map
              </button>
            )}
          </div>
        )}
        {rooms.length > 0 && (
          <div className="pc-fact">
            <Icon name="home" size="xs" />
            <span>{rooms.map((r) => r.name ?? r.email).join(", ")}</span>
          </div>
        )}
        <div className="pc-fact">
          <Icon name="calendar" size="xs" />
          <span className="grow min0 truncate">
            {detail?.calendar?.summary ?? "Calendar"}
            {event.myResponse && <span className="faint"> · You: {RESPONSE[event.myResponse].label.toLowerCase()}</span>}
          </span>
          {acct && <AccountBadge account={acct} />}
        </div>
      </div>

      {people.length > 0 && (
        <div className="pc-section">
          <div className="pc-title">
            {people.length} {people.length === 1 ? "guest" : "guests"}
            <span className="faint">
              {" "}
              · {[counts.accepted && `${counts.accepted} going`, counts.tentative && `${counts.tentative} maybe`, counts.declined && `${counts.declined} declined`, counts.needsAction && `${counts.needsAction} awaiting`].filter(Boolean).join(", ")}
            </span>
          </div>
          <ul className="ev-people">
            {shown.map((a) => {
              const r = RESPONSE[a.response];
              return (
                <li key={a.email} title={`${a.email} · ${r.label}`}>
                  <span className="ev-av">
                    <Avatar person={{ name: a.name, email: a.email }} size="sm" photo={!isMe(a.email)} tone={isMe(a.email) ? "gray" : undefined} />
                    {r.icon && (
                      <span className={"ev-resp " + r.cls}>
                        <Icon name={r.icon} size="2xs" />
                      </span>
                    )}
                  </span>
                  <span className="truncate grow">
                    {a.self ? "You" : a.name ?? a.email}
                    {a.organizer && <span className="faint"> · organizer</span>}
                    {a.optional && <span className="faint"> · optional</span>}
                  </span>
                </li>
              );
            })}
          </ul>
          {people.length > shown.length && (
            <button className="pc-inline-link small" onClick={() => setShowAll(true)}>
              Show all {people.length}
            </button>
          )}
        </div>
      )}

      {event.description && (
        <div className="pc-section">
          <div className="ev-desc">
            <LinkedText text={event.description} />
          </div>
        </div>
      )}

      <div className="pc-actions ev-actions">
        {detail?.thread && (
          <button
            className="btn btn-secondary btn-sm"
            title={detail.thread.subject}
            onClick={() => {
              closeEvent();
              openThread({ accountId: detail.thread!.accountId, threadId: detail.thread!.threadId });
            }}
          >
            <Icon name="mail" size="xs" />
            Invitation email
          </button>
        )}
        <span className="grow" />
        {event.htmlLink && (
          <button className="btn btn-ghost btn-sm" onClick={() => void api.openExternal(event.htmlLink!).catch((e) => console.warn(asCommandError(e).message))}>
            <Icon name="external" size="xs" />
            Google Calendar
          </button>
        )}
      </div>
    </>
  );
}
