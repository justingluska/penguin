// OWNER: search/command/compose agent.
//
// Mock search, send and setup over the fictional world of the design mockups
// (Sam Okafor with Northwind, Harbor Labs and Personal accounts; Mike Delgado
// at Cedar & Pine Property, and so on). Every address uses .example domains.
//
// Dev knobs (localStorage):
//   penguin.mock.unconfigured = 1  → OAuth client not set up and no accounts,
//                                    so App shows onboarding.
//   penguin.mock.addFails = 1      → add_account rejects (to review the error UI).
//   penguin.mock.draftsOffline = 1 → save_draft fails like a network error.
// The URL param ?mock=unconfigured / ?mock=addFails / ?mock=draftsOffline does the same.
// Accounts, sync status and threads live in the inbox mock (mockMail); this
// file adds the search-only threads to it and drives the onboarding flow.
import type {
  Account,
  Address,
  AttachmentHit,
  AttachmentMeta,
  PersonHit,
  SearchChip,
  SearchHit,
  SearchRequest,
  SearchResponse,
  CommandError,
  CommandErrorCode,
  Draft,
  DraftRef,
  OpenedDraft,
  OutgoingAttachment,
  Reminder,
  ReminderDue,
  ScheduledSend,
  ScheduledSentBatch,
  ThreadView,
  MessageView,
} from "../types";
import { mailHandlers, mockBrowserSignIn, mockMail, removeMockThread } from "./mail";
import { mockBackend, type MockHandler } from "./index";
import { composeHandlers } from "./compose";
import { tokenize } from "../../features/search/query";
import { dateChipLabel, parseDatePhrase } from "../../features/search/dates";
import { embedded, meaningMatch, semanticState, type MeaningMatch } from "./semantic";

/** penguin_core::query::DATE_ERROR */
const DATE_ERROR = "Couldn't read that date";

// ---------------------------------------------------------------------------
// World
// ---------------------------------------------------------------------------

export const MOCK_ACCOUNT_IDS = {
  work: "acc-northwind",
  harbor: "acc-harbor",
  personal: "acc-personal",
} as const;
const W = MOCK_ACCOUNT_IDS.work;
const H = MOCK_ACCOUNT_IDS.harbor;
const P = MOCK_ACCOUNT_IDS.personal;

const ACCOUNT_EMAIL: Record<string, string> = {
  [W]: "sam@northwind.example",
  [H]: "sam@harbor-labs.example",
  [P]: "sam.okafor@gmail.example",
};

const ppl = {
  mike: { name: "Mike Delgado", email: "mike@cedarpine.example" },
  cedar: { name: "Cedar & Pine Property", email: "office@cedarpine.example" },
  osei: { name: "Mike Osei", email: "mike@harborlabs.example" },
  priya: { name: "Priya Natarajan", email: "priya@linden.example" },
  marco: { name: "Marco Bellini", email: "marco@linden.example" },
  dana: { name: "Dana Whitfield", email: "dana@whitfield.example" },
  grace: { name: "Grace Kim", email: "grace@northwind.example" },
  ravi: { name: "Ravi Menon", email: "ravi@harborlabs.example" },
  jonas: { name: "Jonas Weber", email: "jonas@alpinevc.example" },
  lena: { name: "Lena Park", email: "lena@okafor.example" },
  theo: { name: "Theo Laurent", email: "theo@northwind.example" },
  kofi: { name: "Kofi Mensah", email: "kofi@signalcast.example" },
  ana: { name: "Ana Sousa", email: "ana@northwind.example" },
  ines: { name: "Ines Carvalho", email: "ines@alpinevc.example" },
  ledgerly: { name: "Ledgerly Billing", email: "billing@ledgerly.example" },
  northline: { name: "Northline Air", email: "trips@northlineair.example" },
  plumber: { name: "Alder Plumbing", email: "service@alderplumbing.example" },
  sam: { name: "Sam Okafor", email: "sam.okafor@gmail.example" },
} satisfies Record<string, Address>;

interface MockAtt extends AttachmentMeta {
  /** Extracted text (PDF/DOCX) so "text inside attachments" can match. */
  text?: string;
}

interface MockMsg {
  accountId: string;
  threadId: string;
  id: string;
  subject: string;
  from: Address;
  to: Address[];
  date: string; // ISO
  body: string;
  labelIds: string[];
  unread?: boolean;
  starred?: boolean;
  attachments?: MockAtt[];
}

let attSeq = 0;
function att(filename: string, size: number, text?: string): MockAtt {
  const ext = filename.split(".").pop()!.toLowerCase();
  const mime: Record<string, string> = {
    pdf: "application/pdf",
    png: "image/png",
    jpg: "image/jpeg",
    docx: "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
    xlsx: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
    ics: "text/calendar",
    key: "application/x-iwork-keynote-sffkey",
  };
  return {
    id: `att-${++attSeq}`,
    filename,
    mimeType: mime[ext] ?? "application/octet-stream",
    size,
    contentId: null,
    inline: false,
    text,
  };
}

const me = (acc: string): Address => ({ name: "Sam Okafor", email: ACCOUNT_EMAIL[acc] });

