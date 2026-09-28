// Invitation helpers for the row chip (InviteChip.tsx) and the thread card
// (InviteCard.tsx): what to show for an invite, its answer state, "What
// changed" lines, the organizer's time, and the propose-new-time picker's
// values. Pure functions, no DOM or state, so tests/invites.test.ts runs
// them under node. OWNER: calendar agent.
import type { EventAttendee, EventResponse, InviteAnswer, InviteCard, InviteChip } from "../../lib/types";
import { DAY, MIN, startOfDay } from "./time.ts";

export const ANSWERS: InviteAnswer[] = ["accepted", "tentative", "declined"];

/** The button words: Yes / Maybe / No. */
export function answerWord(r: InviteAnswer): string {
  return r === "accepted" ? "Yes" : r === "tentative" ? "Maybe" : "No";
}

/** The state after answering: "Going", "Maybe", "Not going". */
export function responseLabel(r: EventResponse | null | undefined): string | null {
  switch (r) {
    case "accepted":
      return "Going";
    case "tentative":
      return "Maybe";
    case "declined":
      return "Not going";
    default:
      return null;
  }
}

export function isAnswer(r: EventResponse | null | undefined): r is InviteAnswer {
  return r === "accepted" || r === "tentative" || r === "declined";
}

/** An answer clicked but not reflected in the data yet (optimistic). */
export interface PendingAnswer {
  response: InviteAnswer;
  /** The response shown when it was clicked; the override holds until fresh data differs. */
  baseline: EventResponse | null;
  /** Still in flight. */
  pending: boolean;
}

/** What to show: the click in flight, else the click until fresh data arrives, else the data. */
export function shownResponse(current: EventResponse | null, o: PendingAnswer | null | undefined): EventResponse | null {
  if (!o) return current;
  return o.pending || current === o.baseline ? o.response : current;
}

// ---------------------------------------------------------------------------
// Compact dates: "Tue Sep 29 · 10–11am"
// ---------------------------------------------------------------------------
const dayFmt = new Intl.DateTimeFormat("en-US", { weekday: "short", month: "short", day: "numeric" });
const monthDayFmt = new Intl.DateTimeFormat("en-US", { month: "short", day: "numeric" });

/** "Tue Sep 29" (no comma, as in list rows). */
export function shortDay(ms: number): string {
  return dayFmt.format(ms).replace(",", "");
}

function clock(ms: number): { h: number; m: number; pm: boolean } {
  const d = new Date(ms);
  const h24 = d.getHours();
  return { h: h24 % 12 || 12, m: d.getMinutes(), pm: h24 >= 12 };
}

function hm(c: { h: number; m: number }): string {
  return c.m ? `${c.h}:${String(c.m).padStart(2, "0")}` : String(c.h);
}

/** "10–11am", "10:30–11am", "11:30am–12:30pm", "9am–5pm". */
export function compactRange(start: number, end: number): string {
  const a = clock(start);
  const b = clock(end);
  const suf = (c: { pm: boolean }) => (c.pm ? "pm" : "am");
  return a.pm === b.pm ? `${hm(a)}–${hm(b)}${suf(b)}` : `${hm(a)}${suf(a)}–${hm(b)}${suf(b)}`;
}

/** The chip's when: "Tue Sep 29 · 10–11am", "Tue Sep 29 · All day", "Sep 29 – Oct 1". */
export function chipWhen(e: { start: number; end: number; allDay: boolean }): string {
  if (e.allDay) {
    const last = e.end - DAY;
    if (startOfDay(last) > startOfDay(e.start)) return `${monthDayFmt.format(e.start)} – ${monthDayFmt.format(last)}`;
    return `${shortDay(e.start)} · All day`;
  }
  if (startOfDay(e.start) !== startOfDay(Math.max(e.start, e.end - 1))) {
    return `${shortDay(e.start)} ${hm(clock(e.start))}${clock(e.start).pm ? "pm" : "am"} – ${shortDay(e.end)}`;
  }
  return `${shortDay(e.start)} · ${compactRange(e.start, e.end)}`;
}

// ---------------------------------------------------------------------------
// The row chip
// ---------------------------------------------------------------------------
export interface ChipView {
  when: string;
  /** Updated / Canceled tag. */
  tag: "Updated" | "Canceled" | null;
  /** Yes / Maybe / No buttons. */
  buttons: boolean;
  /** The answer state ("✓ Going"), when answered. */
  state: string | null;
  response: EventResponse | null;
  /** In flight. */
  pending: boolean;
  /** "Conflicts with Standup" (+n). */
  conflict: string | null;
  /** A reply to your event: "Priya accepted". */
  reply: string | null;
  /** Past or cancelled: quieter. */
  muted: boolean;
}

