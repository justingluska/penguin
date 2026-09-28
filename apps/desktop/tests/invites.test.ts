// Invitation helpers (features/calendar/invite.ts): the row chip's state,
// optimistic answers, compact times, "What changed", the organizer's time
// and the propose-new-time picker. Fictional people and .example domains.
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import type { CalendarEvent, EventAttendee, InviteChip, InviteSnapshot } from "../src/lib/types.ts";
import {
  changeLines,
  chipView,
  chipWhen,
  compactRange,
  defaultProposal,
  guestSummary,
  organizerTime,
  parseProposal,
  replyText,
  routeHint,
  shownResponse,
} from "../src/features/calendar/invite.ts";

const at = (d: number, h: number, m = 0) => new Date(2026, 8, d, h, m).getTime(); // Sep d, 2026 local
const NOW = at(25, 12);

function chip(over: Partial<InviteChip> = {}): InviteChip {
  return {
    messageId: "m1",
    uid: "roadmap@linden.example",
    method: "request",
    updated: false,
    summary: "Roadmap review",
    start: at(29, 10),
    end: at(29, 11),
    allDay: false,
    startDate: null,
    endDate: null,
    recurring: false,
    response: "needsAction",
    replier: null,
    canRespond: true,
    conflict: null,
    conflicts: 0,
    ...over,
  };
}

const person = (email: string, response: EventAttendee["response"], extra: Partial<EventAttendee> = {}): EventAttendee => ({
  email,
  name: null,
  response,
  organizer: false,
  self: false,
  optional: false,
  resource: false,
  ...extra,
});

test("compact times read like a calendar", () => {
  assert.equal(compactRange(at(29, 10), at(29, 11)), "10–11am");
  assert.equal(compactRange(at(29, 10, 30), at(29, 11)), "10:30–11am");
  assert.equal(compactRange(at(29, 11, 30), at(29, 12, 30)), "11:30am–12:30pm");
  assert.equal(compactRange(at(29, 13), at(29, 14, 15)), "1–2:15pm");
  assert.equal(chipWhen({ start: at(29, 10), end: at(29, 11), allDay: false }), "Tue Sep 29 · 10–11am");
  assert.equal(chipWhen({ start: at(29, 0), end: at(30, 0), allDay: true }), "Tue Sep 29 · All day");
  assert.equal(chipWhen({ start: at(29, 0), end: at(32, 0), allDay: true }), "Sep 29 – Oct 1");
});

test("a new invite offers Yes / Maybe / No and names a clash", () => {
  const v = chipView(chip({ conflict: "Standup", conflicts: 2 }), null, NOW);
  assert.equal(v.buttons, true);
  assert.equal(v.state, null);
  assert.equal(v.tag, null);
  assert.equal(v.conflict, "Conflicts with Standup +1");
  assert.equal(v.when, "Tue Sep 29 · 10–11am");
});

test("answered, updated, cancelled, over", () => {
  const going = chipView(chip({ response: "accepted", conflict: "Standup", conflicts: 1 }), null, NOW);
  assert.equal(going.buttons, false);
  assert.equal(going.state, "Going");
  assert.equal(going.conflict, "Conflicts with Standup");
  // Declined: the clash no longer matters.
  assert.equal(chipView(chip({ response: "declined", conflict: "Standup", conflicts: 1 }), null, NOW).conflict, null);
  assert.equal(chipView(chip({ updated: true }), null, NOW).tag, "Updated");
  const cancel = chipView(chip({ method: "cancel", canRespond: false, response: null }), null, NOW);
  assert.equal(cancel.tag, "Canceled");
  assert.equal(cancel.buttons, false);
  assert.equal(cancel.muted, true);
  const over = chipView(chip(), null, at(29, 12));
  assert.equal(over.buttons, false);
  assert.equal(over.muted, true);
  // A series keeps going past its first instance.
  assert.equal(chipView(chip({ recurring: true }), null, at(29, 12)).buttons, true);
});

test("an answer shows at once, and holds until fresh data arrives", () => {
  const o = { response: "accepted" as const, baseline: "needsAction" as const, pending: true };
  const v = chipView(chip(), o, NOW);
  assert.equal(v.pending, true);
  assert.equal(v.state, "Going");
  assert.equal(v.buttons, false);
  // Sent; the list hasn't refreshed yet: still shown.
  assert.equal(shownResponse("needsAction", { ...o, pending: false }), "accepted");
  // Fresh data arrived (even if something else changed it since): the data wins.
  assert.equal(shownResponse("accepted", { ...o, pending: false }), "accepted");
  assert.equal(shownResponse("declined", { ...o, pending: false }), "declined");
  // Rolled back: no override, the data again.
  assert.equal(shownResponse("needsAction", null), "needsAction");
});