const MSGS: MockMsg[] = [
  // --- Cedar & Pine: the lease thread world (Personal) ---
  {
    accountId: P, threadId: "t-lease-renewal", id: "m-lease-1", subject: "Lease renewal — 418 Alder St, Unit 3B",
    from: ppl.mike, to: [me(P)], date: "2026-04-10T15:12:00", labelIds: ["INBOX", "Label_home"],
    body: "Hi Sam, your lease is up for renewal on June 1. I've drafted the renewal paperwork and will send it for signature next week. Let me know if anything changed on your side.",
  },
  {
    accountId: P, threadId: "t-lease-renewal", id: "m-lease-2", subject: "Re: Lease renewal — 418 Alder St, Unit 3B",
    from: me(P), to: [ppl.mike], date: "2026-04-14T09:03:00", labelIds: ["SENT", "Label_home"],
    body: "Thanks Mike. Nothing has changed. Happy to renew the lease on the same terms.",
  },
  {
    accountId: P, threadId: "t-lease-renewal", id: "m-lease-3", subject: "Re: Lease renewal — 418 Alder St, Unit 3B",
    from: ppl.mike, to: [me(P)], date: "2026-04-18T11:41:00", labelIds: ["INBOX", "Label_home"], starred: true,
    body: "Great. Attached is the signed lease renewal for June 1, 2026 through May 31, 2027. Rent stays at $2,450, and the 60-day notice clause from the original lease still applies. Keep a copy for your records.",
    attachments: [att("Lease_Renewal_2026_Unit3B.pdf", 421_888, "Residential lease renewal agreement. Lease term: June 1, 2026 to May 31, 2027. Monthly rent $2,450. Sixty (60) days written notice required.")],
  },
  {
    accountId: P, threadId: "t-renewal-terms", id: "m-terms-1", subject: "Re: Renewal terms",
    from: ppl.mike, to: [me(P)], date: "2026-05-02T10:20:00", labelIds: ["INBOX", "Label_home"],
    body: "To answer your question: the lease auto-renews unless either side gives notice by March 31. Otherwise it converts to month-to-month at the same rate.",
    attachments: [att("Renewal_Terms_Summary.pdf", 88_000, "Summary of renewal terms and notice periods for the lease.")],
  },
  {
    accountId: P, threadId: "t-parking", id: "m-parking-1", subject: "Parking spot addendum",
    from: ppl.mike, to: [me(P)], date: "2026-05-06T16:05:00", labelIds: ["INBOX", "Label_home"],
    body: "Good news: spot 14 is now attached to your lease at no extra cost. The addendum is attached; no signature needed.",
    attachments: [att("Lease_Addendum_Parking.pdf", 98_304, "Lease addendum: parking space 14 assigned to Unit 3B at no additional charge.")],
  },
  {
    accountId: P, threadId: "t-inspection", id: "m-insp-1", subject: "Move-in inspection — Unit 3B",
    from: ppl.mike, to: [me(P)], date: "2026-04-22T13:30:00", labelIds: ["INBOX", "Label_home"],
    body: "Per section 4 of the lease, please note any existing damage on the attached form and send it back within 7 days. Photos are welcome.",
    attachments: [att("Move-in_Inspection.pdf", 1_887_436, "Move-in inspection checklist. Kitchen, bath, bedroom, living room. Section 4 of the lease.")],
  },
  {
    accountId: P, threadId: "t-renewal-offer", id: "m-offer-1", subject: "Renewal offer for 2026–27",
    from: ppl.mike, to: [me(P)], date: "2026-03-10T09:00:00", labelIds: ["INBOX", "Label_home"],
    body: "We'd be happy to extend the lease another year at the same rate if you'd like to stay. Let me know by April.",
    attachments: [att("Renewal_Offer_2026.pdf", 64_000, "Offer to renew the lease for 2026 to 2027 at the current rate.")],
  },
  {
    accountId: P, threadId: "t-renewal-offer", id: "m-offer-2", subject: "Re: Renewal offer for 2026–27",
    from: ppl.mike, to: [me(P)], date: "2026-03-12T14:44:00", labelIds: ["INBOX", "Label_home"],
    body: "Following up: happy to extend the lease another year at the same rate if that works for you.",
  },
  {
    accountId: P, threadId: "t-rent-increase", id: "m-rent-1", subject: "Rent increase notice — none this year",
    from: ppl.cedar, to: [me(P)], date: "2026-03-03T08:15:00", labelIds: ["INBOX", "Label_home"],
    body: "This is a courtesy notice: there is no change to the rent under the current lease for the coming term.",
    attachments: [att("Rent_Notice_2026.pdf", 52_000, "No rent increase for the lease term beginning June 2026.")],
  },
  {
    accountId: P, threadId: "t-rent", id: "m-receipt-1", subject: "October rent receipt",
    from: ppl.cedar, to: [me(P)], date: "2026-09-19T07:00:00", labelIds: ["INBOX", "Label_home"],
    body: "Payment of $2,450.00 for 418 Alder St, Unit 3B was received on Sep 19. Thank you.",
    attachments: [att("Receipt_Oct_2026.pdf", 34_000, "Rent receipt $2,450.00 418 Alder St Unit 3B.")],
  },
  {
    accountId: P, threadId: "t-lease-2025", id: "m-lease25-1", subject: "Signed lease — 418 Alder St",
    from: ppl.mike, to: [me(P)], date: "2025-05-20T10:00:00", labelIds: ["Label_home"],
    body: "Welcome to Cedar & Pine! Your signed lease for Unit 3B is attached. Keys are at the front office.",
    attachments: [att("Lease_Unit3B_2025.pdf", 512_000, "Residential lease agreement 418 Alder St Unit 3B. Lease term June 1, 2025 to May 31, 2026.")],
  },
  {
    accountId: P, threadId: "t-water-heater", id: "m-water-1", subject: "Re: Water heater service window",
    from: ppl.plumber, to: [me(P)], date: "2026-09-22T17:40:00", labelIds: ["INBOX", "Label_home"], unread: true,
    body: "Plumber confirmed Thursday between 10 and 12. I'll leave the side gate unlocked so they can reach the water heater.",
  },
  {
    accountId: P, threadId: "t-lake-photos", id: "m-lake-1", subject: "Photos from the lake weekend",
    from: ppl.lena, to: [me(P)], date: "2026-09-20T20:12:00", labelIds: ["INBOX"],
    body: "Finally uploaded everything. The sunrise ones from Sunday came out unreal.",
    attachments: [att("sunrise_dock.jpg", 3_400_000), att("canoe.jpg", 2_900_000)],
  },
  {
    accountId: P, threadId: "t-lisbon", id: "m-lisbon-1", subject: "Your trip to Lisbon — boarding passes",
    from: ppl.northline, to: [me(P)], date: "2026-09-18T06:30:00", labelIds: ["INBOX", "Label_travel"],
    body: "Flight NL 214 departs Oct 9 at 7:25 PM. Seats 14A, 14B confirmed. Boarding passes are attached.",
    attachments: [att("Boarding_Pass_NL214.pdf", 140_000, "Boarding pass flight NL 214 Lisbon seat 14A")],
  },
  // --- Northwind (work) ---
  {
    accountId: W, threadId: "t-priya-q4", id: "m-q4-1", subject: "Q4 brand refresh — final review deck",
    from: ppl.priya, to: [me(W)], date: "2026-09-23T09:41:00", labelIds: ["INBOX", "Label_clients"], unread: true,
    body: "Attached v7 with the updated lockups. Two open questions on the secondary palette before we lock it: keep the warm gray, or drop it? And can we move the stakeholder review to Monday?",
    attachments: [att("Q4_Brand_Refresh_v7.pdf", 8_400_000, "Q4 brand refresh review deck. Lockups, secondary palette, typography.")],
  },
  {
    accountId: W, threadId: "t-offsite", id: "m-offsite-1", subject: "Offsite agenda: Oct 14–15",
    from: ppl.grace, to: [me(W)], date: "2026-09-22T15:02:00", labelIds: ["INBOX"],
    body: "Draft agenda is in the doc. Can you own the Friday retro block? Need an answer by Thursday.",
    attachments: [att("Offsite_Agenda_Oct.docx", 44_000, "Offsite agenda October 14 to 15. Friday retro block.")],
  },
  {
    accountId: W, threadId: "t-pricing-copy", id: "m-pricing-1", subject: "Re: Pricing page copy",
    from: ppl.theo, to: [me(W)], date: "2026-09-19T12:10:00", labelIds: ["INBOX"],
    body: "Version B reads cleaner. I'd cut the second FAQ entirely and move the invoice question up.",
  },
  {
    accountId: W, threadId: "t-design-crit", id: "m-crit-1", subject: "Design crit notes — onboarding flow",
    from: ppl.ana, to: [me(W)], date: "2026-09-21T18:22:00", labelIds: ["INBOX"],
    body: "Cleaned up my notes from today. Biggest theme: step three asks for too much before showing any value.",
  },
  {
    accountId: H, threadId: "t-weekly-sync", id: "m-weekly-1", subject: "Invitation: Weekly sync",
    from: ppl.theo, to: [me(H), ppl.ana], date: "2026-09-20T08:00:00", labelIds: ["INBOX"],
    body: "Thu Sep 24, 10:00 – 10:30 AM · Theo Laurent, Ana Sousa, you",
    attachments: [att("invite.ics", 2_000)],
  },
  {
    accountId: W, threadId: "t-priya-invoice", id: "m-pinv-1", subject: "Invoice 0932 — brand refresh phase 2",
    from: ppl.priya, to: [me(W)], date: "2026-08-28T10:00:00", labelIds: ["INBOX", "Label_clients"],
    body: "Please find attached the invoice for phase 2 of the brand refresh. Net 30, as usual.",
    attachments: [att("Linden_Invoice_0932.pdf", 72_000, "Invoice 0932 Linden Studio brand refresh phase 2 total $18,400")],
  },
  // --- Harbor Labs ---
  {
    accountId: H, threadId: "t-ledgerly", id: "m-hl-1", subject: "Invoice HL-4821 paid — $1,240.00",
    from: ppl.ledgerly, to: [me(H)], date: "2026-09-22T11:00:00", labelIds: ["INBOX", "Label_receipts"],
    body: "Thanks! Your payment to Harbor Labs was received. Receipt attached for your records.",
    attachments: [att("Receipt_HL-4821.pdf", 30_000, "Receipt invoice HL-4821 $1,240.00 paid")],
  },
  {
    accountId: H, threadId: "t-beta-signups", id: "m-beta-1", subject: "Beta: 40 new signups overnight",
    from: ppl.osei, to: [me(H)], date: "2026-09-22T07:45:00", labelIds: ["INBOX"], unread: true,
    body: "Mostly from the changelog post. Week-one retention cohort is holding at 62%, which is the best we've seen.",
    attachments: [att("signups_week38.xlsx", 22_000, "Signups by source, retention cohort week one")],
  },
  {
    accountId: W, threadId: "t-contract", id: "m-contract-1", subject: "Contract countersigned",
    from: ppl.ravi, to: [me(W)], date: "2026-09-21T16:30:00", labelIds: ["INBOX"],
    body: "All set on our side. Scanned copy attached; originals go out by courier Friday.",
    attachments: [att("Harbor_MSA_countersigned.pdf", 610_000, "Master services agreement countersigned Harbor Labs")],
  },
  {
    accountId: H, threadId: "t-osei-lease", id: "m-osei-1", subject: "Office lease walkthrough",
    from: ppl.osei, to: [me(H)], date: "2025-11-04T13:00:00", labelIds: ["INBOX"],
    body: "Walked the new office space. The lease is 3 years with an option to extend; landlord wants an answer by December.",
  },
  {
    accountId: H, threadId: "t-intro-jonas", id: "m-intro-1", subject: "Intro: Jonas ↔ Sam (seed round)",
    from: ppl.ines, to: [me(H), ppl.jonas], date: "2026-09-20T10:00:00", labelIds: ["INBOX"],
    body: "Moving Ines to bcc. Sam, happy to share our data room whenever you're ready.",
  },
  {
    accountId: H, threadId: "t-data-room", id: "m-dr-1", subject: "Data room access",
    from: ppl.jonas, to: [me(H)], date: "2026-09-19T09:00:00", labelIds: ["INBOX", "Label_finance"],
    body: "You should have view access now. Model is in the Finance folder, v3.",
  },
  {
    accountId: W, threadId: "t-podcast", id: "m-pod-1", subject: "Podcast booking — Oct 2 recording",
    from: ppl.kofi, to: [me(W)], date: "2026-09-18T14:00:00", labelIds: ["INBOX"],
    body: "Locked in 2 PM. I'll send the outline and the mic kit by Friday.",
  },
  // --- Mail Sam sent (in:sent, to:new, is:awaiting) ---
  {
    accountId: W, threadId: "t-podcast-intro", id: "m-podintro-1", subject: "Intro: sponsoring a Signalcast episode",
    from: me(W), to: [ppl.kofi], date: "2026-08-27T14:10:00", labelIds: ["SENT"],
    body: "Hi Kofi, Grace suggested I reach out. We'd love to sponsor an episode this fall; happy to share our audience numbers.",
  },
  {
    accountId: W, threadId: "t-q4-notes", id: "m-q4notes-1", subject: "Q4 planning notes",
    from: me(W), to: [ppl.grace, ppl.theo], date: "2026-08-27T09:30:00", labelIds: ["SENT"],
    body: "Sharing my notes before Thursday: hiring plan, the pricing test and the offsite budget.",
  },
  {
    accountId: H, threadId: "t-ines-terms", id: "m-ines-1", subject: "Term sheet questions",
    from: me(H), to: [ppl.ines], date: "2026-09-21T16:20:00", labelIds: ["SENT"],
    body: "Hi Ines, two questions on the pro-rata clause and the board seat. Could we talk this week?",
  },
];

