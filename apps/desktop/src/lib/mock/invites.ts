// Mock calendar invitations (calendar agent) for `npm run dev:mock`: a new
// invite that clashes with a standup (answered by email: Northwind has no
// RSVP), an updated invite on Harbor Labs' calendar (answered through
// "Google Calendar", with what changed), a cancellation on the IMAP account,
// a guest's reply to your event, and one you already accepted. The rows get
// event chips (list_threads), threads get cards (event_invite), and
// respond_to_invite answers after a short delay. `?mockInviteFail=1` makes
// answering fail, to see the rollback. Fictional people, .example domains.
import type {
  Address,
  CalendarEvent,
  EventAttendee,
  EventResponse,
  InviteAnswer,
  InviteCard,
  InviteChip,
  InviteResponse,
  InviteSnapshot,
  MessageView,
  ThreadSummary,
} from "../types";
import { mockBackend, type MockHandler } from "./index";
import { mockMail } from "./mail";
import { mockCalendar } from "./calendar";

const MIN = 60_000;
const HOUR = 60 * MIN;
const NOW = Date.now();

const NW = "acc-northwind";
const HL = "acc-harbor";
const FM = "acc-okafor";
const EMAIL: Record<string, string> = {
  [NW]: "sam@northwind.example",
  [HL]: "sam@harbor-labs.example",
  [FM]: "sam@okafor.example",
};

/** The next weekday `n` weekdays from today, at h:m local. */
function weekdayAt(n: number, h: number, m = 0): number {
  const d = new Date(NOW);
  d.setHours(h, m, 0, 0);
  let left = n;
  while (left > 0) {
    d.setDate(d.getDate() + 1);
    if (d.getDay() !== 0 && d.getDay() !== 6) left--;
  }
  return d.getTime();
}

const P = (name: string, email: string): Address => ({ name, email });
const priya = P("Priya Natarajan", "priya@linden.example");
const marco = P("Marco Bellini", "marco@northwind.example");
const dana = P("Dana Whitfield", "dana@northwind.example");
const theo = P("Theo Laurent", "theo@harborlabs.example");
const ana = P("Ana Sousa", "ana@harborlabs.example");
const mara = P("Mara Lind", "mara@lindenreads.example");
const grace = P("Grace Kim", "grace.kim@alderpoint.example");

function guest(a: Address, response: EventResponse, extra: Partial<EventAttendee> = {}): EventAttendee {
  return { email: a.email, name: a.name, response, organizer: false, self: false, optional: false, resource: false, ...extra };
}
function me(accountId: string, response: EventResponse): EventAttendee {
  return { email: EMAIL[accountId], name: "Sam Okafor", response, organizer: false, self: true, optional: false, resource: false };
}

interface Fixture {
  accountId: string;
  threadId: string;
  from: Address;
  subject: string;
  body: string;
  minutesAgo: number;
  snap: InviteSnapshot;
  previous?: InviteSnapshot;
  /** Put it on the account's (mock) Google Calendar. */
  onCalendar?: boolean;
  response: EventResponse | null;
  sent: InviteResponse | null;
}

function snap(s: Partial<InviteSnapshot> & Pick<InviteSnapshot, "summary" | "start" | "end" | "uid">): InviteSnapshot {
  return {
    method: "request",
    sequence: 0,
    recurrenceId: null,
    recurring: false,
    location: "",
    allDay: false,
    startDate: null,
    endDate: null,
    timeZone: null,
    organizer: null,
    attendees: [],
    ...s,
  };
}

const roadmapStart = weekdayAt(2, 9, 15);
const retroWas = weekdayAt(3, 15);
const retroNow = weekdayAt(4, 16);
const clubStart = weekdayAt(3, 18, 30);
const critStart = weekdayAt(1, 14);
const offsiteStart = weekdayAt(5, 11);

