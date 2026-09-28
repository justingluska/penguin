// "Last meeting / Next meeting with X" for the person card and the thread's
// context panel. Local only (person_meetings); nothing renders when there's
// no event with them. OWNER: calendar agent.
import { useEffect, useState } from "react";
import type { CalendarEvent, PersonMeetings as Meetings } from "../../lib/types";
import { api } from "../../lib/api";
import { Icon } from "../../components/Icon";
import { openEvent, useCalendarVersion } from "./state";
import { relative, timeOf } from "./time";
import "./calendar.css";

const cache = new Map<string, Meetings>();

export function usePersonMeetings(email: string | null): Meetings | null {
  const v = useCalendarVersion();
  const key = email?.trim().toLowerCase() ?? null;
  const [m, setM] = useState<Meetings | null>(key ? cache.get(key) ?? null : null);
  useEffect(() => {
    if (!key) return setM(null);
    setM(cache.get(key) ?? null);
    let live = true;
    api.personMeetings(key).then(
      (r) => {
        cache.set(key, r);
        if (live) setM(r);
      },
      () => {},
    );
    return () => {
      live = false;
    };
  }, [key, v]);
  return m;
}

const dayFmt = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric" });

function when(e: CalendarEvent, now: number): string {
  const day = dayFmt.format(e.start);
  return e.allDay ? `${day} (${relative(e.start, now)})` : `${day}, ${timeOf(e.start)} (${relative(e.start, now)})`;
}

/**
 * `variant`: "card" uses the person card's rows (pc-*), "panel" the thread
 * context panel's (ctx-*).
 */
export function PersonMeetings({ email, variant }: { email: string; variant: "card" | "panel" }) {
  const m = usePersonMeetings(email);
  if (!m || (!m.last && !m.next)) return null;
  const now = Date.now();
  const rows = [
    m.next && { label: "Next meeting", e: m.next },
    m.last && { label: "Last meeting", e: m.last },
  ].filter(Boolean) as { label: string; e: CalendarEvent }[];
  const title = (
    <>
      <span>Meetings</span>
      <span className="faint tnum">{m.count}</span>
    </>
  );
  return (
    <div className={variant === "card" ? "pc-section" : "ctx-section"}>
      <div className={variant === "card" ? "pc-title pm-title" : "ctx-title"}>{title}</div>
      {rows.map(({ label, e }) => (
        <button key={label} className={(variant === "card" ? "pc-row" : "ctx-row") + " pm-row"} onClick={(ev) => openEvent(e, ev.currentTarget)}>
          <Icon name="calendar" size="xs" />
          <span className="grow min0 pm-text">
            <span className="faint">{label}: </span>
            <span className="truncate">{e.summary || "(no title)"}</span>
            <span className="faint small pm-when">{when(e, now)}</span>
          </span>
        </button>
      ))}
    </div>
  );
}