/** People the compose + search autocomplete can suggest before any search. */
export const MOCK_PEOPLE: Address[] = Object.values(ppl).filter((a) => a.email !== ppl.sam.email);

const LABEL_NAMES: Record<string, string> = {
  Label_home: "Home",
  Label_finance: "Finance",
  Label_clients: "Clients",
  Label_travel: "Travel",
  Label_receipts: "Receipts",
};

// "Today" in the mock world matches the mockups.
const NOW = new Date("2026-09-23T10:00:00");

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

interface Filters {
  from: string[];
  to: string[];
  has: string[];
  is: string[];
  in: string[];
  label: string[];
  account: string[];
  subject: string[];
  filename: string[];
  after: number | null;
  before: number | null;
  words: string[];
  phrases: string[];
  neg: string[];
  /** type:event / is:event / in:calendar: calendar events only (no mail hits). */
  eventsOnly: boolean;
  /** Operators simulated as per-message predicates (with their negation applied). */
  preds: Array<(m: MockMsg) => boolean>;
}

const DAY = 86_400_000;
const shortDate = (ms: number) => new Date(ms).toLocaleDateString("en-US", { month: "short", day: "numeric", year: "numeric" });

const cap = (s: string) => s.charAt(0).toUpperCase() + s.slice(1);
const HAS_LABEL: Record<string, string> = {
  pdf: "PDF", image: "image", doc: "document", spreadsheet: "spreadsheet", presentation: "presentation", attachment: "attachment",
};
const IN_LABEL: Record<string, string> = {
  inbox: "In Inbox", sent: "In Sent", drafts: "In Drafts", draft: "In Drafts", trash: "In Trash", spam: "In Spam",
  anywhere: "Anywhere, incl. Trash & Spam", all: "Anywhere, incl. Trash & Spam", done: "Done", archive: "Done", archived: "Done",
  starred: "Starred", important: "Important",
};
const IS_LABEL: Record<string, string> = {
  unread: "Unread", read: "Read", starred: "Starred", unstarred: "Not starred", important: "Important", snoozed: "Snoozed",
  newsletter: "Newsletter", "new-sender": "New sender", "first-outbound": "First email to them", "known-sender": "Someone you've written to", unanswered: "Unanswered",
  replied: "Replied", awaiting: "Awaiting reply", reply: "A reply", sent: "In Sent", draft: "In Drafts",
};
/** penguin-core's aliases for is: values. */
const IS_ALIAS: Record<string, string> = {
  new_sender: "new-sender", newsender: "new-sender", "first-contact": "new-sender", first_outbound: "first-outbound",
  "new-recipient": "first-outbound", unreplied: "unanswered", "needs-reply": "unanswered", answered: "replied",
  "awaiting-reply": "awaiting", waiting: "awaiting", "no-reply": "awaiting", replies: "reply", bulk: "newsletter", newsletters: "newsletter",
  known: "known-sender", known_sender: "known-sender", "known-senders": "known-sender", "people-you-know": "known-sender",
};

// --- Derived facts the real store keeps (first contacts, reply state) ---
const isMine = (a: Address) => Object.values(ACCOUNT_EMAIL).includes(a.email.toLowerCase());
const isSent = (m: MockMsg) => m.labelIds.includes("SENT");
const isReceived = (m: MockMsg) => !isSent(m) && !m.labelIds.includes("DRAFT") && !m.labelIds.includes("SPAM");
const ms = (m: MockMsg) => new Date(m.date).getTime();
const threadOf = (m: MockMsg) => MSGS.filter((x) => x.accountId === m.accountId && x.threadId === m.threadId);
function firstFrom(m: MockMsg): boolean {
  if (!isReceived(m)) return false;
  const e = m.from.email.toLowerCase();
  return !MSGS.some((x) => x !== m && isReceived(x) && x.from.email.toLowerCase() === e && ms(x) < ms(m));
}
function firstTo(m: MockMsg): boolean {
  if (!isSent(m)) return false;
  return m.to.some((r) => !MSGS.some((x) => x !== m && isSent(x) && ms(x) < ms(m) && x.to.some((t) => t.email.toLowerCase() === r.email.toLowerCase())));
}
function isState(m: MockMsg, v: string): boolean {
  const later = threadOf(m).filter((x) => ms(x) > ms(m) && !x.labelIds.includes("DRAFT"));
  switch (v) {
    case "unread": return !!m.unread;
    case "read": return !m.unread;
    case "starred": return !!m.starred;
    case "unstarred": return !m.starred;
    case "important": return false;
    case "snoozed": return false;
    case "newsletter": return m.from.email.startsWith("billing@") || m.from.email.startsWith("trips@");
    case "new-sender": return firstFrom(m);
    case "first-outbound": return firstTo(m);
    case "known-sender": return isReceived(m) && MSGS.some((x) => isSent(x) && x.to.some((t) => t.email.toLowerCase() === m.from.email.toLowerCase()));
    case "unanswered": return isReceived(m) && !isState(m, "newsletter") && !later.some(isSent);
    case "replied": return isReceived(m) && later.some(isSent);
    case "awaiting": return isSent(m) && later.length === 0;
    case "reply": return threadOf(m).some((x) => ms(x) < ms(m));
    case "sent": return isSent(m);
    case "draft": return m.labelIds.includes("DRAFT");
  }
  return false;
}
const attSize = (m: MockMsg) => (m.attachments ?? []).reduce((n, a) => n + a.size, 0);
function bytes(v: string): number | null {
  const x = /^(\d+(?:\.\d+)?)(k|kb|m|mb|g|gb|b)?$/.exec(v.replace(/^[<>=]+/, ""));
  if (!x) return null;
  const mult = { k: 1024, m: 1024 ** 2, g: 1024 ** 3, b: 1 }[(x[2] ?? "b")[0] as "k"] ?? 1;
  return Math.round(+x[1] * mult);
}
function fmtBytes(b: number): string {
  if (b >= 1024 ** 2) return `${+(b / 1024 ** 2).toFixed(1)} MB`;
  if (b >= 1024) return `${+(b / 1024).toFixed(1)} KB`;
  return `${b} bytes`;
}
const WEEKDAY: Record<string, number> = { sun: 0, mon: 1, tue: 2, tues: 2, wed: 3, weds: 3, thu: 4, thur: 4, thurs: 4, fri: 5, sat: 6 };
function dayMask(v: string): number | null {
  if (/^weekends?$/.test(v)) return 0b1000001;
  if (/^(weekdays?|workdays?)$/.test(v)) return 0b0111110;
  let mask = 0;
  for (const part of v.split(",")) {
    const k = part.trim().replace(/s$/, "");
    const d = WEEKDAY[k] ?? WEEKDAY[k.slice(0, 3)];
    if (d === undefined || !/^(sun|mon|tue|wed|thu|fri|sat)/.test(k)) return null;
    mask |= 1 << d;
  }
  return mask || null;
}
function threadLen(v: string): [number, number] | null {
  let x: RegExpExecArray | null;
  if ((x = /^(\d+)\.\.(\d+)$/.exec(v))) return [+x[1], +x[2]];
  if ((x = /^>=(\d+)$/.exec(v))) return [+x[1], Infinity];
  if ((x = /^<=(\d+)$/.exec(v))) return [0, +x[1]];
  if ((x = /^>(\d+)$/.exec(v))) return [+x[1] + 1, Infinity];
  if ((x = /^<(\d+)$/.exec(v))) return [0, +x[1] - 1];
  if ((x = /^(\d+)\+$/.exec(v))) return [+x[1], Infinity];
  if ((x = /^=?(\d+)$/.exec(v))) return [+x[1], +x[1]];
  return null;
}

