// Privacy details copy per removed tracker (features/message-body/privacy.ts).
import { test } from "node:test";
import assert from "node:assert/strict";
import {
  QUERY_NOTE,
  linkCopy,
  listNames,
  privacyFacts,
  privacySummary,
  trackerCopy,
  trackerLabel,
  unlistedTrackers,
} from "../src/features/message-body/privacy.ts";
import type { MessageView, TrackerRemoved } from "../src/lib/types.ts";

const known: TrackerRemoved = {
  host: "pixel.mailerlite.example",
  path: "/o/5f2c1a9e",
  kind: "knownTracker",
  company: "MailerLite",
  rule: "host pixel.mailerlite.example",
  hadQuery: true,
  count: 1,
  status: "removed",
};

test("a known tracker is named by its company and matched rule", () => {
  const c = trackerCopy(known);
  assert.equal(c.title, "MailerLite");
  assert.equal(c.tag, "Known tracker");
  assert.equal(c.address, "pixel.mailerlite.example/o/5f2c1a9e");
  assert.match(c.lines[0], /MailerLite/);
  assert.match(c.lines[1], /matched by its host, pixel\.mailerlite\.example/);
  // The query-string note is said once for the list, not per tracker.
  assert.ok(!c.lines.some((l) => /unique to you/.test(l)));
  assert.match(QUERY_NOTE, /your email address or an ID unique to you/);
});

test("no company: the host is the title; paths and families read as sentences", () => {
  const c = trackerCopy({ ...known, company: null, rule: "path /wf/open", hadQuery: false });
  assert.equal(c.title, "pixel.mailerlite.example");
  assert.match(c.lines[0], /known email-tracking service/);
  assert.match(c.lines[1], /matched by its path, \/wf\/open/);
  assert.ok(!c.lines.some((l) => /unique to you/.test(l)));
  assert.match(trackerCopy({ ...known, rule: "host family cmail*" }).lines[1], /matched by its domain, cmail\*/);
});

test("hidden images explain the pixel and repeat counts", () => {
  const c = trackerCopy({ ...known, kind: "hiddenImage", company: null, rule: "hidden image", count: 3 });
  assert.equal(c.tag, "Hidden pixel");
  assert.equal(c.title, "pixel.mailerlite.example");
  assert.match(c.lines[0], /3×3 pixels or smaller\) or hidden image/);
  assert.match(c.lines.at(-1)!, /3 times/);
});

test("counts", () => {
  assert.equal(trackerLabel(1), "1 tracker removed");
  assert.equal(trackerLabel(4), "4 trackers removed");
  assert.equal(unlistedTrackers(60, [{ ...known, count: 2 }, known]), 57);
  assert.equal(unlistedTrackers(3, [{ ...known, count: 3 }]), 0);
});

// A newsletter's privacy fields, as get_thread returns them.
function msg(p: Partial<MessageView>): MessageView {
  return {
    accountId: "sam@mail.example",
    id: "m1",
    threadId: "t1",
    date: 0,
    from: { name: "News", email: "news@shop.example" },
    to: [],
    cc: [],
    bcc: [],
    replyTo: [],
    subject: "Sale",
    snippet: "",
    bodyText: "",
    html: "",
    blockedRemoteImages: 0,
    trackersRemoved: 0,
    trackers: [],
    labelIds: [],
    attachments: [],
    unread: false,
    starred: false,
    ...p,
  };
}

test("tracker statuses explain what blocking-off did", () => {
  assert.match(trackerCopy({ ...known, status: "held" }).lines.at(-1)!, /will load if you load images/);
  assert.match(trackerCopy({ ...known, status: "loaded" }).lines.at(-1)!, /loaded with the images/);
  assert.ok(!trackerCopy(known).lines.some((l) => /blocking is off/.test(l)));
});

test("the privacy row summarizes removals, cleaned links and loaded trackers", () => {
  assert.equal(privacySummary(msg({})), null);
  assert.deepEqual(privacySummary(msg({ trackersRemoved: 2, trackers: [{ ...known, count: 2 }] })), {
    label: "2 trackers removed",
    tone: "quiet",
  });
  assert.deepEqual(privacySummary(msg({ trackersRemoved: 1, linksCleaned: 3 })), {
    label: "1 tracker removed · 3 links cleaned",
    tone: "quiet",
  });
  // Blocking off, images held: the images count covers them; nothing to summarize.
  assert.equal(privacySummary(msg({ trackersAllowed: 2, blockedRemoteImages: 5, trackers: [{ ...known, count: 2, status: "held" }] })), null);
  // Blocking off, images loaded: a warning.
  assert.deepEqual(privacySummary(msg({ trackersAllowed: 1, trackers: [{ ...known, status: "loaded" }] })), {
    label: "1 tracker loaded",
    tone: "warn",
  });
});

test("message details list every count", () => {
  assert.equal(privacyFacts(msg({ trackersRemoved: 1, blockedRemoteImages: 3 })), "1 tracker removed · 3 remote images blocked");
  assert.equal(
    privacyFacts(msg({ trackersAllowed: 2, trackers: [{ ...known, count: 2, status: "loaded" }], linksCleaned: 1 })),
    "0 trackers removed · 2 trackers loaded · 0 remote images blocked · 1 link cleaned",
  );
});

test("link copy names the parameters, never values, and is honest about click trackers", () => {
  assert.equal(listNames(["mc_eid"]), "mc_eid");
  assert.equal(listNames(["mc_eid", "_hsenc", "fbclid"]), "mc_eid, _hsenc and fbclid");
  const on = linkCopy(msg({ linksCleaned: 2, linkParams: ["mc_eid", "fbclid"], trackedLinks: 1 }), true)!;
  assert.match(on[0], /removed mc_eid and fbclid from 2 links/);
  assert.match(on[1], /1 link goes through the sender's click tracker/);
  assert.equal(on.length, 2);
  // Nothing found and stripping on: no section.
  assert.equal(linkCopy(msg({}), true), null);
  // Stripping off: say so.
  const off = linkCopy(msg({ trackedLinks: 2 }), false)!;
  assert.match(off[0], /2 links go through/);
  assert.match(off.at(-1)!, /off \(Settings → Privacy\)/);
});
