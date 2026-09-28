// The search overlay's "Calendar" group: events matching the query (all of
// the results for type:event). Rows take the overlay's item props, so they
// join its keyboard selection. OWNER: calendar agent.
import type { Account, CalendarEvent } from "../../lib/types";
import { accountTone } from "../../lib/accountColor";
import { dayLabel, startOfDay, timeRange } from "./time";
import "./calendar.css";

/** Stable id for a result row (the overlay's item ids). */
export function eventItemId(e: CalendarEvent): string {
  return `e:${e.accountId}/${e.calendarId}/${e.id}`;
}

interface RowProps {
  "data-sid": string;
  onMouseDown: (e: { preventDefault(): void }) => void;
  onMouseMove: () => void;
  onClick: (e: { metaKey: boolean; ctrlKey: boolean }) => void;
  active: boolean;
}

const dateFmt = new Intl.DateTimeFormat(undefined, { weekday: "short", month: "short", day: "numeric" });
const yearFmt = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" });

function dateOf(e: CalendarEvent, now: number): string {
  const d = startOfDay(e.start);
  const near = Math.abs(d - startOfDay(now)) <= 86_400_000;
  if (near) return dayLabel(d, now);
  return new Date(e.start).getFullYear() === new Date(now).getFullYear() ? dateFmt.format(e.start) : yearFmt.format(e.start);
}

export function EventResults({
  events,
  accounts,
  itemProps,
}: {
  events: CalendarEvent[];
  accounts: Record<string, Account | undefined>;
  itemProps: (id: string) => RowProps;
}) {
  const now = Date.now();
  return (
    <div className="hits ev-results">
      {events.map((e) => {
        const { active, ...rest } = itemProps(eventItemId(e));
        const acc = accounts[e.accountId];
        return (
          <div key={eventItemId(e)} className={`hit sx-row evr${active ? " is-active" : ""}${e.end < now ? " is-past" : ""}`} {...rest}>
            <span className="evr-date tnum">
              <i className={`dot dot-sm t-${acc ? accountTone(acc.color) : "gray"}`} title={acc?.email} />
              {dateOf(e, now)}
            </span>
            <span className="hit-main truncate">
              <span className={"hit-subj" + (e.myResponse === "declined" ? " is-declined" : "")}>{e.summary || "(no title)"}</span>
              {e.location && <span className="hit-snip"> — {e.location}</span>}
            </span>
            <span className="hit-meta tnum">{e.allDay ? "All day" : timeRange(e)}</span>
          </div>
        );
      })}
    </div>
  );
}
