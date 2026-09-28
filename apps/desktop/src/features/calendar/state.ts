// Calendar data for the UI: a small range cache over list_events, the
// connection status, and the open event popover. Everything invalidates on
// penguin://calendar-changed. OWNER: calendar agent.
import { useEffect, useState, useSyncExternalStore } from "react";
import { api, onCalendarChanged } from "../../lib/api";
import type { CalendarEvent, CalendarStatus } from "../../lib/types";
import { getUi, setUi, useUi } from "../../lib/ui";
import { profileScope } from "../../app/store";

// ---------------------------------------------------------------------------
// Change signal
// ---------------------------------------------------------------------------
let version = 0;
const subs = new Set<() => void>();
let listening = false;

function ensureListening() {
  if (listening) return;
  listening = true;
  void onCalendarChanged(() => invalidateCalendar());
}

export function invalidateCalendar() {
  version++;
  rangeCache.clear();
  statusCache = null;
  subs.forEach((f) => f());
}

/** Re-renders when calendar data changes; returns a version number. */
export function useCalendarVersion(): number {
  ensureListening();
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => version,
  );
}

// ---------------------------------------------------------------------------
// Scope: the account filter, else the active profile, else everything
// ---------------------------------------------------------------------------
export function calendarScope(): string[] | null {
  const ui = getUi();
  return ui.accountFilter ? [ui.accountFilter] : profileScope(ui);
}

export function useCalendarScope(): string[] | null {
  const filter = useUi((s) => s.accountFilter);
  const profile = useUi((s) => s.profileId);
  const [scope, setScope] = useState(calendarScope);
  useEffect(() => setScope(calendarScope()), [filter, profile]);
  return scope;
}

// ---------------------------------------------------------------------------
// Events by range
// ---------------------------------------------------------------------------
const rangeCache = new Map<string, Promise<CalendarEvent[]>>();
const RANGE_MAX = 24;

function loadRange(from: number, to: number, scope: string[] | null): Promise<CalendarEvent[]> {
  const key = `${from}:${to}:${scope?.join(",") ?? "*"}`;
  let p = rangeCache.get(key);
  if (!p) {
    p = api.listEvents(from, to, scope);
    // A failure isn't cached: the next render asks again.
    p.catch(() => rangeCache.delete(key));
    rangeCache.set(key, p);
    while (rangeCache.size > RANGE_MAX) rangeCache.delete(rangeCache.keys().next().value!);
  }
  return p;
}

/** Events overlapping [from, to) in the current scope; null while loading. */
export function useEvents(from: number, to: number, scope: string[] | null = null): { events: CalendarEvent[] | null; error: string | null } {
  const v = useCalendarVersion();
  const [state, setState] = useState<{ key: string; events: CalendarEvent[] | null; error: string | null }>({ key: "", events: null, error: null });
  const key = `${from}:${to}:${scope?.join(",") ?? "*"}:${v}`;
  useEffect(() => {
    let live = true;
    loadRange(from, to, scope).then(
      (events) => live && setState({ key, events, error: null }),
      (e) => live && setState({ key, events: [], error: String(e?.message ?? e) }),
    );
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps -- key covers from/to/scope/version
  }, [key]);
  // Keep showing the previous range while the next one loads (no flash).
  return { events: state.events, error: state.error };
}

// ---------------------------------------------------------------------------
// Connection status
// ---------------------------------------------------------------------------
let statusCache: Promise<CalendarStatus> | null = null;

export function loadCalendarStatus(force = false): Promise<CalendarStatus> {
  if (force || !statusCache) {
    statusCache = api.calendarStatus();
    statusCache.catch(() => (statusCache = null));
  }
  return statusCache;
}

/** Status of every account; null until loaded. */
export function useCalendarStatus(): CalendarStatus | null {
  const v = useCalendarVersion();
  const [status, setStatus] = useState<CalendarStatus | null>(null);
  useEffect(() => {
    let live = true;
    loadCalendarStatus().then(
      (s) => live && setStatus(s),
      () => live && setStatus(null),
    );
    return () => {
      live = false;
    };
  }, [v]);
  return status;
}

export function anyConnected(s: CalendarStatus | null): boolean {
  return !!s?.accounts.some((a) => a.granted);
}

/** Calendar color by account + calendar id, for event bars. */
export function calendarColor(s: CalendarStatus | null, accountId: string, calendarId: string): string | null {
  const a = s?.accounts.find((x) => x.accountId === accountId);
  return a?.calendars.find((c) => c.id === calendarId)?.color ?? null;
}

// ---------------------------------------------------------------------------
// Open event (popover)
// ---------------------------------------------------------------------------
export interface OpenEvent {
  event: CalendarEvent;
  rect: DOMRect | null;
  /** The element it was opened from: pressing it again closes the popover. */
  anchor: HTMLElement | null;
  returnFocus: HTMLElement | null;
}

let open: OpenEvent | null = null;
const openSubs = new Set<() => void>();

export function openEvent(event: CalendarEvent, anchor?: HTMLElement | DOMRect | null) {
  const el = anchor instanceof HTMLElement ? anchor : null;
  if (el && open?.anchor === el) return closeEvent();
  const rect = anchor instanceof HTMLElement ? anchor.getBoundingClientRect() : (anchor ?? null);
  open = { event, rect, anchor: el, returnFocus: el ?? (document.activeElement as HTMLElement | null) };
  openSubs.forEach((f) => f());
}

export function closeEvent() {
  if (!open) return;
  const back = open.returnFocus;
  open = null;
  openSubs.forEach((f) => f());
  back?.focus?.({ preventScroll: true });
}

export function isEventOpen(): boolean {
  return open !== null;
}

export function useOpenEvent(): OpenEvent | null {
  return useSyncExternalStore(
    (cb) => {
      openSubs.add(cb);
      return () => openSubs.delete(cb);
    },
    () => open,
  );
}

// ---------------------------------------------------------------------------
// "Open in Calendar": show the Calendar surface at an event's day with the
// event selected (the invite card's button). CalendarView takes it on mount.
// ---------------------------------------------------------------------------
export interface CalendarFocus {
  /** Unix ms inside the day to show. */
  at: number;
  /** `accountId/calendarId/id` of the event to select, when it is on a calendar. */
  key: string | null;
}

let focus: CalendarFocus | null = null;

export function showInCalendar(event: CalendarEvent) {
  focus = { at: event.start, key: event.calendarId && event.id ? `${event.accountId}/${event.calendarId}/${event.id}` : null };
  setUi({ surface: "calendar", threadOpen: false, overlay: null });
}

/** The pending focus, once. */
export function takeCalendarFocus(): CalendarFocus | null {
  const f = focus;
  focus = null;
  return f;
}
