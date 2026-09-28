// The Calendar surface (sidebar "Calendar", g c): an agenda of upcoming days,
// a week grid and a month grid, over every connected account in the current
// scope (account filter or profile). Reads only the local store (list_events).
// Keys: a agenda · w week · m month · t today · ←/→ move (day / week) ·
// j/k select · Enter open. In Month the selected day moves instead (←/→ j/k a
// day, ↑/↓ a week, ⇧←/⇧→ a month) and Enter opens it in Week. OWNER: calendar agent.
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type CSSProperties } from "react";
import type { CalendarEvent, CalendarStatus } from "../../lib/types";
import { getUi } from "../../lib/ui";
import { registerShortcuts } from "../../lib/keyboard";
import { accountTone } from "../../lib/accountColor";
import { Icon } from "../../components/Icon";
import { Kbd } from "../../components/Kbd";
import { accountById } from "../../app/store";
import { api } from "../../lib/api";
import { isSettingsOpen, openSettings } from "../settings/state";
import { anyConnected, calendarColor, isEventOpen, openEvent, useCalendarScope, useCalendarStatus, useEvents, takeCalendarFocus } from "./state";
import {
  HOUR,
  MIN,
  addDays,
  addMonths,
  byAllDayThenStart,
  dayLabel,
  dedupe,
  eventDays,
  fitMonthWeek,
  groupByDay,
  layoutDay,
  layoutMonthWeek,
  monthWeeks,
  startOfDay,
  startOfMonth,
  startOfWeek,
  timeOf,
  timeRange,
} from "./time";
import { responseLabel } from "./EventPopover";
import "./calendar.css";

type Mode = "agenda" | "week" | "month";
const MODES: [Mode, string, string][] = [
  ["agenda", "Agenda", "A"],
  ["week", "Week", "W"],
  ["month", "Month", "M"],
];
const MODE_KEY = "penguin.calendarMode";
const AGENDA_DAYS = 28;
const HOUR_PX = 48;
/** Month grid: the day-number row and one event line. */
const MO_HEAD_PX = 30;
const MO_LANE_PX = 20;

function storedMode(): Mode {
  try {
    const m = localStorage.getItem(MODE_KEY);
    return m === "week" || m === "month" ? m : "agenda";
  } catch {
    return "agenda";
  }
}

const inCalendar = () => {
  const ui = getUi();
  return ui.surface === "calendar" && !ui.threadOpen && (ui.overlay === null || ui.overlay === "command") && !isEventOpen() && !isSettingsOpen();
};

const evKey = (e: CalendarEvent) => `${e.accountId}/${e.calendarId}/${e.id}`;

/** Ticks every 30 s so the now-line and "past" dimming stay current. */
function useNow(): number {
  const [now, setNow] = useState(Date.now);
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), 30_000);
    return () => clearInterval(t);
  }, []);
  return now;
}