const FIXTURES: Fixture[] = [
  {
    accountId: NW,
    threadId: "t-inv-roadmap",
    from: priya,
    subject: "Invitation: Roadmap review",
    body: "Priya Natarajan has invited you to Roadmap review.\n\nWe'll walk through the Q4 roadmap and agree on the two launch dates. Pre-read: https://linden.example/docs/q4-roadmap\n\nReply for sam@northwind.example: Yes · No · Maybe · More options",
    minutesAgo: 18,
    snap: snap({
      uid: "roadmap-review@linden.example",
      summary: "Roadmap review",
      start: roadmapStart,
      end: roadmapStart + 45 * MIN,
      location: "Linden HQ, Room 4",
      timeZone: "America/Los_Angeles",
      organizer: priya,
      attendees: [guest(priya, "accepted", { organizer: true }), guest(marco, "needsAction"), guest(dana, "tentative"), me(NW, "needsAction")],
    }),
    response: "needsAction",
    sent: null,
  },
  {
    accountId: HL,
    threadId: "t-inv-retro",
    from: theo,
    subject: "Updated invitation: Launch retro",
    body: "This event has been updated.\n\nChanged: time, location\n\nLaunch retro with Theo Laurent, Ana Sousa and you.\n\nReply for sam@harbor-labs.example: Yes · No · Maybe · More options",
    minutesAgo: 41,
    previous: snap({
      uid: "launch-retro@harborlabs.example",
      sequence: 0,
      summary: "Launch retro",
      start: retroWas,
      end: retroWas + HOUR,
      location: "Room 2",
      organizer: theo,
      attendees: [guest(theo, "accepted", { organizer: true }), guest(ana, "accepted"), me(HL, "accepted")],
    }),
    snap: snap({
      uid: "launch-retro@harborlabs.example",
      sequence: 2,
      summary: "Launch retro",
      start: retroNow,
      end: retroNow + HOUR,
      location: "Dock room, 2nd floor",
      organizer: theo,
      attendees: [guest(theo, "accepted", { organizer: true }), guest(ana, "accepted"), guest(grace, "needsAction", { optional: true }), me(HL, "needsAction")],
    }),
    onCalendar: true,
    response: "needsAction",
    sent: null,
  },
  {
    accountId: FM,
    threadId: "t-inv-bookclub",
    from: mara,
    subject: "Canceled event: Book club",
    body: "Mara Lind has canceled Book club.\n\nSorry all, the library room fell through. Let's pick a new date next month.",
    minutesAgo: 95,
    snap: snap({
      method: "cancel",
      uid: "bookclub-oct@lindenreads.example",
      sequence: 1,
      summary: "Book club",
      start: clubStart,
      end: clubStart + 90 * MIN,
      location: "Linden Public Library",
      organizer: mara,
      attendees: [guest(mara, "accepted", { organizer: true }), me(FM, "accepted")],
    }),
    response: null,
    sent: null,
  },
  {
    accountId: HL,
    threadId: "t-inv-crit-reply",
    from: ana,
    subject: "Accepted: Design crit",
    body: "Ana Sousa has accepted this invitation.",
    minutesAgo: 130,
    snap: snap({
      method: "reply",
      uid: "design-crit@harbor-labs.example",
      summary: "Design crit",
      start: critStart,
      end: critStart + 30 * MIN,
      organizer: P("Sam Okafor", EMAIL[HL]),
      attendees: [guest(ana, "accepted")],
    }),
    response: null,
    sent: null,
  },
  {
    accountId: NW,
    threadId: "t-inv-offsite",
    from: marco,
    subject: "Invitation: Offsite planning",
    body: "Marco Bellini has invited you to Offsite planning.\n\nLet's lock the venue and the agenda.",
    minutesAgo: 200,
    snap: snap({
      uid: "offsite-planning@northwind.example",
      summary: "Offsite planning",
      start: offsiteStart,
      end: offsiteStart + HOUR,
      location: "Northwind office, Loft",
      organizer: marco,
      attendees: [guest(marco, "accepted", { organizer: true }), me(NW, "accepted")],
    }),
    response: "accepted",
    sent: { response: "accepted", via: "email", sequence: 0, comment: null, proposedStart: null, proposedEnd: null, at: NOW - 3 * HOUR },
  },
];

const byThread = new Map(FIXTURES.map((f) => [`${f.accountId}/${f.threadId}`, f]));

