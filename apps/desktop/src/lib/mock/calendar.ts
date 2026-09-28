// Mock Google Calendar (calendar agent) for `npm run dev:mock`: two connected
// accounts (Northwind without RSVP, Harbor Labs with it; Personal not
// connected), events over ±6 weeks around now (a full month grid) with the
// mock mail's people, and an invite card for the "Invitation: Weekly sync"
// thread. Fictional people and .example domains only.
import type {
  Address,
  CalendarAccountStatus,
  CalendarEvent,
  CalendarInfo,
  CalendarStatus,
  EventAttendee,
  EventDetail,
  InviteCard,
  PersonMeetings,
  SearchResponse,
  Settings,
} from "../types";
import { mockBackend, type MockHandler } from "./index";
import { mailHandlers } from "./mail";

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;
const NOW = Date.now();

const NW = "acc-northwind";
const HL = "acc-harbor";
const PE = "acc-personal";
const EMAIL: Record<string, string> = {
  [NW]: "sam@northwind.example",
  [HL]: "sam@harbor-labs.example",
  [PE]: "sam.okafor@gmail.example",
};

const P = (name: string, email: string): Address => ({ name, email });
const people = {
  priya: P("Priya Natarajan", "priya@linden.example"),
  marco: P("Marco Bellini", "marco@northwind.example"),
  dana: P("Dana Whitfield", "dana@northwind.example"),
  ravi: P("Ravi Menon", "ravi@northwind.example"),
  kofi: P("Kofi Mensah", "kofi@northwind.example"),
  theo: P("Theo Laurent", "theo@harborlabs.example"),
  ana: P("Ana Sousa", "ana@harborlabs.example"),
  grace: P("Grace Kim", "grace.kim@alderpoint.example"),
  jonas: P("Jonas Weber", "jonas@tidewater.example"),
};

/** Local midnight today + `days`, at h:m. */
function at(days: number, h: number, m = 0): number {
  const d = new Date(NOW);
  d.setHours(0, 0, 0, 0);
  d.setDate(d.getDate() + days);
  d.setHours(h, m, 0, 0);
  return d.getTime();
}
function ymd(ms: number): string {
  const d = new Date(ms);
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
}

// ---------------------------------------------------------------------------
// Calendars
// ---------------------------------------------------------------------------
const calendars: CalendarInfo[] = [
  { accountId: NW, id: EMAIL[NW], summary: "Sam Okafor", color: "#7986cb", selected: true, primary: true, accessRole: "owner" },
  { accountId: NW, id: "team@northwind.example", summary: "Team Northwind", color: "#33b679", selected: true, primary: false, accessRole: "writer" },
  { accountId: NW, id: "en.usa#holiday@group.v.calendar.google.com", summary: "Holidays in United States", color: "#0b8043", selected: false, primary: false, accessRole: "reader" },
  { accountId: HL, id: EMAIL[HL], summary: "Sam Okafor", color: "#f4511e", selected: true, primary: true, accessRole: "owner" },
  { accountId: HL, id: "launch@harborlabs.example", summary: "Launch plan", color: "#8e24aa", selected: true, primary: false, accessRole: "reader" },
];

const grants: Record<string, { granted: boolean; rsvp: boolean; syncedAt: number | null; error: string | null }> = {
  [NW]: { granted: true, rsvp: false, syncedAt: NOW - 3 * MIN, error: null },
  [HL]: { granted: true, rsvp: true, syncedAt: NOW - 3 * MIN, error: null },
  [PE]: { granted: false, rsvp: false, syncedAt: null, error: null },
};

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------
let seq = 0;
function guest(a: Address, response: EventAttendee["response"] = "accepted", extra: Partial<EventAttendee> = {}): EventAttendee {
  return { email: a.email, name: a.name, response, organizer: false, self: false, optional: false, resource: false, ...extra };
}
function me(accountId: string, response: EventAttendee["response"] = "accepted", organizer = false): EventAttendee {
  return { email: EMAIL[accountId], name: null, response, organizer, self: true, optional: false, resource: false };
}

interface Spec {
  account: string;
  calendar?: string;
  summary: string;
  start: number;
  mins?: number;
  allDayDays?: number;
  organizer?: Address;
  guests?: EventAttendee[];
  myResponse?: EventAttendee["response"] | null;
  location?: string;
  description?: string;
  conference?: [string, CalendarEvent["conferenceKind"]];
  uid?: string;
  recurring?: string;
  free?: boolean;
}