export function CalendarView() {
  const [mode, setModeState] = useState<Mode>(storedMode);
  const now = useNow();
  // "Open in Calendar" from an invite lands on the event's day, selected.
  const [initialFocus] = useState(takeCalendarFocus);
  const [anchor, setAnchor] = useState(() => startOfDay(initialFocus?.at ?? Date.now()));
  const [sel, setSel] = useState<string | null>(initialFocus?.key ?? null);
  const scope = useCalendarScope();
  const status = useCalendarStatus();

  // In Month, `anchor` is the selected day; the grid is its month.
  const month = startOfMonth(anchor);
  const weeks = useMemo(() => monthWeeks(month), [month]);
  const from = mode === "week" ? startOfWeek(anchor) : mode === "month" ? weeks[0] : anchor;
  const to = mode === "week" ? addDays(from, 7) : mode === "month" ? addDays(weeks[0], 7 * weeks.length) : addDays(from, AGENDA_DAYS);
  const { events: raw } = useEvents(from, to, scope);
  const events = useMemo(() => (raw ? dedupe(raw) : null), [raw]);
  // Keyboard order: by start, all-day first within a day.
  const ordered = useMemo(
    () => (events ?? []).slice().sort((a, b) => startOfDay(a.start) - startOfDay(b.start) || byAllDayThenStart(a, b)),
    [events],
  );

  const setMode = useCallback((m: Mode) => {
    setModeState(m);
    try {
      localStorage.setItem(MODE_KEY, m);
    } catch {
      // Storage unavailable: the choice lasts this session.
    }
  }, []);

  const move = useCallback(
    (dir: 1 | -1) => setAnchor((a) => (mode === "month" ? addMonths(a, dir) : addDays(a, dir * (mode === "week" ? 7 : 1)))),
    [mode],
  );
  const today = useCallback(() => setAnchor(startOfDay(Date.now())), []);
  // Month: move the selected day; leaving the month turns the page.
  const moveDay = useCallback((n: number) => setAnchor((a) => addDays(a, n)), []);
  const openDay = useCallback(
    (day: number) => {
      setAnchor(day);
      setMode("week");
    },
    [setMode],
  );

  const refs = useRef(new Map<string, HTMLElement>());
  const register = useCallback((k: string) => (el: HTMLElement | null) => {
    if (el) refs.current.set(k, el);
    else refs.current.delete(k);
  }, []);

  const step = useCallback(
    (d: 1 | -1) => {
      if (!ordered.length) return;
      const i = sel ? ordered.findIndex((e) => evKey(e) === sel) : -1;
      // The first j lands on the next event that hasn't ended.
      const j = i < 0 ? Math.max(0, ordered.findIndex((e) => e.end > Date.now())) : Math.min(ordered.length - 1, Math.max(0, i + d));
      const next = ordered[j];
      setSel(evKey(next));
      refs.current.get(evKey(next))?.scrollIntoView({ block: "nearest" });
    },
    [ordered, sel],
  );

  const openSelected = useCallback(() => {
    const e = ordered.find((x) => evKey(x) === sel);
    if (e) openEvent(e, refs.current.get(evKey(e)) ?? null);
  }, [ordered, sel]);

  const isMonth = mode === "month";

  useEffect(
    () =>
      registerShortcuts([
        { id: "cal.agenda", keys: "a", label: "Agenda", group: "Calendar", when: inCalendar, run: () => setMode("agenda") },
        { id: "cal.week", keys: "w", label: "Week", group: "Calendar", when: inCalendar, run: () => setMode("week") },
        { id: "cal.month", keys: "m", label: "Month", group: "Calendar", when: inCalendar, run: () => setMode("month") },
        { id: "cal.today", keys: "t", label: "Today", group: "Calendar", when: inCalendar, run: today },
        { id: "cal.next", keys: "arrowright", label: "Next day / week", group: "Calendar", when: inCalendar, run: () => (isMonth ? moveDay(1) : move(1)) },
        { id: "cal.prev", keys: "arrowleft", label: "Previous day / week", group: "Calendar", when: inCalendar, run: () => (isMonth ? moveDay(-1) : move(-1)) },
        { id: "cal.month.next", keys: "shift+arrowright", label: "Next month", group: "Calendar", when: () => inCalendar() && isMonth, run: () => move(1) },
        { id: "cal.month.prev", keys: "shift+arrowleft", label: "Previous month", group: "Calendar", when: () => inCalendar() && isMonth, run: () => move(-1) },
        { id: "cal.down", keys: "j", label: "Next event / day", group: "Calendar", when: inCalendar, run: () => (isMonth ? moveDay(1) : step(1)) },
        { id: "cal.up", keys: "k", label: "Previous event / day", group: "Calendar", when: inCalendar, run: () => (isMonth ? moveDay(-1) : step(-1)) },
        { id: "cal.down.arrow", keys: "arrowdown", label: "Next event", group: "Calendar", hidden: true, when: inCalendar, run: () => (isMonth ? moveDay(7) : step(1)) },
        { id: "cal.up.arrow", keys: "arrowup", label: "Previous event", group: "Calendar", hidden: true, when: inCalendar, run: () => (isMonth ? moveDay(-7) : step(-1)) },
        { id: "cal.open", keys: "enter", label: "Open event", group: "Calendar", when: () => inCalendar() && !isMonth && sel !== null, run: openSelected },
        { id: "cal.open.o", keys: "o", label: "Open event", group: "Calendar", hidden: true, when: () => inCalendar() && !isMonth && sel !== null, run: openSelected },
        { id: "cal.open.day", keys: "enter", label: "Open day in Week", group: "Calendar", when: () => inCalendar() && isMonth, run: () => openDay(anchor) },
      ]),
    [setMode, today, move, moveDay, openDay, step, openSelected, sel, isMonth, anchor],
  );

  const connected = anyConnected(status);
  const title =
    mode === "week"
      ? weekTitle(from)
      : isMonth
        ? monthTitle.format(month)
        : anchor === startOfDay(now)
        ? "Upcoming"
        : `From ${dayLabel(anchor, now)}`;

  return (
    <section className="cal" aria-label="Calendar">
      <header className="cal-head">
        <h1 className="cal-title">{title}</h1>
        <div className="cal-nav">
          <button className="btn btn-ghost btn-sm btn-icon" title="Previous" aria-label="Previous" onClick={() => move(-1)}>
            <Icon name="left" size="xs" />
          </button>
          <button className="btn btn-secondary btn-sm" onClick={today} title="Go to today (T)">
            Today
          </button>
          <button className="btn btn-ghost btn-sm btn-icon" title="Next" aria-label="Next" onClick={() => move(1)}>
            <Icon name="right" size="xs" />
          </button>
        </div>
        <span className="grow" />
        <div className="setting-choice cal-mode" role="radiogroup" aria-label="Calendar view">
          {MODES.map(([m, label, key]) => (
            <button key={m} role="radio" aria-checked={mode === m} className={mode === m ? "selected" : ""} onClick={() => setMode(m)}>
              {label}
              <Kbd>{key}</Kbd>
            </button>
          ))}
        </div>
        <button className="btn btn-ghost btn-sm btn-icon" title="Sync calendars" aria-label="Sync calendars" onClick={() => void api.calendarSyncNow()}>
          <Icon name="refresh" size="xs" />
        </button>
      </header>
      {status && !connected ? (
        <NotConnected />
      ) : mode === "agenda" ? (
        <Agenda events={events} from={from} now={now} sel={sel} status={status} register={register} onSelect={setSel} />
      ) : mode === "week" ? (
        <Week events={events} from={from} now={now} sel={sel} status={status} register={register} onSelect={setSel} />
      ) : (
        <Month
          events={events}
          from={from}
          now={now}
          sel={sel}
          status={status}
          register={register}
          onSelect={setSel}
          weeks={weeks}
          month={month}
          day={anchor}
          onDay={setAnchor}
          onOpenDay={openDay}
        />
      )}
    </section>
  );
}