function who(a: EventAttendee): string {
  const n = a.name?.trim();
  return n ? n.split(/\s+/)[0] : a.email;
}

export function replyText(chip: Pick<InviteChip, "method" | "replier">): string | null {
  if (chip.method !== "reply" && chip.method !== "counter") return null;
  const a = chip.replier;
  if (!a) return chip.method === "counter" ? "New time proposed" : "Response";
  if (chip.method === "counter") return `${who(a)} proposed a new time`;
  switch (a.response) {
    case "accepted":
      return `${who(a)} accepted`;
    case "declined":
      return `${who(a)} declined`;
    case "tentative":
      return `${who(a)} said maybe`;
    default:
      return `${who(a)} replied`;
  }
}

export function chipView(chip: InviteChip, o: PendingAnswer | null | undefined, now: number): ChipView {
  const cancelled = chip.method === "cancel";
  const over = chip.end <= now && !chip.recurring;
  const response = shownResponse(chip.response, o);
  const answered = isAnswer(response);
  const live = chip.canRespond && !cancelled && !over;
  return {
    when: chipWhen(chip),
    tag: cancelled ? "Canceled" : chip.updated && chip.method === "request" ? "Updated" : null,
    buttons: live && !answered,
    state: !cancelled && answered ? responseLabel(response) : null,
    response,
    pending: !!o?.pending,
    conflict:
      live && response !== "declined" && chip.conflict
        ? `Conflicts with ${chip.conflict}${chip.conflicts > 1 ? ` +${chip.conflicts - 1}` : ""}`
        : null,
    reply: replyText(chip),
    muted: cancelled || over,
  };
}

// ---------------------------------------------------------------------------
// The card
// ---------------------------------------------------------------------------
export interface ChangeLine {
  field: "time" | "location" | "title" | "guests";
  label: string;
  before: string;
  after: string;
}

const fullWhenFmt = new Intl.DateTimeFormat("en-US", { weekday: "short", month: "short", day: "numeric", hour: "numeric", minute: "2-digit" });

function snapWhen(s: { start: number; end: number; allDay: boolean }): string {
  return chipWhen(s);
}

/** "What changed": one line per changed field, old → new. */
export function changeLines(card: Pick<InviteCard, "changes" | "previous" | "event">): ChangeLine[] {
  const prev = card.previous;
  if (!prev) return [];
  const e = card.event;
  const out: ChangeLine[] = [];
  for (const f of card.changes) {
    if (f === "time") out.push({ field: f, label: "Time", before: snapWhen(prev), after: snapWhen(e) });
    else if (f === "location") out.push({ field: f, label: "Location", before: prev.location || "none", after: e.location || "none" });
    else if (f === "title") out.push({ field: f, label: "Title", before: prev.summary || "(no title)", after: e.summary || "(no title)" });
    else if (f === "guests") {
      const was = new Set(prev.attendees.filter((a) => !a.resource).map((a) => a.email));
      const now = new Set(e.attendees.filter((a) => !a.resource).map((a) => a.email));
      const added = [...now].filter((x) => !was.has(x)).length;
      const removed = [...was].filter((x) => !now.has(x)).length;
      const parts = [added ? `${added} added` : "", removed ? `${removed} removed` : ""].filter(Boolean);
      out.push({ field: f, label: "Guests", before: `${was.size}`, after: parts.join(", ") || `${now.size}` });
    }
  }
  return out;
}

/**
 * The time in the organizer's zone when it differs from yours:
 * "7:00 AM in Los Angeles". Null for all-day events, the same zone, or a
 * zone name the browser doesn't know (e.g. Outlook's Windows names).
 */
export function organizerTime(start: number, allDay: boolean, zone: string | null, localZone: string): string | null {
  if (!zone || allDay || zone === localZone) return null;
  let fmt: Intl.DateTimeFormat;
  try {
    fmt = new Intl.DateTimeFormat("en-US", { timeZone: zone, weekday: "short", hour: "numeric", minute: "2-digit" });
  } catch {
    // Not an IANA zone name: nothing to convert with.
    return null;
  }
  const local = new Intl.DateTimeFormat("en-US", { timeZone: localZone, weekday: "short", hour: "numeric", minute: "2-digit" });
  const theirs = fmt.format(start);
  if (theirs === local.format(start)) return null;
  const city = zone.split("/").pop()!.replace(/_/g, " ");
  return `${theirs} in ${city}`;
}

