// Status bar chip: "Next up: 3:00 PM Weekly sync (in 25 min)", or "Now:"
// with a Join link while a meeting runs. Busy timed events in the next 12
// hours only; hidden when Settings → Calendar → Next up is off or nothing
// is connected. OWNER: calendar agent.
import { useEffect, useState } from "react";
import { api } from "../../lib/api";
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
  const next = events ? nextUp(events, now) : null;
  if (!next) return null;
  const e = next.event;
  return (
    <span className="nextup">
      <button className="nextup-btn" title={`${e.summary}${e.location ? " · " + e.location : ""}`} onClick={(ev) => openEvent(e, ev.currentTarget)}>
        <span className={"nextup-dot" + (next.running ? " is-now" : "")} />
        <span className="faint">{next.running ? "Now:" : "Next up:"}</span>
        {!next.running && <span className="tnum">{timeOf(e.start)}</span>}
        <span className="truncate nextup-title">{e.summary || "(no title)"}</span>
        <span className="faint">({next.running ? `ends ${relative(e.end, now)}` : relative(e.start, now)})</span>
      </button>
      {e.conferenceUrl && (next.running || e.start - now < 15 * MIN) && (
        <button className="nextup-join" onClick={() => void api.openExternal(e.conferenceUrl!)}>
          {joinLabel(e.conferenceKind)}
        </button>
      )}
    </span>
  );
}