function ev(s: Spec): CalendarEvent {
  const id = `ev${++seq}`;
  const allDay = !!s.allDayDays;
  const start = allDay ? at(Math.round((s.start - at(0, 0)) / DAY), 0) : s.start;
  const end = allDay ? at(Math.round((s.start - at(0, 0)) / DAY) + s.allDayDays!, 0) : start + (s.mins ?? 30) * MIN;
  const myResponse = s.myResponse === undefined ? (s.organizer ? "accepted" : null) : s.myResponse;
  const attendees: EventAttendee[] = s.guests
    ? [...(s.organizer ? [guest(s.organizer, "accepted", { organizer: true })] : [me(s.account, "accepted", true)]), ...s.guests]
    : [];
  if (s.guests && s.organizer) attendees.push(me(s.account, myResponse ?? "needsAction"));
  return {
    accountId: s.account,
    calendarId: s.calendar ?? EMAIL[s.account],
    id: s.recurring ? `${s.recurring}_${ymd(start).replace(/-/g, "")}` : id,
    icalUid: s.uid ?? (s.recurring ? `${s.recurring}@google.com` : `${id}@google.com`),
    status: "confirmed",
    summary: s.summary,
    description: s.description ?? "",
    location: s.location ?? "",
    start,
    end,
    allDay,
    startDate: allDay ? ymd(start) : null,
    endDate: allDay ? ymd(end) : null,
    organizer: s.organizer ?? (s.guests ? { name: "Sam Okafor", email: EMAIL[s.account] } : null),
    attendees,
    myResponse: s.guests ? (s.organizer ? myResponse : "accepted") : null,
    htmlLink: `https://calendar.google.com/calendar/event?eid=${id}`,
    conferenceUrl: s.conference?.[0] ?? null,
    conferenceKind: s.conference?.[1] ?? null,
    recurringEventId: s.recurring ?? null,
    free: !!s.free,
    updated: NOW - DAY,
  };
}

/** Recurring series run ±SPAN days: enough to fill any month grid (≤ 6 weeks) around today. */
const SPAN = 45;

/** Round now up to the next 5 minutes, then +25 min: "Next up … in 25 min". */
const soon = Math.ceil((NOW + 25 * MIN) / (5 * MIN)) * 5 * MIN;