function messageView(f: Fixture): MessageView {
  const date = NOW - f.minutesAgo * MIN;
  const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");
  return {
    accountId: f.accountId,
    id: `${f.threadId}-m0`,
    threadId: f.threadId,
    date,
    from: f.from,
    to: [{ name: "Sam Okafor", email: EMAIL[f.accountId] }],
    cc: [],
    bcc: [],
    replyTo: [],
    subject: f.subject,
    snippet: f.body.slice(0, 140),
    bodyText: f.body,
    html:
      `<!doctype html><html><head><meta charset="utf-8"><style>:root{color-scheme:light dark}` +
      `body{margin:0;font:14px/22px Inter,system-ui,sans-serif;color:CanvasText}</style></head><body>` +
      f.body.split("\n\n").map((p) => `<p>${esc(p)}</p>`).join("") +
      `</body></html>`,
    blockedRemoteImages: 0,
    trackersRemoved: 0,
    trackers: [],
    labelIds: ["INBOX", "IMPORTANT"],
    attachments: [{ id: `${f.threadId}-ics`, filename: "invite.ics", mimeType: "text/calendar", size: 2_400, contentId: null, inline: false }],
    unread: f.minutesAgo < 60,
    starred: false,
    senderAuthenticated: true,
  };
}

function synced(f: Fixture): CalendarEvent | null {
  return f.snap.uid ? mockCalendar.byUid(f.accountId, f.snap.uid) : null;
}

function asEvent(f: Fixture): CalendarEvent {
  const s = f.snap;
  return {
    accountId: f.accountId,
    calendarId: "",
    id: "",
    icalUid: s.uid,
    status: "confirmed",
    summary: s.summary,
    description: "",
    location: s.location,
    start: s.start,
    end: s.end,
    allDay: s.allDay,
    startDate: s.startDate,
    endDate: s.endDate,
    organizer: s.organizer,
    attendees: s.attendees.map((a) => (a.self && f.response ? { ...a, response: f.response } : a)),
    myResponse: f.response,
    htmlLink: null,
    conferenceUrl: null,
    conferenceKind: null,
    recurringEventId: null,
    free: false,
    updated: NOW - HOUR,
  };
}

for (const f of FIXTURES) {
  const v = messageView(f);
  mockMail.addThread({ accountId: f.accountId, threadId: f.threadId, subject: f.subject, labelIds: v.labelIds, messages: [v] });
  if (f.onCalendar) mockCalendar.add({ ...asEvent(f), calendarId: EMAIL[f.accountId], id: `ev-${f.threadId}`, htmlLink: null });
}

function answerable(f: Fixture, now: number): boolean {
  return f.snap.method === "request" && f.snap.end > now;
}

function route(f: Fixture): InviteCard["route"] {
  if (!answerable(f, Date.now())) return "none";
  const g = mockCalendar.grant(f.accountId);
  if (f.accountId !== FM && g.rsvp && synced(f)) return "calendar";
  return "email";
}

function conflicts(f: Fixture): CalendarEvent[] {
  if (f.snap.method !== "request" || f.snap.allDay) return [];
  return mockCalendar.conflicts(f.snap.start, f.snap.end, f.snap.uid);
}

function chipOf(f: Fixture): InviteChip {
  const c = conflicts(f);
  const s = f.snap;
  return {
    messageId: `${f.threadId}-m0`,
    uid: s.uid,
    method: s.method,
    updated: s.method === "request" && (s.sequence > 0 || !!f.previous),
    summary: s.summary,
    start: s.start,
    end: s.end,
    allDay: s.allDay,
    startDate: s.startDate,
    endDate: s.endDate,
    recurring: s.recurring,
    response: s.method === "request" ? f.response : null,
    replier: s.method === "reply" || s.method === "counter" ? (s.attendees.find((a) => !a.self) ?? null) : null,
    canRespond: answerable(f, Date.now()),
    conflict: c[0]?.summary ?? null,
    conflicts: c.length,
  };
}

