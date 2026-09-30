// Status bar chip: "Next up: 3:00 PM Weekly sync (in 25 min)", or "Now:"
// with a Join link while a meeting runs; "Now: Flight +1 · Next: 3:00 PM
// Sync (in 25 min)" when a meeting starts during what's running. Busy timed events in the next 12
// hours only; hidden when Settings → Calendar → Next up is off or nothing
// is connected. OWNER: calendar agent.
import { useEffect, useState } from "react";
import { api } from "../../lib/api";
import type { CalendarEvent } from "../../lib/types";
import { useSetting } from "../../lib/settings";
import { anyConnected, openEvent, useCalendarScope, useCalendarStatus, useEvents } from "./state";
import { HOUR, MIN, joinLabel, nextUp, relative, timeOf } from "./time";
import "./calendar.css";

export function NextUp() {
  const on = useSetting("calendar")?.nextUp ?? true;
  const status = useCalendarStatus();
  if (!on || !anyConnected(status)) return null;
  return <Chip />;
}

function Chip() {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(t);
  }, []);
  // A stable range per 10 minutes, so the cache is reused between ticks.
  const from = Math.floor(now / (10 * MIN)) * 10 * MIN - 4 * HOUR;
  const scope = useCalendarScope();
  const { events } = useEvents(from, from + 17 * HOUR, scope);
  const up = events ? nextUp(events, now) : null;
  if (!up) return null;
  const [cur, ...others] = up.running;
  const next = up.next;
  // Join the meeting about to start over the one already running.
  const nextSoon = next && next.start - now < 15 * MIN;
  const join = next?.conferenceUrl && nextSoon ? next : cur?.conferenceUrl ? cur : null;
  return (
    <span className={"nextup" + (cur && next ? " is-both" : "")}>
      {cur && (
        <button className="nextup-btn" title={tip(cur)} onClick={(ev) => openEvent(cur, ev.currentTarget)}>
          <span className="nextup-dot is-now" />
          <span className="faint">Now:</span>
          <span className="truncate nextup-title">{cur.summary || "(no title)"}</span>
          {!next && <span className="faint">(ends {relative(cur.end, now)})</span>}
        </button>
      )}
      {others.length > 0 && (
        <button
          className="nextup-more"
          title={others.map((e) => `${e.summary || "(no title)"}, ends ${relative(e.end, now)}`).join("\n")}
          onClick={(ev) => openEvent(others[0], ev.currentTarget)}
        >
          +{others.length}
        </button>
      )}
      {cur && next && <span className="nextup-sep faint">·</span>}
      {next && (
        <button className="nextup-btn" title={tip(next)} onClick={(ev) => openEvent(next, ev.currentTarget)}>
          {!cur && <span className="nextup-dot" />}
          <span className="faint">{cur ? "Next:" : "Next up:"}</span>
          <span className="tnum">{timeOf(next.start)}</span>
          <span className="truncate nextup-title">{next.summary || "(no title)"}</span>
          <span className="faint">({relative(next.start, now)})</span>
        </button>
      )}
      {join && (
        <button className="nextup-join" onClick={() => void api.openExternal(join.conferenceUrl!)}>
          {joinLabel(join.conferenceKind)}
        </button>
      )}
    </span>
  );
}

function tip(e: CalendarEvent): string {
  return `${e.summary}${e.location ? " · " + e.location : ""}`;
}