const monthDay = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric" });
const monthDayYear = new Intl.DateTimeFormat(undefined, { month: "short", day: "numeric", year: "numeric" });
const monthTitle = new Intl.DateTimeFormat(undefined, { month: "long", year: "numeric" });

function weekTitle(from: number): string {
  const last = addDays(from, 6);
  return `${monthDay.format(from)} – ${new Date(from).getMonth() === new Date(last).getMonth() ? new Date(last).getDate() + ", " + new Date(last).getFullYear() : monthDayYear.format(last)}`;
}

function NotConnected() {
  return (
    <div className="cal-empty">
      <Icon name="calendar" />
      <h2>Connect Google Calendar</h2>
      <p className="faint">
        See your agenda next to your mail, invitation details in threads, and your last and next meeting with anyone. Penguin reads your calendar
        with read-only access, per account, and keeps it on this Mac.
      </p>
      <button className="btn btn-primary" onClick={() => openSettings("calendar")}>
        Set up in Settings
      </button>
    </div>
  );
}

interface ViewProps {
  events: CalendarEvent[] | null;
  from: number;
  now: number;
  sel: string | null;
  status: CalendarStatus | null;
  register: (k: string) => (el: HTMLElement | null) => void;
  onSelect: (k: string) => void;
}

/** Account tone class (account color), else the calendar's own color. */
function colorOf(e: CalendarEvent, status: CalendarStatus | null): { cls: string; style?: CSSProperties } {
  const acct = accountById(e.accountId);
  if (acct) return { cls: `t-${accountTone(acct.color)}` };
  const c = calendarColor(status, e.accountId, e.calendarId);
  return { cls: "", style: c ? ({ "--t11": c } as CSSProperties) : undefined };
}

function eventClass(e: CalendarEvent, now: number, active: boolean): string {
  let c = "";
  if (e.end <= now) c += " is-past";
  if (e.myResponse === "declined") c += " is-declined";
  if (e.myResponse === "needsAction" || e.myResponse === "tentative") c += " is-pending";
  if (e.free) c += " is-free";
  if (active) c += " is-active";
  return c;
}