function build(): CalendarEvent[] {
  const out: CalendarEvent[] = [];
  const weekday = (d: number) => [1, 2, 3, 4, 5].includes(new Date(at(d, 12)).getDay());
  // Northwind: a daily standup with Dana, Ravi and Kofi (Meet).
  for (let d = -SPAN; d <= SPAN; d++) {
    if (!weekday(d)) continue;
    out.push(
      ev({
        account: NW,
        calendar: "team@northwind.example",
        summary: "Standup",
        start: at(d, 9, 30),
        mins: 15,
        organizer: people.dana,
        guests: [guest(people.ravi), guest(people.kofi)],
        myResponse: "accepted",
        conference: ["https://meet.google.com/nwd-stnd-upp", "meet"],
        recurring: "standup",
      }),
    );
  }
  // Harbor Labs: Thursday "Weekly sync" with Theo and Ana (the invite thread).
  for (let d = -SPAN; d <= SPAN; d++) {
    if (new Date(at(d, 12)).getDay() !== 4) continue;
    out.push(
      ev({
        account: HL,
        summary: "Weekly sync",
        start: at(d, 10),
        mins: 30,
        organizer: people.theo,
        guests: [guest(people.ana)],
        myResponse: d < 0 ? "accepted" : "needsAction",
        description: "Standing agenda:\n1. Metrics (signups, retention)\n2. Launch blockers\n3. Hiring\n\nNotes doc: https://docs.harborlabs.example/weekly-sync",
        conference: ["https://meet.google.com/hlb-wkly-snc", "meet"],
        recurring: "weekly-sync",
      }),
    );
  }
  // Priya: a past review and an upcoming one (Zoom link in the location).
  out.push(
    ev({
      account: NW,
      summary: "Brand direction review",
      start: at(-5, 14),
      mins: 60,
      organizer: people.priya,
      guests: [guest(people.marco)],
      myResponse: "accepted",
      location: "https://linden.zoom.example/j/88213",
      description: "Round two directions. Deck: https://linden.example/decks/round-2",
    }),
    ev({
      account: NW,
      summary: "Q4 brand review",
      start: at(4, 15),
      mins: 60,
      organizer: people.priya,
      guests: [guest(people.marco), guest(people.dana, "tentative")],
      myResponse: "accepted",
      location: "Linden & Co, 214 Harbor St, Portland",
      conference: ["https://linden.zoom.example/j/99471", "zoom"],
      description: "Final review of the v7 deck before the stakeholder meeting.",
    }),
  );
  // Today: next up in ~25 min (with a conflicting event), a free lunch.
  out.push(
    ev({
      account: NW,
      summary: "Design crit: onboarding flow",
      start: soon,
      mins: 45,
      organizer: people.ravi,
      guests: [guest(people.dana), guest(people.kofi, "declined")],
      myResponse: "accepted",
      conference: ["https://meet.google.com/nwd-crit-onb", "meet"],
      description: "Step three: defer team invite and calendar connection. Notes: https://docs.northwind.example/crit",
    }),
    ev({
      account: HL,
      summary: "Pricing copy pass",
      start: soon + 15 * MIN,
      mins: 45,
      guests: [guest(people.ana)],
      location: "Harbor Labs, Room 2",
    }),
    ev({ account: NW, summary: "Lunch", start: at(0, 12, 30), mins: 60, free: true }),
  );
  // Declined vendor demo tomorrow; focus block; a client call.
  out.push(
    ev({
      account: HL,
      summary: "Vendor demo: Ledgerly",
      start: at(1, 11),
      mins: 45,
      organizer: P("Ledgerly Sales", "sales@ledgerly.example"),
      guests: [guest(people.theo, "tentative")],
      myResponse: "declined",
    }),
    ev({ account: NW, summary: "Focus: pricing page", start: at(1, 14), mins: 120 }),
    ev({
      account: NW,
      summary: "Alderpoint kickoff",
      start: at(2, 10),
      mins: 60,
      organizer: people.grace,
      guests: [guest(people.marco)],
      myResponse: "accepted",
      conference: ["https://teams.microsoft.example/l/meetup-join/19%3a", "teams"],
    }),
    ev({
      account: HL,
      summary: "Seed round: Tidewater intro",
      start: at(-9, 16),
      mins: 30,
      organizer: people.jonas,
      guests: [],
      myResponse: "accepted",
      location: "Tidewater Ventures, 90 Pier Ave",
    }),
    ev({ account: NW, summary: "Northwind offsite", start: at(9, 0), allDayDays: 2, location: "Cedar Lodge, Hood River" }),
    ev({ account: HL, calendar: "launch@harborlabs.example", summary: "Public beta launch", start: at(12, 0), allDayDays: 1 }),
    ev({ account: NW, summary: "Dentist", start: at(-2, 8), mins: 45, location: "418 Alder St, Suite 2" }),
  );
  // The rest of the month view: 1:1s and a design review, trips that run
  // over a weekend, a red-eye across midnight, and one packed day.
  for (let d = -SPAN; d <= SPAN; d++) {
    const dow = new Date(at(d, 12)).getDay();
    if (dow === 2 && Math.floor((at(d, 12) - at(0, 12)) / (7 * DAY)) % 2 === 0)
      out.push(
        ev({
          account: NW,
          summary: "1:1 Marco / Sam",
          start: at(d, 11),
          mins: 30,
          organizer: people.marco,
          guests: [],
          myResponse: "accepted",
          conference: ["https://meet.google.com/nwd-one-one", "meet"],
          recurring: "one-on-one",
        }),
      );
    if (dow === 1)
      out.push(
        ev({
          account: HL,
          summary: "Design review",
          start: at(d, 14),
          mins: 45,
          organizer: people.ana,
          guests: [guest(people.theo)],
          myResponse: "accepted",
          location: "Harbor Labs, Room 1",
          recurring: "design-review",
        }),
      );
  }
  const weekdayFrom = (d: number) => {
    while (!weekday(d)) d++;
    return d;
  };
  const packed = weekdayFrom(3);
  out.push(
    ev({ account: NW, summary: "Out of office", start: at(-17, 0), allDayDays: 5, free: true }),
    ev({ account: HL, summary: "Ana's birthday", start: at(-8, 0), allDayDays: 1 }),
    ev({ account: NW, summary: "Flight PDX → BOS", start: at(16, 22, 15), mins: 7 * 60 + 40, location: "PDX" }),
    ev({ account: NW, summary: "DesignOps Summit", start: at(17, 9), mins: 2 * 24 * 60 + 8 * 60, location: "Seaport Hall, Boston" }),
    ev({ account: NW, summary: "Coffee with Grace", start: at(packed, 8), mins: 30, organizer: people.grace, guests: [], myResponse: "accepted", location: "Driftwood Coffee, SE Division" }),
    ev({
      account: NW,
      summary: "Interview: frontend candidate",
      start: at(packed, 11),
      mins: 60,
      organizer: people.kofi,
      guests: [guest(people.ravi)],
      myResponse: "needsAction",
      conference: ["https://meet.google.com/nwd-intv-fe1", "meet"],
    }),
    ev({ account: HL, summary: "Board prep", start: at(packed, 16), mins: 60, guests: [guest(people.theo), guest(people.ana)] }),
    ev({ account: HL, summary: "Dinner: Tidewater", start: at(packed, 19), mins: 90, organizer: people.jonas, guests: [], myResponse: "tentative", location: "Larch & Pine" }),
  );
  // Next Thursday's weekly sync collides with a Northwind client call.
  const nextThu = out.find((e) => e.recurringEventId === "weekly-sync" && e.start > NOW);
  if (nextThu)
    out.push(
      ev({ account: NW, summary: "Client call: Alderpoint", start: nextThu.start - 15 * MIN, mins: 45, organizer: people.grace, guests: [], myResponse: "accepted" }),
    );
  return out;
}