test("replies to your own events", () => {
  const ana = person("ana@harborlabs.example", "accepted", { name: "Ana Sousa" });
  assert.equal(replyText({ method: "reply", replier: ana }), "Ana accepted");
  assert.equal(replyText({ method: "reply", replier: { ...ana, response: "tentative" } }), "Ana said maybe");
  assert.equal(replyText({ method: "counter", replier: ana }), "Ana proposed a new time");
  assert.equal(replyText({ method: "request", replier: null }), null);
  const v = chipView(chip({ method: "reply", replier: ana, canRespond: false, response: null }), null, NOW);
  assert.equal(v.reply, "Ana accepted");
  assert.equal(v.buttons, false);
});

test("what changed, old → new", () => {
  const prev: InviteSnapshot = {
    method: "request",
    uid: "retro@harborlabs.example",
    sequence: 0,
    recurrenceId: null,
    recurring: false,
    summary: "Launch retro",
    location: "Room 2",
    start: at(29, 15),
    end: at(29, 16),
    allDay: false,
    startDate: null,
    endDate: null,
    timeZone: null,
    organizer: null,
    attendees: [person("theo@harborlabs.example", "accepted"), person("ana@harborlabs.example", "accepted")],
  };
  const event = {
    summary: "Launch retro",
    location: "Dock room",
    start: at(30, 16),
    end: at(30, 17),
    allDay: false,
    attendees: [person("theo@harborlabs.example", "accepted"), person("ana@harborlabs.example", "accepted"), person("grace.kim@alderpoint.example", "needsAction")],
  } as unknown as CalendarEvent;
  const lines = changeLines({ changes: ["time", "location", "guests"], previous: prev, event });
  assert.deepEqual(
    lines.map((l) => [l.label, l.before, l.after]),
    [
      ["Time", "Tue Sep 29 · 3–4pm", "Wed Sep 30 · 4–5pm"],
      ["Location", "Room 2", "Dock room"],
      ["Guests", "2", "1 added"],
    ],
  );
  assert.deepEqual(changeLines({ changes: ["time"], previous: null, event }), []);
});

test("the organizer's time only when it differs and the zone is real", () => {
  const t = Date.UTC(2026, 8, 29, 14, 0); // 10:00 in New York
  assert.equal(organizerTime(t, false, "America/Los_Angeles", "America/New_York"), "Tue 7:00 AM in Los Angeles");
  assert.equal(organizerTime(t, false, "America/New_York", "America/New_York"), null);
  assert.equal(organizerTime(t, true, "America/Los_Angeles", "America/New_York"), null);
  // Outlook's Windows zone names aren't IANA: nothing to convert with.
  assert.equal(organizerTime(t, false, "Eastern Standard Time", "Europe/Berlin"), null);
  assert.equal(organizerTime(t, false, null, "Europe/Berlin"), null);
});

test("the propose picker starts on the next weekday and checks its input", () => {
  // Fri Sep 25 10:00–10:45 → Mon Sep 28, same time and length.
  const f = defaultProposal({ start: at(25, 10), end: at(25, 10, 45), allDay: false });
  assert.deepEqual(f, { date: "2026-09-28", start: "10:00", end: "10:45" });
  assert.deepEqual(parseProposal(f, NOW), { start: at(28, 10), end: at(28, 10, 45) });
  assert.deepEqual(parseProposal({ ...f, end: "09:00" }, NOW), { error: "The end must be after the start" });
  assert.deepEqual(parseProposal({ ...f, date: "2026-09-01" }, NOW), { error: "Pick a time in the future" });
  assert.deepEqual(parseProposal({ ...f, date: "" }, NOW), { error: "Pick a day, a start and an end" });
});

test("guest counts and the route hint", () => {
  assert.equal(
    guestSummary([person("a@x.example", "accepted"), person("b@x.example", "accepted"), person("c@x.example", "needsAction"), person("r@x.example", "accepted", { resource: true })]),
    "2 yes, 1 awaiting",
  );
  assert.equal(routeHint({ route: "calendar", rsvpAvailable: false }, "Priya"), null);
  assert.match(routeHint({ route: "email", rsvpAvailable: true }, "Priya")!, /emails your answer to Priya\. Turn on RSVP/);
  assert.equal(routeHint({ route: "email", rsvpAvailable: false }, null), "Penguin emails your answer to the organizer.");
});

// Regression: the card was only in the opened ("full") layout, so selecting
// an invitation in the list (the reading pane beside it) never showed it.
test("both thread layouts show the invite card", () => {
  const src = readFileSync(new URL("../src/features/thread/ThreadView.tsx", import.meta.url), "utf8");
  const card = "<InviteCard thread={thread} />";
  const between = (from: string, to: string) => {
    const a = src.indexOf(from);
    assert.ok(a >= 0, `ThreadView has ${from}`);
    return src.slice(a, src.indexOf(to, a));
  };
  const preview = between('<div className="preview-body v-scroll" ref={scrollRef}>', '<div className="preview-stack">');
  const full = between('<div className="thread-scroll v-scroll" ref={scrollRef}>', '<div className="stack">');
  assert.ok(preview.includes(card), "the reading pane beside the list shows the card above the messages");
  assert.ok(full.includes(card), "the opened thread shows the card above the messages");
});