// ---------------------------------------------------------------------------
// Agenda
// ---------------------------------------------------------------------------
function Agenda({ events, from, now, sel, status, register, onSelect }: ViewProps) {
  const groups = useMemo(() => (events ? groupByDay(events, from, AGENDA_DAYS) : null), [events, from]);
  const today = startOfDay(now);
  if (!groups) return <div className="cal-scroll v-scroll" />;
  if (!groups.length)
    return (
      <div className="cal-scroll v-scroll">
        <div className="cal-none faint">Nothing on your calendars in the next four weeks.</div>
      </div>
    );
  return (
    <div className="cal-scroll v-scroll">
      <div className="agenda">
        {groups.map((g) => {
          // Where the now-line goes in today's list: before the first event that hasn't ended.
          const nowAt = g.day === today ? g.events.findIndex((e) => !e.allDay && e.end > now) : -2;
          return (
            <section key={g.day} className={"ag-day" + (g.day === today ? " is-today" : "")}>
              <h2 className="ag-date">
                <span>{dayLabel(g.day, now)}</span>
                {g.day <= addDays(today, 1) && g.day >= today && <span className="faint">{monthDay.format(g.day)}</span>}
              </h2>
              <div className="ag-list">
                {g.events.map((e, i) => {
                  const k = evKey(e);
                  const { cls, style } = colorOf(e, status);
                  const cont = startOfDay(e.start) < g.day;
                  return (
                    <div key={k} className="ag-slot">
                      {i === nowAt && <NowLine now={now} />}
                      <button
                        ref={register(k)}
                        className={"ag-ev" + eventClass(e, now, sel === k)}
                        onClick={(ev) => {
                          onSelect(k);
                          openEvent(e, ev.currentTarget);
                        }}
                      >
                        <span className="ag-time tnum">{e.allDay ? (eventDays(e).length > 1 ? `Day ${eventDays(e).indexOf(g.day) + 1}/${eventDays(e).length}` : "All day") : cont ? "cont." : timeOf(e.start)}</span>
                        <i className={"cal-bar " + cls} style={style} />
                        <span className="ag-main min0">
                          <span className="ag-title truncate">{e.summary || "(no title)"}</span>
                          <span className="ag-sub faint truncate">
                            {[!e.allDay && timeRange(e), e.location, attendeeLine(e), responseNote(e)].filter(Boolean).join(" · ")}
                          </span>
                        </span>
                        {e.conferenceUrl && e.end > now && (
                          <span
                            role="button"
                            tabIndex={-1}
                            className="btn btn-secondary btn-sm ag-join"
                            onClick={(ev) => {
                              ev.stopPropagation();
                              void api.openExternal(e.conferenceUrl!);
                            }}
                          >
                            Join
                          </span>
                        )}
                      </button>
                    </div>
                  );
                })}
                {nowAt === -1 && <NowLine now={now} />}
              </div>
            </section>
          );
        })}
      </div>
    </div>
  );
}

function attendeeLine(e: CalendarEvent): string | null {
  const others = e.attendees.filter((a) => !a.self && !a.resource);
  if (!others.length) return null;
  const first = others[0].name?.split(" ")[0] ?? others[0].email;
  return others.length === 1 ? `with ${first}` : `with ${first} +${others.length - 1}`;
}

function responseNote(e: CalendarEvent): string | null {
  if (e.myResponse === "needsAction") return "not answered";
  if (e.myResponse === "tentative" || e.myResponse === "declined") return responseLabel(e.myResponse)?.toLowerCase() ?? null;
  return e.free ? "free" : null;
}