const events = build();

function selectedCal(e: CalendarEvent): boolean {
  const c = calendars.find((x) => x.accountId === e.accountId && x.id === e.calendarId);
  return !!c?.selected && !!grants[e.accountId]?.granted;
}

function visible(): CalendarEvent[] {
  return events.filter(selectedCal);
}

function status(): CalendarStatus {
  const accounts: CalendarAccountStatus[] = [NW, HL, PE].map((id) => {
    const g = grants[id];
    return {
      accountId: id,
      granted: g.granted,
      rsvpGranted: g.rsvp,
      syncing: false,
      syncedAt: g.granted ? g.syncedAt : null,
      events: g.granted ? events.filter((e) => e.accountId === id && selectedCal(e)).length : 0,
      error: g.error,
      calendars: g.granted ? calendars.filter((c) => c.accountId === id) : [],
    };
  });
  return { accounts };
}

function changed(accountIds: string[]) {
  setTimeout(() => mockBackend.emit("penguin://calendar-changed", { accountIds }), 20);
}

/** For ./invites.ts: put invite fixtures on the calendar, find clashes, read grants. */
export const mockCalendar = {
  add(e: CalendarEvent) {
    events.push(e);
  },
  byUid(accountId: string, uid: string): CalendarEvent | null {
    return visible().find((e) => e.accountId === accountId && e.icalUid === uid) ?? null;
  },
  conflicts(start: number, end: number, uid: string | null): CalendarEvent[] {
    const seen = new Set<string>();
    return visible().filter(
      (e) =>
        !e.allDay &&
        !e.free &&
        e.myResponse !== "declined" &&
        e.icalUid !== uid &&
        e.start < end &&
        e.end > start &&
        !seen.has(e.icalUid ?? e.id) &&
        !!seen.add(e.icalUid ?? e.id),
    );
  },
  grant(accountId: string): { granted: boolean; rsvp: boolean } {
    const g = grants[accountId];
    return { granted: !!g?.granted, rsvp: !!g?.rsvp };
  },
  changed,
  respondSeries,
};

// ---------------------------------------------------------------------------
// Search (the "Calendar" group), used by ./index.ts around the mail search
// ---------------------------------------------------------------------------
const EVENT_OPS = /(^|\s)(type:(event|events|meeting|meetings|calendar)|is:(event|events)|in:calendar)(?=\s|$)/i;

export function withEvents(base: MockHandler): MockHandler {
  return async (args) => {
    const req = args.request as { query: string; accountIds?: string[] | null; accountId?: string | null };
    const q = req.query ?? "";
    const only = EVENT_OPS.exec(q);
    const invite = /(^|\s)has:(invite|invites|invitation|invitations|ics)(?=\s|$)/i.exec(q);
    let mailQuery = only ? q.replace(EVENT_OPS, " ") : q;
    if (invite) mailQuery = mailQuery.replace(invite[0], `${invite[1]}filename:ics`);
    const resp = (await base({ ...args, request: { ...req, query: mailQuery.trim() } })) as SearchResponse;
    if (invite) {
      const chip = resp.chips.find((c) => c.kind === "filename" && /ics/i.test(c.raw));
      if (chip) Object.assign(chip, { kind: "has", label: "Has invitation", raw: invite[0].trim() });
    }
    if (only) resp.chips.unshift({ kind: "type", label: "Calendar events", raw: only[0].trim() });
    const scope = req.accountIds ?? (req.accountId ? [req.accountId] : null);
    resp.events = searchEvents(mailQuery, !!only, scope, !!invite);
    if (only) {
      resp.hits = [];
      resp.attachments = [];
      resp.people = [];
    }
    return resp;
  };
}