/** The attendees to list under "Guests" (people, organizer first, you last). */
export function guestList(attendees: EventAttendee[]): EventAttendee[] {
  return attendees
    .filter((a) => !a.resource)
    .slice()
    .sort((a, b) => Number(b.organizer) - Number(a.organizer) || Number(a.self) - Number(b.self));
}

/** "3 yes, 1 maybe, 2 awaiting". */
export function guestSummary(attendees: EventAttendee[]): string {
  const people = attendees.filter((a) => !a.resource);
  const n = (r: EventResponse) => people.filter((a) => a.response === r).length;
  return [
    n("accepted") && `${n("accepted")} yes`,
    n("tentative") && `${n("tentative")} maybe`,
    n("declined") && `${n("declined")} no`,
    n("needsAction") && `${n("needsAction")} awaiting`,
  ]
    .filter(Boolean)
    .join(", ");
}

// ---------------------------------------------------------------------------
// Propose new time
// ---------------------------------------------------------------------------
export interface ProposalFields {
  /** YYYY-MM-DD */
  date: string;
  /** HH:MM (24 h) */
  start: string;
  end: string;
}

function pad(n: number): string {
  return String(n).padStart(2, "0");
}

function fields(start: number, end: number): ProposalFields {
  const s = new Date(start);
  const e = new Date(end);
  return {
    date: `${s.getFullYear()}-${pad(s.getMonth() + 1)}-${pad(s.getDate())}`,
    start: `${pad(s.getHours())}:${pad(s.getMinutes())}`,
    end: `${pad(e.getHours())}:${pad(e.getMinutes())}`,
  };
}

/** Where the picker starts: the same time the next weekday, same length (30 min for all-day). */
export function defaultProposal(e: { start: number; end: number; allDay: boolean }): ProposalFields {
  const len = e.allDay ? 30 * MIN : Math.max(15 * MIN, Math.min(e.end - e.start, 8 * 60 * MIN));
  const base = new Date(e.allDay ? e.start + 10 * 60 * MIN : e.start);
  base.setDate(base.getDate() + 1);
  while (base.getDay() === 0 || base.getDay() === 6) base.setDate(base.getDate() + 1);
  return fields(base.getTime(), base.getTime() + len);
}

/** Picker values → unix ms, or an error to show. End at or before start = the next day is not guessed. */
export function parseProposal(f: ProposalFields, now: number): { start: number; end: number } | { error: string } {
  const d = /^(\d{4})-(\d{2})-(\d{2})$/.exec(f.date);
  const s = /^(\d{1,2}):(\d{2})$/.exec(f.start);
  const e = /^(\d{1,2}):(\d{2})$/.exec(f.end);
  if (!d || !s || !e) return { error: "Pick a day, a start and an end" };
  const at = (t: RegExpExecArray) => new Date(+d[1], +d[2] - 1, +d[3], +t[1], +t[2]).getTime();
  const start = at(s);
  const end = at(e);
  if (end <= start) return { error: "The end must be after the start" };
  if (start < now) return { error: "Pick a time in the future" };
  return { start, end };
}

/** "Wed Sep 30, 2:00 PM – 3:00 PM" for the proposal line. */
export function proposalLabel(start: number, end: number): string {
  return `${fullWhenFmt.format(start)} – ${new Intl.DateTimeFormat("en-US", { hour: "numeric", minute: "2-digit" }).format(end)}`;
}

/** The one-line explanation of how an answer is sent. */
export function routeHint(card: Pick<InviteCard, "route" | "rsvpAvailable">, organizer: string | null): string | null {
  const to = organizer ?? "the organizer";
  switch (card.route) {
    case "calendar":
      return null;
    case "graph":
      return `Outlook tells ${to}; if it can't, Penguin emails your answer.`;
    case "email":
      return card.rsvpAvailable
        ? `Penguin emails your answer to ${to}. Turn on RSVP to answer through Google Calendar.`
        : `Penguin emails your answer to ${to}.`;
    default:
      return null;
  }
}

/** Snapshots and chips both carry these for "is this the same event". */
export function inviteKey(accountId: string, x: { uid: string | null; messageId?: string }): string {
  return `${accountId}|${x.uid ?? x.messageId ?? ""}`;
}