function cardOf(f: Fixture): InviteCard {
  const r = route(f);
  const ev = synced(f) ?? asEvent(f);
  const g = mockCalendar.grant(f.accountId);
  const changes: InviteCard["changes"] = [];
  if (f.previous) {
    if (f.previous.start !== f.snap.start || f.previous.end !== f.snap.end) changes.push("time");
    if (f.previous.location !== f.snap.location) changes.push("location");
    if (f.previous.summary !== f.snap.summary) changes.push("title");
    const emails = (s: InviteSnapshot) => s.attendees.map((a) => a.email).sort().join(",");
    if (emails(f.previous) !== emails(f.snap)) changes.push("guests");
  }
  return {
    accountId: f.accountId,
    threadId: f.threadId,
    messageId: `${f.threadId}-m0`,
    uid: f.snap.uid,
    method: f.snap.method,
    event: { ...ev, attendees: f.snap.attendees.map((a) => (a.self && f.response ? { ...a, response: f.response } : a)), location: f.snap.location },
    inCalendar: !!synced(f),
    recurring: f.snap.recurring,
    conflicts: conflicts(f),
    canRespond: r !== "none",
    route: r,
    rsvpAvailable: r === "email" && f.accountId !== FM && !g.rsvp,
    canPropose: r !== "none" && !f.snap.recurring,
    response: f.snap.method === "request" ? f.response : null,
    sent: f.sent,
    timeZone: f.snap.timeZone,
    updated: f.snap.method === "request" && (f.snap.sequence > 0 || !!f.previous),
    changes,
    previous: f.previous ?? null,
    calendarConnected: g.granted,
  };
}

/** Invitations waiting for your answer, soonest first (the Invites smart view, mock/smart.ts). */
export function mockPendingInvites(): { accountId: string; threadId: string; start: number }[] {
  return FIXTURES.map((f) => ({ f, chip: chipOf(f) }))
    .filter(({ chip }) => chip.method === "request" && chip.canRespond && (chip.response === null || chip.response === "needsAction"))
    .map(({ f, chip }) => ({ accountId: f.accountId, threadId: f.threadId, start: chip.start }))
    .sort((a, b) => a.start - b.start);
}

/** list_threads with event chips on the invitation rows. */
export function withInvites(base: MockHandler): MockHandler {
  return async (args) => {
    const rows = (await base(args)) as ThreadSummary[];
    return rows.map((t) => {
      const f = byThread.get(`${t.accountId}/${t.threadId}`);
      return f ? { ...t, invite: chipOf(f) } : t;
    });
  };
}

/** event_invite: the fixtures' cards, else the calendar mock's. */
export function withInviteCards(base: MockHandler): MockHandler {
  return async (args) => {
    const f = byThread.get(`${args.accountId}/${args.threadId}`);
    return f ? cardOf(f) : base(args);
  };
}

function fail(): boolean {
  try {
    return new URLSearchParams(location.search).get("mockInviteFail") === "1";
  } catch {
    return false;
  }
}

export const inviteHandlers: Record<string, MockHandler> = {
  respond_to_invite: async ({ accountId, threadId, response, comment, proposal }) => {
    await new Promise((r) => setTimeout(r, 700));
    if (fail()) throw { code: "network", message: "Offline; nothing was sent" };
    const f = byThread.get(`${accountId}/${threadId}`);
    if (!f) return mockCalendar.respondSeries(accountId, threadId, response, (comment as string | null) ?? null);
    const r = route(f);
    if (r === "none") throw { code: "invalidInput", message: "This invitation can't be answered" };
    f.response = response as InviteAnswer;
    f.sent = {
      response: response as InviteAnswer,
      via: r === "calendar" ? "calendar" : "email",
      sequence: f.snap.sequence,
      comment: (comment as string | null) ?? null,
      proposedStart: (proposal as { start: number } | null)?.start ?? null,
      proposedEnd: (proposal as { end: number } | null)?.end ?? null,
      at: Date.now(),
    };
    const ev = synced(f);
    if (ev) {
      ev.myResponse = f.response;
      ev.attendees = ev.attendees.map((a) => (a.self ? { ...a, response: f.response! } : a));
      mockCalendar.changed([f.accountId]);
    }
    setTimeout(() => mockBackend.emit("penguin://mail-changed", { accountId, threadIds: [threadId] }), 20);
    return cardOf(f);
  },
};