function searchEvents(query: string, only: boolean, scope: string[] | null, mailOnly: boolean): CalendarEvent[] {
  if (mailOnly) return [];
  const words: string[] = [];
  const from: string[] = [];
  for (const raw of query.toLowerCase().split(/\s+/).filter(Boolean)) {
    const [op, v] = raw.includes(":") ? raw.split(":", 2) : [null, raw];
    if (op === "from") from.push(v.replace(/"/g, ""));
    else if (op === "to") words.push(v.replace(/"/g, ""));
    else if (op) return only ? [] : [];
    else if (!raw.startsWith("-")) words.push(raw.replace(/"/g, ""));
  }
  const pool = visible().filter((e) => !scope || scope.includes(e.accountId));
  if (!words.length && !from.length) {
    return only ? pool.filter((e) => e.end > NOW).sort((a, b) => a.start - b.start).slice(0, 50) : [];
  }
  const hay = (e: CalendarEvent) =>
    [e.summary, e.description, e.location, e.organizer?.name, e.organizer?.email, ...e.attendees.flatMap((a) => [a.name, a.email])]
      .filter(Boolean)
      .join(" ")
      .toLowerCase();
  const hits = pool.filter(
    (e) =>
      words.every((w) => hay(e).includes(w)) &&
      from.every((f) => `${e.organizer?.name ?? ""} ${e.organizer?.email ?? ""}`.toLowerCase().includes(f)),
  );
  // One per series: the instance nearest to now.
  const bySeries = new Map<string, CalendarEvent>();
  for (const e of hits) {
    const k = e.recurringEventId ?? e.id;
    const cur = bySeries.get(k);
    if (!cur || Math.abs(e.start - NOW) < Math.abs(cur.start - NOW)) bySeries.set(k, e);
  }
  return [...bySeries.values()].sort((a, b) => Math.abs(a.start - NOW) - Math.abs(b.start - NOW)).slice(0, only ? 50 : 8);
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------
const INVITE_THREADS: Record<string, string> = { [`${HL}/t-weekly-sync`]: "weekly-sync" };

function grant(accountId: string, rsvp: boolean) {
  const g = grants[accountId];
  if (!g) throw { code: "notFound", message: "unknown account" };
  g.granted = true;
  g.rsvp = g.rsvp || rsvp;
  g.syncedAt = Date.now();
  if (!calendars.some((c) => c.accountId === accountId))
    calendars.push({ accountId, id: EMAIL[accountId], summary: "Sam Okafor", color: "#e67c73", selected: true, primary: true, accessRole: "owner" });
  changed([accountId]);
}


/** The "Weekly sync" series invite (next instance) and its answers. */
const seriesSent = new Map<string, InviteCard["sent"]>();

function seriesCard(accountId: string, threadId: string): InviteCard | null {
  const series = INVITE_THREADS[`${accountId}/${threadId}`];
  if (!series) return null;
  const next = events.filter((e) => e.recurringEventId === series && e.end > Date.now()).sort((a, b) => a.start - b.start)[0];
  if (!next) return null;
  const conflicts = visible().filter((e) => e !== next && !e.allDay && !e.free && e.myResponse !== "declined" && e.start < next.end && e.end > next.start);
  const g = grants[accountId];
  const route = g.rsvp && g.granted ? "calendar" : "email";
  return {
    accountId,
    threadId,
    messageId: "m-weekly-sync",
    uid: next.icalUid,
    method: "request",
    event: next,
    inCalendar: true,
    recurring: true,
    conflicts,
    canRespond: true,
    route,
    rsvpAvailable: route === "email",
    canPropose: false,
    response: next.myResponse,
    sent: seriesSent.get(series) ?? null,
    timeZone: null,
    updated: false,
    changes: [],
    previous: null,
    calendarConnected: g.granted,
  };
}

/** respond_to_invite for the series thread (./invites.ts delegates here). */
async function respondSeries(accountId: string, threadId: string, response: EventAttendee["response"], comment: string | null): Promise<InviteCard> {
  const series = INVITE_THREADS[`${accountId}/${threadId}`];
  if (!series) throw { code: "notFound", message: "there's no invitation in this conversation" };
  const g = grants[accountId];
  for (const e of events) {
    if (e.recurringEventId !== series || e.end < Date.now()) continue;
    e.myResponse = response;
    e.attendees = e.attendees.map((a) => (a.self ? { ...a, response } : a));
  }
  seriesSent.set(series, { response: response as "accepted", via: g.rsvp ? "calendar" : "email", sequence: 0, comment, proposedStart: null, proposedEnd: null, at: Date.now() });
  changed([accountId]);
  return seriesCard(accountId, threadId)!;
}

export const calendarHandlers: Record<string, MockHandler> = {
  calendar_status: () => status(),
  connect_calendar: async ({ accountId, rsvp }) => {
    await new Promise((r) => setTimeout(r, 700));
    grant(accountId as string, !!rsvp);
    return status();
  },
  // Reconnect asks for read-only calendar in the same consent when
  // calendar.connectOnSignIn is on (as the backend does), so Personal gets
  // connected by it. ?mockCalendarConsent=untick plays the user unticking
  // the calendar box: mail reconnects, the calendar stays not connected.
  reconnect_account: async (args) => {
    const account = await mailHandlers.reconnect_account(args);
    const settings = (await mailHandlers.get_settings({})) as Settings;
    const untick = typeof location !== "undefined" && new URLSearchParams(location.search).get("mockCalendarConsent") === "untick";
    const id = args.accountId as string;
    if (settings.calendar.connectOnSignIn && !untick && grants[id] && !grants[id].granted) grant(id, false);
    return account;
  },
  set_calendar_selected: ({ accountId, calendarId, selected }) => {
    const c = calendars.find((x) => x.accountId === accountId && x.id === calendarId);
    if (!c) throw { code: "notFound", message: `unknown calendar ${calendarId}` };
    c.selected = !!selected;
    changed([accountId]);
    return status();
  },
  list_events: ({ fromMs, toMs, accountIds }) =>
    visible()
      .filter((e) => e.start < toMs && e.end > fromMs && (!accountIds || (accountIds as string[]).includes(e.accountId)))
      .sort((a, b) => Number(b.allDay) - Number(a.allDay) || a.start - b.start),
  get_event: ({ accountId, calendarId, eventId }): EventDetail | null => {
    const event = events.find((e) => e.accountId === accountId && e.calendarId === calendarId && e.id === eventId);
    if (!event) return null;
    return {
      event,
      calendar: calendars.find((c) => c.accountId === accountId && c.id === calendarId) ?? null,
      thread: event.recurringEventId === "weekly-sync" ? { accountId: HL, threadId: "t-weekly-sync", subject: "Invitation: Weekly sync" } : null,
    };
  },
  event_invite: ({ accountId, threadId }): InviteCard | null => seriesCard(accountId as string, threadId as string),
  person_meetings: ({ email }): PersonMeetings => {
    const e = String(email).toLowerCase();
    const withThem = visible().filter(
      (x) => x.myResponse !== "declined" && x.attendees.some((a) => !a.self && a.email === e && a.response !== "declined"),
    );
    const past = withThem.filter((x) => x.start < Date.now()).sort((a, b) => b.start - a.start);
    const next = withThem.filter((x) => x.start >= Date.now()).sort((a, b) => a.start - b.start);
    return { email: e, last: past[0] ?? null, next: next[0] ?? null, count: withThem.length };
  },
  respond_to_event: async ({ accountId, calendarId, eventId, response }) => {
    await new Promise((r) => setTimeout(r, 300));
    if (!grants[accountId as string]?.rsvp) throw { code: "invalidInput", message: "Turn on RSVP for this account first (Settings → Calendar)" };
    const e = events.find((x) => x.accountId === accountId && x.calendarId === calendarId && x.id === eventId);
    if (!e) throw { code: "notFound", message: "that event isn't on your calendar" };
    e.myResponse = response;
    e.attendees = e.attendees.map((a) => (a.self ? { ...a, response } : a));
    changed([accountId]);
    return e;
  },
  calendar_sync_now: () => {
    for (const g of Object.values(grants)) if (g.granted) g.syncedAt = Date.now();
    changed([NW, HL]);
  },
};