function opLabel(op: string, v: string, display: string): string {
  switch (op) {
    case "from": return `From ${display}`;
    case "to": case "bcc": return `To ${display}`;
    case "cc": return `Cc ${display}`;
    case "has": return `Has ${HAS_LABEL[v] ?? v}`;
    case "in": return IN_LABEL[v] ?? `In ${cap(v)}`;
    case "is": return IS_LABEL[v] ?? cap(v);
    case "label": return `Label ${display}`;
    case "account": return `Account ${v}`;
    case "subject": return `Subject “${v}”`;
    case "filename": return `Filename ${v}`;
  }
  return `${cap(op)} ${v}`;
}

/** Mirrors penguin-core's chips: operator chips, date chips, quoted text, exclusions; free words get none. */
function parse(q: string): { f: Filters; chips: SearchChip[] } {
  const f: Filters = {
    from: [], to: [], has: [], is: [], in: [], label: [], account: [], subject: [], filename: [],
    after: null, before: null, words: [], phrases: [], neg: [], eventsOnly: false, preds: [],
  };
  const chips: SearchChip[] = [];
  // Operators the mock simulates as predicates: [chip kind, label, predicate].
  const simulated = (op: string, v: string): [string, string, (m: MockMsg) => boolean] | null => {
    switch (op) {
      case "from":
        if (v === "me") return ["from", "From me", (m) => isMine(m.from) || isSent(m)];
        if (v === "new") return ["is", "New sender", firstFrom];
        return null;
      case "to": case "cc": case "bcc":
        if (v === "me") return ["to", "To me", (m) => m.to.some(isMine)];
        if (v === "new" && op === "to") return ["is", "First email to them", firstTo];
        return null;
      case "with": {
        const who = resolvePerson(v);
        return ["with", `With ${who?.name ?? v}`, (m) => matchPerson(m.from, v) || m.to.some((a) => matchPerson(a, v))];
      }
      case "domain": {
        const d = v.replace(/^@/, "");
        const at = (a: Address) => a.email.toLowerCase().endsWith(`@${d}`) || a.email.toLowerCase().endsWith(`.${d}`);
        return ["domain", `Domain ${d}`, (m) => at(m.from) || m.to.some(at)];
      }
      case "is": {
        const s = IS_ALIAS[v] ?? v;
        if (!(s in IS_LABEL) || s === "event") return null;
        return ["is", IS_LABEL[s], (m) => isState(m, s)];
      }
      case "in":
        if (v === "snoozed") return ["is", "Snoozed", () => false];
        return null;
      case "has":
        if (v === "link" || v === "links") return ["has", "Has link", (m) => /https?:\/\/|www\./.test(m.body)];
        if (v === "otp" || v === "code") return ["has", "Has verification code", (m) => /\b(code|verification)\b/i.test(m.body) && /\b\d{6}\b/.test(m.body)];
        if (v === "unsubscribe") return ["has", "Has unsubscribe link", (m) => isState(m, "newsletter")];
        return null;
      case "larger": case "smaller": case "size": {
        const small = op === "smaller" || v.startsWith("<");
        const b = bytes(v);
        if (b === null) return null;
        return ["size", `${small ? "Smaller" : "Larger"} than ${fmtBytes(b)}`, (m) => (small ? attSize(m) < b : attSize(m) >= b)];
      }
      case "messages": {
        const r = threadLen(v);
        if (!r) return null;
        const label = r[1] === Infinity ? `Threads of ${r[0]}+ messages` : r[0] === r[1] ? `Threads of ${r[0]} message${r[0] === 1 ? "" : "s"}` : `Threads of ${r[0]}–${r[1]} messages`;
        return ["messages", label, (m) => {
          const n = threadOf(m).length;
          return n >= r[0] && n <= r[1];
        }];
      }
      case "day": {
        const mask = dayMask(v);
        if (mask === null) return null;
        const names = ["Sundays", "Mondays", "Tuesdays", "Wednesdays", "Thursdays", "Fridays", "Saturdays"].filter((_, i) => mask & (1 << i));
        const label = mask === 0b1000001 ? "On weekends" : mask === 0b0111110 ? "On weekdays" : `On ${names.join(", ")}`;
        return ["day", label, (m) => (mask & (1 << new Date(m.date).getDay())) !== 0];
      }
      case "category":
        return ["label", `Category ${cap(v === "primary" ? "personal" : v)}`, (m) => m.labelIds.includes(`CATEGORY_${(v === "primary" ? "personal" : v).toUpperCase()}`)];
    }
    return null;
  };
  for (const t of tokenize(q)) {
    if (t.kind === "space" || t.kind === "or" || t.kind === "paren") continue;
    if (t.kind === "word" || t.kind === "phrase") {
      const v = t.text.replace(/^-/, "").replace(/"/g, "").toLowerCase();
      if (!v) continue;
      if (t.negated) {
        f.neg.push(v);
        chips.push({ kind: "exclude", label: `Excluding “${v}”`, raw: t.text });
      } else if (t.kind === "phrase") {
        f.phrases.push(v);
        chips.push({ kind: "text", label: `“${v}”`, raw: t.text });
      } else {
        f.words.push(v);
      }
      continue;
    }
    const v = t.value.toLowerCase();
    if (!v) continue; // a dangling "from:" is no filter yet
    const sim = simulated(t.op, v);
    if (sim) {
      const [kind, label, pred] = sim;
      f.preds.push(t.negated ? (m) => !pred(m) : pred);
      chips.push(t.negated ? { kind: "exclude", label: `Not ${label.charAt(0).toLowerCase()}${label.slice(1)}`, raw: t.text } : { kind, label, raw: t.text });
      continue;
    }
    if (t.op === "date" || t.op === "on") {
      // Only date: reads natural language; an unreadable (or negated) one is
      // an error chip that constrains nothing, like penguin-core.
      const r = t.negated ? null : parseDatePhrase(v, NOW);
      if (!r) {
        chips.push({ kind: "error", label: DATE_ERROR, raw: t.text });
        continue;
      }
      if (r.from !== null) f.after = r.from;
      if (r.to !== null) f.before = r.to;
      chips.push({ kind: "date", label: dateChipLabel(v, r), raw: t.text });
      continue;
    }
    if (t.negated) {
      // Only word/phrase exclusion is simulated; operator exclusions still get their chip.
      chips.push({ kind: "exclude", label: `Not ${opLabel(t.op, v, v).toLowerCase()}`, raw: t.text });
      continue;
    }
    switch (t.op) {
      case "from": case "to": case "cc": case "bcc": {
        const who = resolvePerson(v);
        (t.op === "from" ? f.from : f.to).push(v);
        chips.push({ kind: t.op === "bcc" ? "to" : t.op, label: opLabel(t.op, v, who?.name ?? v), raw: t.text });
        break;
      }
      case "has": f.has.push(v); chips.push({ kind: "has", label: opLabel("has", v, v), raw: t.text }); break;
      case "is": f.is.push(v); chips.push({ kind: "is", label: opLabel("is", v, v), raw: t.text }); break;
      case "in": f.in.push(v); chips.push({ kind: "in", label: opLabel("in", v, v), raw: t.text }); break;
      case "label": f.label.push(v); chips.push({ kind: "label", label: opLabel("label", v, LABEL_NAMES[`Label_${v}`] ?? cap(v)), raw: t.text }); break;
      case "account": f.account.push(v); chips.push({ kind: "account", label: opLabel("account", v, v), raw: t.text }); break;
      case "subject": f.subject.push(v); chips.push({ kind: "subject", label: opLabel("subject", v, v), raw: t.text }); break;
      case "filename": f.filename.push(v); chips.push({ kind: "filename", label: opLabel("filename", v, v), raw: t.text }); break;
      case "type":
        if (v === "event") {
          f.eventsOnly = true;
          chips.push({ kind: "type", label: "Calendar events", raw: t.text });
        }
        break;
      case "after": case "before": case "since": case "until": {
        const iso = /^\d{4}[-/]\d{2}[-/]\d{2}$/.test(v) ? new Date(`${v.replace(/\//g, "-")}T00:00:00`).getTime() : NaN;
        const r = Number.isNaN(iso) ? parseDatePhrase(v, NOW) : { from: iso, to: iso + DAY };
        if (!r || r.from === null) break;
        if (t.op === "until") {
          if (r.to === null) break;
          f.before = r.to;
          chips.push({ kind: "before", label: `Until ${shortDate(r.to - DAY)}`, raw: t.text });
        } else if (t.op === "before") {
          f.before = r.from;
          chips.push({ kind: "before", label: `Before ${shortDate(r.from)}`, raw: t.text });
        } else {
          f.after = r.from;
          chips.push({ kind: "after", label: `${t.op === "since" ? "Since" : "After"} ${shortDate(r.from)}`, raw: t.text });
        }
        break;
      }
      case "older_than": case "newer_than": {
        const m = /^(\d+)([hdwmy])$/.exec(v);
        if (m) {
          const days = { h: 1 / 24, d: 1, w: 7, m: 30, y: 365 }[m[2] as "h"]!;
          const ts = NOW.getTime() - +m[1] * days * DAY;
          const unit = { h: "hour", d: "day", w: "week", m: "month", y: "year" }[m[2] as "h"]!;
          const text = `${m[1]} ${unit}${m[1] === "1" ? "" : "s"}`;
          if (t.op === "older_than") {
            f.before = ts;
            chips.push({ kind: "before", label: `Older than ${text}`, raw: t.text });
          } else {
            f.after = ts;
            chips.push({ kind: "after", label: `Newer than ${text}`, raw: t.text });
          }
        }
        break;
      }
    }
  }
  return { f, chips };
}

function resolvePerson(v: string): Address | null {
  const hits = MOCK_PEOPLE.filter((p) => matchPerson(p, v));
  if (hits.length === 0) return null;
  // Prefer the person with the most mail (Mike Delgado over Mike Osei).
  hits.sort((a, b) => countFrom(b) - countFrom(a));
  return hits[0];
}
function matchPerson(p: Address, v: string) {
  return (p.name ?? "").toLowerCase().split(/\s+/).some((w) => w.startsWith(v)) || p.email.toLowerCase().startsWith(v) || (p.name ?? "").toLowerCase().startsWith(v);
}
function countFrom(p: Address) {
  return MSGS.filter((m) => m.from.email === p.email).length;
}

function words(text: string): string[] {
  return text.toLowerCase().split(/[^a-z0-9$@.]+/).filter(Boolean);
}
function termMatches(text: string, term: string): boolean {
  return words(text).some((w) => w.startsWith(term) || (term.length >= 5 && w.startsWith(term.slice(0, -1))));
}

function hasKind(a: MockAtt, v: string): boolean {
  if (v === "attachment") return true;
  if (v === "pdf") return a.mimeType === "application/pdf";
  if (v === "image") return a.mimeType.startsWith("image/");
  if (v === "doc") return a.mimeType.includes("wordprocessing");
  if (v === "spreadsheet") return a.mimeType.includes("sheet");
  if (v === "calendar" || v === "invite") return a.mimeType === "text/calendar";
  if (v === "presentation") return /presentation|keynote/.test(a.mimeType);
  return false;
}

/** `words` = false checks everything but the free words (a match by meaning needs only the filters). */
function msgMatches(m: MockMsg, f: Filters, scope: string | null, set: string[] | null | undefined, words = true): boolean {
  if (scope && m.accountId !== scope) return false;
  if (set && !set.includes(m.accountId)) return false;
  const t = new Date(m.date).getTime();
  if (f.after !== null && t < f.after) return false;
  if (f.before !== null && t >= f.before) return false;
  if (f.from.some((v) => !matchPerson(m.from, v))) return false;
  if (f.to.some((v) => !m.to.some((a) => matchPerson(a, v)))) return false;
  if (f.has.some((v) => !(m.attachments ?? []).some((a) => hasKind(a, v)))) return false;
  if (f.account.some((v) => !ACCOUNT_EMAIL[m.accountId].includes(v))) return false;
  if (f.label.some((v) => !m.labelIds.some((l) => (LABEL_NAMES[l] ?? l).toLowerCase() === v))) return false;
  if (f.subject.some((v) => !m.subject.toLowerCase().includes(v))) return false;
  if (f.filename.some((v) => !(m.attachments ?? []).some((a) => a.filename.toLowerCase().includes(v)))) return false;
  if (f.preds.some((p) => !p(m))) return false;
  for (const v of f.is) {
    if (v === "unread" && !m.unread) return false;
    if (v === "read" && m.unread) return false;
    if (v === "starred" && !m.starred) return false;
  }
  for (const v of f.in) {
    if (v === "anywhere" || v === "all") continue;
    if (v === "done" || v === "archive" || v === "archived") {
      if (m.labelIds.includes("INBOX")) return false;
    } else if (v === "starred") {
      if (!m.starred) return false;
    } else if (!m.labelIds.includes(v === "drafts" ? "DRAFT" : v === "bin" ? "TRASH" : v === "junk" ? "SPAM" : v.toUpperCase())) {
      return false;
    }
  }
  if (!f.in.some((v) => ["anywhere", "all", "trash", "bin", "spam", "junk"].includes(v)) && (m.labelIds.includes("TRASH") || m.labelIds.includes("SPAM"))) return false;
  const hay = `${m.subject} ${m.body} ${m.from.name ?? ""} ${(m.attachments ?? []).map((a) => `${a.filename} ${a.text ?? ""}`).join(" ")}`;
  if (words && f.words.some((w) => !termMatches(hay, w))) return false;
  if (words && f.phrases.some((p) => !hay.toLowerCase().includes(p))) return false;
  if (f.neg.some((w) => termMatches(hay, w))) return false;
  return true;
}

function esc(s: string) {
  return s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");
}

/** Escaped text with <mark> around every term; the same shape Rust returns. */
function markup(text: string, terms: string[]): string {
  if (terms.length === 0) return esc(text);
  const re = new RegExp(`\\b(${terms.map((t) => t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})[a-z]*`, "gi");
  let out = "";
  let last = 0;
  for (const m of text.matchAll(re)) {
    out += esc(text.slice(last, m.index)) + "<mark>" + esc(m[0]) + "</mark>";
    last = m.index! + m[0].length;
  }
  return out + esc(text.slice(last));
}

/** The matching sentence (not the opener), windowed to ~150 chars. */
function snippet(body: string, terms: string[]): string {
  const lower = body.toLowerCase();
  let at = -1;
  for (const t of terms) {
    const i = lower.search(new RegExp(`\\b${t.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")}`));
    if (i !== -1 && (at === -1 || i < at)) at = i;
  }
  if (at === -1) return markup(body.length > 150 ? body.slice(0, 150) + "…" : body, terms);
  // Start at the matching sentence when it's close, else on a word boundary.
  const sentence = Math.max(body.lastIndexOf(". ", at), body.lastIndexOf(": ", at));
  let start = sentence !== -1 && at - sentence < 90 ? sentence + 2 : Math.max(0, at - 40);
  if (start > 0 && start !== sentence + 2) {
    const sp = body.indexOf(" ", start);
    start = sp !== -1 && sp < at ? sp + 1 : start;
  }
  const end = Math.min(body.length, start + 160);
  const s = (start > 0 ? "…" : "") + body.slice(start, end).trimStart() + (end < body.length ? "…" : "");
  return markup(s, terms);
}

function search(req: SearchRequest): SearchResponse {
  const { f, chips } = parse(req.query);
  const terms = [...f.words, ...f.phrases, ...f.subject];
  const empty = !req.query.trim();
  if ((f.is.includes("event") || f.in.includes("calendar")) && !f.eventsOnly) f.eventsOnly = true;
  // The mock has no calendar: events-only searches find no mail.
  const byWords = empty || f.eventsOnly ? [] : MSGS.filter((m) => msgMatches(m, f, req.accountId, req.accountIds));
  // Search by meaning (semantic.ts): messages past the filters whose passage
  // covers the query's ideas, among those embedded so far.
  const meaning = new Map<MockMsg, MeaningMatch>();
  const free = [...f.words, ...f.phrases];
  if (!empty && !f.eventsOnly && free.length) {
    for (const m of MSGS) {
      if (!embedded(m.id)) continue;
      const mm = meaningMatch(free, m.subject, m.body);
      if (mm && msgMatches(m, f, req.accountId, req.accountIds, false)) meaning.set(m, mm);
    }
  }
  const wordSet = new Set(byWords);
  const matched = [...byWords, ...[...meaning.keys()].filter((m) => !wordSet.has(m))];

  // Collapse to threads: best (latest matching) message + match count.
  const byThread = new Map<string, MockMsg[]>();
  for (const m of matched) {
    const k = `${m.accountId}/${m.threadId}`;
    byThread.set(k, [...(byThread.get(k) ?? []), m]);
  }
  const hits: SearchHit[] = [...byThread.values()].map((ms) => {
    const scored = ms.map((m) => ({
      m,
      s: (meaning.get(m)?.strength ?? 0) * (wordSet.has(m) ? 1.5 : 2.5) + (wordSet.has(m) ? 0 : -1) + terms.reduce((acc, t) => acc + (termMatches(m.subject, t) ? 3 : 0) + (termMatches(m.body, t) ? 2 : 0) + ((m.attachments ?? []).some((a) => termMatches(`${a.filename} ${a.text ?? ""}`, t)) ? 2 : 0), 0) + (m.attachments?.length ? 0.5 : 0),
    }));
    scored.sort((a, b) => b.s - a.s || b.m.date.localeCompare(a.m.date));
    const best = scored[0].m;
    const byW = ms.some((m) => wordSet.has(m));
    const mm = meaning.get(best) ?? ms.map((m) => meaning.get(m)).find(Boolean) ?? null;
    const ageDays = (NOW.getTime() - new Date(best.date).getTime()) / 86_400_000;
    return {
      accountId: best.accountId,
      threadId: best.threadId,
      messageId: realMessageId(best.accountId, best.threadId, best.id),
      subject: best.subject.replace(/^(re|fwd?):\s*/i, ""),
      from: best.from,
      date: new Date(best.date).getTime(),
      snippetHtml: snippet(best.body, terms),
      matchCount: ms.length,
      labelIds: best.labelIds,
      hasAttachments: !!best.attachments?.length,
      unread: !!best.unread,
      score: scored[0].s + ms.length * (byW ? 0.8 : 0.3) + (best.starred ? 1 : 0) - ageDays / 400,
      matchedBy: [...(byW ? (["words"] as const) : []), ...(mm ? (["meaning"] as const) : [])],
      passage: mm?.passage ?? null,
    };
  });
  hits.sort((a, b) => b.score - a.score);

  const attachments: AttachmentHit[] = [];
  for (const m of byWords) {
    for (const a of m.attachments ?? []) {
      const byWord = terms.length > 0 && terms.some((t) => termMatches(`${a.filename} ${a.text ?? ""}`, t));
      const byHas = f.has.length > 0 && f.has.every((v) => hasKind(a, v));
      if (!byWord && !byHas) continue;
      const { text: _t, ...meta } = a;
      attachments.push({ accountId: m.accountId, threadId: m.threadId, messageId: realMessageId(m.accountId, m.threadId, m.id), attachment: meta, from: m.from, date: new Date(m.date).getTime() });
    }
  }
  attachments.sort((a, b) => {
    const wa = terms.some((t) => termMatches(a.attachment.filename, t)) ? 1 : 0;
    const wb = terms.some((t) => termMatches(b.attachment.filename, t)) ? 1 : 0;
    return wb - wa || b.date - a.date;
  });

  // People: whoever the from:/to: values or bare words name.
  const needles = [...f.from, ...f.to, ...f.words];
  const people: PersonHit[] = empty
    ? []
    : MOCK_PEOPLE.filter((p) => needles.some((n) => matchPerson(p, n)))
        .map((p) => ({ address: p, messageCount: MSGS.filter((m) => m.from.email === p.email || m.to.some((t) => t.email === p.email)).length * 4 + 3 }))
        .sort((a, b) => b.messageCount - a.messageCount)
        .slice(0, 4);

  return {
    chips,
    hits: hits.slice(0, req.limit || 50),
    attachments: attachments.slice(0, 9),
    people,
    tookMs: 20 + Math.round(Math.random() * 25),
    indexedMessages: 142_318,
    ...semanticState(),
  };
}

// ---------------------------------------------------------------------------
// Setup + accounts (onboarding)
// ---------------------------------------------------------------------------

function flag(key: string): boolean {
  try {
    // ?mock=unconfigured (or addFails) works too, for headless screenshots.
    const param = new URLSearchParams(window.location.search).get("mock");
    if (param && key === `penguin.mock.${param}`) return true;
    return localStorage.getItem(key) === "1";
  } catch {
    return false;
  }
}

const unconfiguredAtStart = flag("penguin.mock.unconfigured");
let oauthConfigured = !unconfiguredAtStart;
const OAUTH_PATH = "~/Library/Application Support/co.gluska.penguin/google-oauth-client.json";
let iosClientId: string | null = null;

/** Mirrors set_ios_oauth_client's input handling: bare id, .plist or JSON. */
function parseIosClientId(input: string): string {
  const t = input.trim();
  let id = t;
  const plist = /<key>CLIENT_ID<\/key>\s*<string>([^<]+)<\/string>/.exec(t);
  if (plist) id = plist[1].trim();
  else if (t.startsWith("{")) {
    let obj: Record<string, unknown>;
    try {
      obj = JSON.parse(t);
    } catch {
      reject("invalidInput", "That isn't valid JSON. Paste the iOS client ID instead.");
    }
    const inner = (obj.installed ?? obj.web ?? obj) as { client_id?: unknown; client_secret?: unknown };
    if (inner.client_secret) reject("invalidInput", "This client has a secret, so it's a Desktop app client. Create an iOS client and paste its ID.");
    id = typeof inner.client_id === "string" ? inner.client_id.trim() : "";
  }
  if (!/^[0-9]+-[a-z0-9]+\.apps\.googleusercontent\.com$/.test(id)) {
    reject("invalidInput", "That doesn't look like a Google client ID. It ends in .apps.googleusercontent.com.");
  }
  return id;
}

/** Mirrors the backend's CommandError rejections ({ code, message }). */
function reject(code: CommandErrorCode, message: string): never {
  throw { code, message } satisfies CommandError;
}

// The inbox mock owns the account list (mockMail.accounts() is what
// list_accounts serves). In unconfigured mode it starts empty and add_account
// signs its accounts back in one by one.
const CANDIDATES: Account[] = [...mockMail.accounts()];
if (unconfiguredAtStart) mockMail.accounts().splice(0);

function simulateBackfill(acc: Account) {
  const total = [48_210, 31_877, 62_231][mockMail.accounts().length % 3];
  let indexed = 0;
  const step = () => {
    mockMail.setSync({
      accountId: acc.id,
      phase: indexed >= total ? "incremental" : "backfilling",
      indexed: Math.min(indexed, total),
      totalEstimate: total,
      lastSyncedAt: indexed >= total ? Date.now() : null,
      error: null,
      ratePerMin: indexed >= total ? null : Math.round((total / 24) * 4 * 60),
      etaSecs: indexed >= total ? null : Math.ceil(((total - indexed) / (total / 24)) * 0.25),
    });
    if (indexed < total) {
      indexed += Math.round(total / 24);
      setTimeout(step, 250);
    }
  };
  step();
}

function validateClientJson(json: string): void {
  let parsed: unknown;
  try {
    parsed = JSON.parse(json);
  } catch {
    reject("invalidInput", "That isn't valid JSON. Paste the whole file you downloaded from Google Cloud.");
  }
  const inst = (parsed as { installed?: { client_id?: unknown } })?.installed;
  if (!inst || typeof inst.client_id !== "string") {
    reject("invalidInput", 'Expected a "Desktop app" client: the JSON should have an "installed" object with a client_id.');
  }
}

function searchThread(accountId: string, threadId: string): ThreadView | null {
  const msgs = MSGS.filter((m) => m.accountId === accountId && m.threadId === threadId).sort((a, b) => a.date.localeCompare(b.date));
  if (!msgs.length) return null;
  const view = (m: MockMsg): MessageView => ({
    accountId,
    id: m.id,
    threadId,
    date: new Date(m.date).getTime(),
    from: m.from,
    to: m.to,
    cc: [],
    bcc: [],
    replyTo: [],
    subject: m.subject,
    snippet: m.body.slice(0, 120),
    bodyText: m.body,
    html: `<!doctype html><html><head><meta charset="utf-8"><style>:root{color-scheme:light dark}body{margin:0;font:14px/22px Inter,system-ui,sans-serif;color:CanvasText}</style></head><body><p>${esc(m.body)}</p><p>${esc(m.from.name ?? "")}</p></body></html>`,
    blockedRemoteImages: 0,
    trackersRemoved: 0,
    trackers: [],
    labelIds: m.labelIds,
    attachments: (m.attachments ?? []).map(({ text: _t, ...a }) => a),
    unread: !!m.unread,
    starred: !!m.starred,
  });
  return { accountId, threadId, subject: msgs[0].subject.replace(/^re:\s*/i, ""), labelIds: [...new Set(msgs.flatMap((m) => m.labelIds))], messages: msgs.map(view) };
}

// Threads that exist only in the search world (the lease threads) become real
// threads in the inbox mock, so opening a hit lands on them.
for (const k of new Set(MSGS.map((m) => `${m.accountId}\u0000${m.threadId}`))) {
  const [accountId, threadId] = k.split("\u0000");
  if (!mockMail.thread(accountId, threadId)) {
    const view = searchThread(accountId, threadId);
    if (view) mockMail.addThread(view);
  }
}

/** For threads the inbox mock already had, point hits at its message ids. */
function realMessageId(accountId: string, threadId: string, id: string): string {
  const t = mockMail.thread(accountId, threadId);
  if (!t || t.messages.some((m) => m.id === id)) return id;
  return t.messages[t.messages.length - 1]?.id ?? id;
}

// ---------------------------------------------------------------------------
// Drafts (save_draft / delete_draft / get_draft, send_message with a draftId).
// Each saved draft is one DRAFT-labeled message in the inbox mock, so it shows
// in the Drafts view and inside the thread it replies to.
// ---------------------------------------------------------------------------

interface SavedDraft {
  draftId: string;
  accountId: string;
  threadId: string;
  messageId: string;
  draft: Draft;
  /** The saved message's attachments (ids re-minted on every save, like Gmail). */
  atts: AttachmentMeta[];
}

/** Gmail refs to a saved draft's attachments. */
function refs(saved: SavedDraft): OutgoingAttachment[] {
  return saved.atts.map((a) => ({
    kind: "gmail",
    messageId: saved.messageId,
    attachmentId: a.id,
    filename: a.filename,
    mimeType: a.mimeType,
    size: a.size,
    ...(a.contentId ? { contentId: a.contentId } : {}),
  }));
}

function attachmentMetas(d: Draft): AttachmentMeta[] {
  return (d.attachments ?? []).map((a) => ({
    id: `att-${++draftSeq}`,
    filename: a.filename,
    mimeType: a.mimeType,
    size: a.kind === "gmail" ? a.size : Math.floor((a.dataBase64.replace(/\s/g, "").length * 3) / 4),
    // (toDraft only sends inline images the HTML shows, as the backend's plan would keep them.)
    contentId: a.contentId ?? null,
    inline: !!a.contentId,
  }));
}

const DRAFTS = new Map<string, SavedDraft>();
let draftSeq = 0;

function draftMessage(saved: SavedDraft, d: Draft): MessageView {
  const from = { name: "Sam Okafor", email: mockMail.accounts().find((a) => a.id === d.accountId)?.email ?? d.accountId };
  return {
    accountId: d.accountId,
    id: saved.messageId,
    threadId: saved.threadId,
    date: Date.now(),
    from,
    to: d.to,
    cc: d.cc,
    bcc: d.bcc,
    replyTo: [],
    subject: d.subject || "(no subject)",
    snippet: d.bodyText.slice(0, 120),
    bodyText: d.bodyText,
    html: `<!doctype html><html><head><meta charset="utf-8"><style>:root{color-scheme:light dark}body{margin:0;font:14px/22px Inter,system-ui,sans-serif;color:CanvasText}p{margin:0 0 10px}</style></head><body>${d.bodyText
      .split(/\n{2,}/)
      .map((p) => `<p>${esc(p).replace(/\n/g, "<br>")}</p>`)
      .join("")}</body></html>`,
    blockedRemoteImages: 0,
    trackersRemoved: 0,
    trackers: [],
    labelIds: ["DRAFT"],
    attachments: saved.atts,
    unread: false,
    starred: false,
  };
}

/** Replace the thread with `msg` added (the inbox mock's addThread rebuilds it). */
function putMessage(accountId: string, threadId: string, msg: MessageView) {
  const cur = mockMail.thread(accountId, threadId);
  const messages = [...(cur?.messages ?? []).filter((m) => m.id !== msg.id), msg];
  mockMail.addThread({ accountId, threadId, subject: cur?.subject ?? msg.subject, labelIds: [], messages });
  mockBackend.emit("penguin://mail-changed", { accountId, threadIds: [threadId] });
}

function dropDraftMessage(saved: SavedDraft) {
  const cur = mockMail.thread(saved.accountId, saved.threadId);
  if (!cur) return;
  const rest = cur.messages.filter((m) => m.id !== saved.messageId);
  if (rest.length) mockMail.addThread({ ...cur, labelIds: [], messages: rest });
  else removeMockThread(saved.accountId, saved.threadId);
  mockBackend.emit("penguin://mail-changed", { accountId: saved.accountId, threadIds: [saved.threadId] });
}

/** The inbox mock ships a few DRAFT messages of its own; give them draft ids on first open. */
function adoptMailDraft(accountId: string, messageId: string | null): SavedDraft | null {
  if (!messageId) return null;
  for (const t of mockMail.threads()) {
    if (t.accountId !== accountId) continue;
    const view = mockMail.thread(t.accountId, t.threadId);
    const m = view?.messages.find((x) => x.id === messageId && x.labelIds.includes("DRAFT"));
    if (!view || !m) continue;
    const parent = [...view.messages].reverse().find((x) => x.id !== m.id && !x.labelIds.includes("DRAFT"));
    const saved: SavedDraft = {
      draftId: `draft-${++draftSeq}`,
      accountId,
      threadId: view.threadId,
      messageId: m.id,
      draft: {
        accountId,
        to: m.to,
        cc: m.cc,
        bcc: m.bcc,
        subject: m.subject,
        bodyText: m.bodyText,
        bodyHtml: null,
        replyToThreadId: parent ? view.threadId : null,
        replyToMessageId: parent?.id ?? null,
      },
      atts: m.attachments.filter((a) => !a.inline),
    };
    DRAFTS.set(saved.draftId, saved);
    return saved;
  }
  return null;
}

// ---------------------------------------------------------------------------
// Send later + reminders: plain timers standing in for the app's scheduler.
// ---------------------------------------------------------------------------

const SCHEDULES = new Map<string, ScheduledSend>();
const REMINDERS = new Map<string, Reminder>();
const TIMERS = new Map<string, ReturnType<typeof setTimeout>>();

function sendSaved(saved: SavedDraft, d: Draft): { messageId: string; threadId: string } {
  dropDraftMessage(saved);
  DRAFTS.delete(saved.draftId);
  const messageId = `sent-${++draftSeq}`;
  putMessage(saved.accountId, saved.threadId, { ...draftMessage(saved, d), id: messageId, labelIds: ["SENT"], attachments: saved.atts });
  return { messageId, threadId: saved.threadId };
}

function cancelSchedule(id: string): boolean {
  clearTimeout(TIMERS.get(id));
  TIMERS.delete(id);
  return SCHEDULES.delete(id);
}

function cancelReminder(id: string): boolean {
  clearTimeout(TIMERS.get(id));
  TIMERS.delete(id);
  return REMINDERS.delete(id);
}

function fireSchedule(id: string) {
  const sc = SCHEDULES.get(id);
  if (!sc) return;
  cancelSchedule(id);
  const saved = DRAFTS.get(sc.draftId);
  if (!saved) {
    const failed = [{ id, accountId: sc.accountId, draftId: sc.draftId, message: "the draft no longer exists" }];
    mockBackend.emit("penguin://scheduled-sent", { sent: [], failed, missed: 0 } satisfies ScheduledSentBatch);
    return;
  }
  const sent = sendSaved(saved, saved.draft);
  mockBackend.emit("penguin://scheduled-sent", {
    sent: [{ id, accountId: sc.accountId, draftId: sc.draftId, ...sent }],
    failed: [],
    missed: 0,
  } satisfies ScheduledSentBatch);
  if (sc.remindAfterMs) armReminder(sc.accountId, sent.threadId, sent.messageId, Date.now() + sc.remindAfterMs);
}

function armReminder(accountId: string, threadId: string, sentMessageId: string | null, remindAt: number): Reminder {
  const since = Date.now();
  const r: Reminder = { id: `rem-${++draftSeq}`, accountId, threadId, sentMessageId, sentAt: since, remindAt, createdAt: since };
  REMINDERS.set(r.id, r);
  TIMERS.set(
    r.id,
    setTimeout(() => {
      cancelReminder(r.id);
      const t = mockMail.thread(accountId, threadId);
      const replied = t?.messages.some((m) => m.date > since && !m.labelIds.includes("SENT") && !m.labelIds.includes("DRAFT") && !m.labelIds.includes("SPAM"));
      if (replied) return;
      void mailHandlers.modify_threads({ targets: [{ accountId, threadId }], action: { kind: "moveToInbox" } });
      void mailHandlers.modify_threads({ targets: [{ accountId, threadId }], action: { kind: "markUnread" } });
      mockBackend.emit("penguin://reminder-due", { accountId, threadId, subject: t?.subject ?? "" } satisfies ReminderDue);
    }, Math.max(0, remindAt - Date.now())),
  );
  return r;
}

export const searchHandlers: Record<string, MockHandler> = {
  search: ({ request }) => search(request as SearchRequest),
  send_message: ({ draft, draftId }) => {
    const d = draft as Draft;
    if (!d?.to?.length && !d?.cc?.length && !d?.bcc?.length) reject("invalidInput", "Add at least one recipient.");
    if (draftId) {
      const saved = DRAFTS.get(draftId as string);
      if (!saved) reject("notFound", "That draft no longer exists.");
      const sent = sendSaved(saved, d);
      for (const sc of SCHEDULES.values()) if (sc.draftId === draftId) cancelSchedule(sc.id);
      return sent;
    }
    return { messageId: `sent-${++draftSeq}`, threadId: d.replyToThreadId ?? `t-sent-${draftSeq}` };
  },
  schedule_send: ({ accountId, draftId, sendAt, remindAfterMs }) => {
    if (!DRAFTS.has(draftId as string)) reject("notFound", "Save the draft before scheduling it.");
    for (const sc of SCHEDULES.values()) if (sc.draftId === draftId) cancelSchedule(sc.id);
    const sc: ScheduledSend = {
      id: `sch-${++draftSeq}`,
      accountId: accountId as string,
      draftId: draftId as string,
      sendAt: Number(sendAt),
      createdAt: Date.now(),
      remindAfterMs: (remindAfterMs as number | null) ?? null,
      attempts: 0,
      lastError: null,
    };
    SCHEDULES.set(sc.id, sc);
    TIMERS.set(sc.id, setTimeout(() => fireSchedule(sc.id), Math.max(0, sc.sendAt - Date.now())));
    return sc;
  },
  cancel_scheduled_send: ({ id }) => cancelSchedule(id as string),
  list_scheduled_sends: ({ accountId }) =>
    [...SCHEDULES.values()].filter((x) => !accountId || x.accountId === accountId).sort((a, b) => a.sendAt - b.sendAt),
  set_reminder: ({ accountId, threadId, sentMessageId, remindAt }) => {
    for (const r of REMINDERS.values()) if (r.accountId === accountId && r.threadId === threadId) cancelReminder(r.id);
    return armReminder(accountId as string, threadId as string, (sentMessageId as string | null) ?? null, Number(remindAt));
  },
  cancel_reminder: ({ id }) => cancelReminder(id as string),
  list_reminders: ({ accountId }) =>
    [...REMINDERS.values()].filter((x) => !accountId || x.accountId === accountId).sort((a, b) => a.remindAt - b.remindAt),
  save_draft: ({ draft, draftId }) => {
    if (flag("penguin.mock.draftsOffline")) reject("network", "Couldn't reach Gmail (mock offline).");
    const d = draft as Draft;
    // Like the backend: an unknown draftId (deleted or sent elsewhere) is
    // recreated under a new id.
    const prev = draftId ? DRAFTS.get(draftId as string) : undefined;
    if (prev && prev.accountId !== d.accountId) reject("invalidInput", "A draft can't move between accounts; delete it and save a new one.");
    if (prev) dropDraftMessage(prev);
    const saved: SavedDraft = {
      draftId: prev?.draftId ?? `draft-${++draftSeq}`,
      accountId: d.accountId,
      threadId: prev?.threadId ?? d.replyToThreadId ?? `t-draft-${draftSeq}`,
      messageId: `draft-msg-${++draftSeq}`, // changes on every save, like Gmail
      draft: { ...d, attachments: [] },
      atts: attachmentMetas(d),
    };
    DRAFTS.set(saved.draftId, saved);
    putMessage(saved.accountId, saved.threadId, draftMessage(saved, d));
    return { draftId: saved.draftId, messageId: saved.messageId, threadId: saved.threadId, attachments: refs(saved) } satisfies DraftRef;
  },
  delete_draft: ({ draftId }) => {
    const saved = DRAFTS.get(draftId as string);
    if (!saved) return undefined;
    dropDraftMessage(saved);
    DRAFTS.delete(saved.draftId);
    return undefined;
  },
  get_draft: ({ accountId, draftId, messageId }) => {
    const byId = draftId ? DRAFTS.get(draftId as string) : [...DRAFTS.values()].find((x) => x.messageId === messageId);
    const saved = byId ?? adoptMailDraft(accountId as string, messageId as string | null);
    if (!saved) return null;
    return {
      draftId: saved.draftId,
      messageId: saved.messageId,
      threadId: saved.threadId,
      // Like the backend: the stored HTML comes back through the composer allowlist.
      draft: {
        ...saved.draft,
        bodyHtml: saved.draft.bodyHtml
          ? (composeHandlers.sanitize_compose_html({ html: saved.draft.bodyHtml, cids: saved.atts.flatMap((a) => (a.contentId ? [a.contentId] : [])) }) as string)
          : null,
        attachments: refs(saved),
      },
    } satisfies OpenedDraft;
  },
  oauth_client_status: () => ({ configured: oauthConfigured, path: OAUTH_PATH, iosClientId }),
  set_oauth_client: ({ json }) => {
    validateClientJson(json as string);
    oauthConfigured = true;
    return { configured: true, path: OAUTH_PATH, iosClientId };
  },
  set_ios_oauth_client: ({ input }) => {
    iosClientId = parseIosClientId(input as string);
    return { configured: oauthConfigured, path: OAUTH_PATH, iosClientId };
  },
  clear_ios_oauth_client: () => {
    iosClientId = null;
    return { configured: oauthConfigured, path: OAUTH_PATH, iosClientId };
  },
  add_account: async ({ loginHint }) => {
    if (!oauthConfigured) reject("notConfigured", "The Google OAuth client isn't set up.");
    await mockBrowserSignIn(loginHint ?? null);
    if (flag("penguin.mock.addFails")) reject("other", "Google sign-in was cancelled in the browser.");
    const live = mockMail.accounts();
    const hint = typeof loginHint === "string" ? loginHint.toLowerCase() : null;
    // The typed address, like the real sign-in (else the next seeded account).
    const next: Account | undefined =
      hint && !live.some((a) => a.id === hint)
        ? { ...CANDIDATES[0], id: hint, email: hint, displayName: null, nickname: null, color: "#2E9E6B", addedAt: Date.now() }
        : CANDIDATES.find((c) => !live.some((a) => a.id === c.id));
    if (!next) reject("invalidInput", "All mock accounts are already added.");
    const acc: Account = { ...next!, addedAt: Date.now() };
    live.push(acc);
    setTimeout(() => simulateBackfill(acc), 50);
    return acc;
  },
};