function NowLine({ now }: { now: number }) {
  return (
    <div className="cal-now" aria-label={`Now, ${timeOf(now)}`}>
      <span className="tnum">{timeOf(now)}</span>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Week
// ---------------------------------------------------------------------------
function Week({ events, from, now, sel, status, register, onSelect }: ViewProps) {
  const days = useMemo(() => Array.from({ length: 7 }, (_, i) => addDays(from, i)), [from]);
  const scroller = useRef<HTMLDivElement>(null);
  const today = startOfDay(now);
  const allDay = useMemo(() => (events ?? []).filter((e) => e.allDay).sort(byAllDayThenStart), [events]);
  const placed = useMemo(() => days.map((d) => layoutDay(events ?? [], d)), [days, events]);

  // Open at 7 AM, or an hour before now in this week.
  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const inWeek = now >= from && now < addDays(from, 7);
    const h = inWeek ? Math.max(0, new Date(now).getHours() - 1) : 7;
    el.scrollTop = Math.min(h, 7) * HOUR_PX;
    // eslint-disable-next-line react-hooks/exhaustive-deps -- once per week shown
  }, [from]);

  return (
    <div className="wk">
      <div className="wk-head">
        <div className="wk-gutter" />
        {days.map((d) => (
          <div key={d} className={"wk-dayname" + (d === today ? " is-today" : "")}>
            <span>{new Intl.DateTimeFormat(undefined, { weekday: "short" }).format(d)}</span>
            <b className="tnum">{new Date(d).getDate()}</b>
          </div>
        ))}
      </div>
      {allDay.length > 0 && (
        <div className="wk-allday">
          <div className="wk-gutter faint">all-day</div>
          <div className="wk-allday-grid">
            {allDay.map((e) => {
              const ds = eventDays(e).filter((d) => d >= from && d < addDays(from, 7));
              if (!ds.length) return null;
              const col = days.indexOf(ds[0]) + 1;
              const k = evKey(e);
              const { cls, style } = colorOf(e, status);
              return (
                <button
                  key={k}
                  ref={register(k)}
                  className={"wk-chip " + cls + eventClass(e, now, sel === k)}
                  style={{ ...style, gridColumn: `${col} / span ${ds.length}` }}
                  onClick={(ev) => {
                    onSelect(k);
                    openEvent(e, ev.currentTarget);
                  }}
                >
                  <span className="truncate">{e.summary || "(no title)"}</span>
                </button>
              );
            })}
          </div>
        </div>
      )}
      <div className="wk-scroll v-scroll" ref={scroller}>
        <div className="wk-body" style={{ height: 24 * HOUR_PX }}>
          <div className="wk-gutter wk-hours">
            {Array.from({ length: 23 }, (_, h) => (
              <span key={h} style={{ top: (h + 1) * HOUR_PX }} className="tnum">
                {timeOf(addDays(from, 0) + (h + 1) * HOUR).replace(":00", "")}
              </span>
            ))}
          </div>
          {days.map((d, i) => (
            <div key={d} className={"wk-col" + (d === today ? " is-today" : "")}>
              {placed[i].map((p) => {
                const e = p.event;
                const k = evKey(e);
                const { cls, style } = colorOf(e, status);
                const w = 100 / p.cols;
                const short = p.height < 40;
                return (
                  <button
                    key={k}
                    ref={register(k)}
                    className={"wk-ev " + cls + eventClass(e, now, sel === k) + (short ? " is-short" : "")}
                    style={{
                      ...style,
                      top: (p.top / 60) * HOUR_PX,
                      height: Math.max(18, (p.height / 60) * HOUR_PX - 2),
                      left: `calc(${p.col * w}% + 2px)`,
                      width: `calc(${w}% - 4px)`,
                    }}
                    title={`${e.summary}\n${timeRange(e)}${e.location ? "\n" + e.location : ""}`}
                    onClick={(ev) => {
                      onSelect(k);
                      openEvent(e, ev.currentTarget);
                    }}
                  >
                    <span className="wk-ev-title truncate">{e.summary || "(no title)"}</span>
                    {!short && <span className="wk-ev-time tnum truncate">{timeRange(e)}</span>}
                  </button>
                );
              })}
              {d === today && <div className="wk-now" style={{ top: ((now - d) / MIN / 60) * HOUR_PX }} />}
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}

// ---------------------------------------------------------------------------
// Month
// ---------------------------------------------------------------------------
const weekdayShort = new Intl.DateTimeFormat(undefined, { weekday: "short" });
const monthShort = new Intl.DateTimeFormat(undefined, { month: "short" });
const dayLong = new Intl.DateTimeFormat(undefined, { weekday: "long", month: "long", day: "numeric" });

interface MonthProps extends ViewProps {
  weeks: number[];
  /** The 1st of the month shown, and the selected day. */
  month: number;
  day: number;
  onDay: (day: number) => void;
  onOpenDay: (day: number) => void;
}

function Month({ events, now, sel, status, onSelect, weeks, month, day, onDay, onOpenDay }: MonthProps) {
  const grid = useRef<HTMLDivElement>(null);
  const [lanes, setLanes] = useState(4);
  const today = startOfDay(now);
  const nextMonth = addMonths(month, 1);
  const rows = useMemo(() => weeks.map((w) => layoutMonthWeek(events ?? [], w)), [weeks, events]);

  // As many event lines as a week row holds.
  useLayoutEffect(() => {
    const el = grid.current;
    if (!el) return;
    const fit = () => setLanes(Math.max(1, Math.floor((el.clientHeight / weeks.length - MO_HEAD_PX - 4) / MO_LANE_PX)));
    fit();
    const ro = new ResizeObserver(fit);
    ro.observe(el);
    return () => ro.disconnect();
  }, [weeks.length]);

  return (
    <div className="mo">
      <div className="mo-head">
        {Array.from({ length: 7 }, (_, i) => (
          <span key={i}>{weekdayShort.format(addDays(weeks[0], i))}</span>
        ))}
      </div>
      <div className="mo-grid" ref={grid} style={{ gridTemplateRows: `repeat(${weeks.length}, minmax(0, 1fr))` }}>
        {weeks.map((w, wi) => {
          const { shown, more } = fitMonthWeek(rows[wi], lanes);
          return (
            <div key={w} className="mo-week" style={{ gridTemplateRows: `${MO_HEAD_PX}px repeat(${lanes}, ${MO_LANE_PX}px) 1fr` }}>
              {Array.from({ length: 7 }, (_, c) => {
                const d = addDays(w, c);
                const out = d < month || d >= nextMonth;
                const first = new Date(d).getDate() === 1;
                return (
                  <div
                    key={d}
                    className={"mo-day" + (out ? " is-out" : "") + (d === today ? " is-today" : "") + (d === day ? " is-sel" : "")}
                    style={{ gridColumn: c + 1 }}
                    onClick={() => onDay(d)}
                    onDoubleClick={() => onOpenDay(d)}
                  >
                    <button
                      className="mo-num tnum"
                      title="Open in Week"
                      aria-label={`${dayLong.format(d)}, open in Week`}
                      aria-current={d === today ? "date" : undefined}
                      onClick={(ev) => {
                        ev.stopPropagation();
                        onOpenDay(d);
                      }}
                    >
                      {first && <span className="mo-mon">{monthShort.format(d)}</span>}
                      {new Date(d).getDate()}
                    </button>
                  </div>
                );
              })}
              {shown.map((s) => {
                const e = s.event;
                const k = evKey(e);
                const { cls, style } = colorOf(e, status);
                return (
                  <button
                    key={k}
                    className={
                      "mo-chip " +
                      cls +
                      eventClass(e, now, sel === k) +
                      (s.bar ? " is-bar" : "") +
                      (s.before ? " is-before" : "") +
                      (s.after ? " is-after" : "")
                    }
                    style={{ ...style, gridColumn: `${s.col + 1} / span ${s.span}`, gridRow: s.lane + 2 }}
                    title={`${e.summary || "(no title)"}\n${timeRange(e)}${e.location ? "\n" + e.location : ""}`}
                    onClick={(ev) => {
                      onSelect(k);
                      openEvent(e, ev.currentTarget);
                    }}
                  >
                    {s.bar ? (
                      <span className="truncate">
                        {!e.allDay && !s.before && <span className="mo-time tnum">{shortTime(e.start)} </span>}
                        {e.summary || "(no title)"}
                      </span>
                    ) : (
                      <>
                        <i className="dot" />
                        <span className="mo-time tnum">{shortTime(e.start)}</span>
                        <span className="mo-title truncate">{e.summary || "(no title)"}</span>
                      </>
                    )}
                  </button>
                );
              })}
              {more.map(
                (n, c) =>
                  n > 0 && (
                    <button
                      key={`more-${c}`}
                      className="mo-more"
                      style={{ gridColumn: c + 1, gridRow: lanes + 1 }}
                      title="Open the day in Week"
                      onClick={() => onOpenDay(addDays(w, c))}
                    >
                      +{n} more
                    </button>
                  ),
              )}
            </div>
          );
        })}
      </div>
    </div>
  );
}

/** "9:30 AM", "3 PM": the hour alone when on the hour, as the week gutter. */
function shortTime(ms: number): string {
  return timeOf(ms).replace(":00", "");
}
