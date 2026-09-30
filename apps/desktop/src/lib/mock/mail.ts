// OWNER: inbox/thread agent. In-memory mock mailbox for `npm run dev:mock`.
// Fictional people and companies only (.example domains), mirroring the
// design mockups (design/01-inbox.html, 02-thread.html): three Gmail accounts
// (Northwind, Harbor Labs, Personal) plus an IMAP one with folders (Okafor,
// on Fastmail), the hand-written threads from the
// mockups, plus ~300 generated threads so virtualization and paging get
// exercised. Add `?mockThreads=12000` to the URL to stress-test the list.
//
// State is mutable: modify_threads changes labels and emits mail-changed,
// exactly like the real backend's optimistic path.
import type {
  Account,
  AttachmentHit,
  Address,
  AttachmentMeta,
  InboxTab,
  Label,
  ListQuery,
  MailboxView,
  MessageView,
  QuoteSource,
  SyncStatus,
  ThreadAction,
  ThreadRef,
  ThreadSummary,
  ThreadView,
} from "../types";
import { mockProviderFields } from "./accounts";
import { mockBackend, type MockHandler } from "./index";
import { reconcileSnooze, snoozeOf } from "./snoozeState";
import { mockBackoff, mockRecordFailure, mockRecordProgress, mockRetryNow } from "./syncFailures";
import { hideSyncAlert } from "../syncHides";
import { mockPrivacy } from "./trackers";
import { unreadLabels } from "../../app/optimistic";
import { mockReceipts } from "./receipts";
import { normalizeSmartViews } from "./smartSettings";
import { composeHandlers } from "./compose";

function mockFlag(key: string): boolean {
  try {
    const param = new URLSearchParams(window.location.search).get("mock");
    if (param && key === `penguin.mock.${param}`) return true;
    return localStorage.getItem(key) === "1";
  } catch {
    return false;
  }
}

// ---------------------------------------------------------------------------
// Deterministic randomness
// ---------------------------------------------------------------------------
function rng(seed: number) {
  let a = seed >>> 0;
  return () => {
    a = (a + 0x6d2b79f5) >>> 0;
    let t = a;
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
}
const pick = <T,>(r: () => number, xs: readonly T[]): T => xs[Math.floor(r() * xs.length)];

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;
const NOW = Date.now();

function startOfToday(): number {
  const d = new Date(NOW);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}
/** Today at h:m, clamped into the past (so "9:41 AM" works at any time of day). */
function todayAt(h: number, m: number, fallbackMinutesAgo: number): number {
  const t = startOfToday() + h * HOUR + m * MIN;
  return t < NOW ? t : Math.max(startOfToday() + MIN, NOW - fallbackMinutesAgo * MIN);
}
function daysAgoAt(days: number, h: number, m: number): number {
  return startOfToday() - days * DAY + h * HOUR + m * MIN;
}

// ---------------------------------------------------------------------------
// Accounts, people, labels
// ---------------------------------------------------------------------------
// fm is an IMAP account (the family domain on Fastmail, app password): no
// labels, folders instead ("f:<path>" user labels), no inbox categories, so
// Move to… replaces Label as… on its conversations.
const ACC = { nw: "acc-northwind", hl: "acc-harbor", pe: "acc-personal", fm: "acc-okafor" } as const;
type AccKey = keyof typeof ACC;

const FM_EMAIL = "sam@okafor.example";
const accounts: Account[] = [
  // displayName is the person's Google profile name, as the real backend reports
  // it. The UI derives the account label ("Northwind", "Harbor Labs",
  // "Personal") from the address (see accountLabel in lib/format.ts).
  { id: ACC.nw, email: "sam@northwind.example", displayName: "Sam Okafor", nickname: null, color: "#8d5ffe", addedAt: NOW - 400 * DAY, ...mockProviderFields() },
  { id: ACC.hl, email: "sam@harbor-labs.example", displayName: "Sam Okafor", nickname: null, color: "#2bd67b", addedAt: NOW - 300 * DAY, ...mockProviderFields() },
  { id: ACC.pe, email: "sam.okafor@gmail.example", displayName: "Sam Okafor", nickname: null, color: "#ff8b3d", addedAt: NOW - 200 * DAY, ...mockProviderFields() },
  {
    id: ACC.fm,
    email: FM_EMAIL,
    displayName: null,
    nickname: null,
    color: "#0090ff",
    addedAt: NOW - 30 * DAY,
    ...mockProviderFields("imap", {
      auth: "appPassword",
      host: "fastmail",
      imap: { host: "imap.fastmail.com", port: 993, security: "tls", username: FM_EMAIL },
      smtp: { host: "smtp.fastmail.com", port: 465, security: "tls", username: FM_EMAIL },
    }),
  },
];
const ME: Record<AccKey, Address> = {
  nw: { name: "Sam Okafor", email: "sam@northwind.example" },
  hl: { name: "Sam Okafor", email: "sam@harbor-labs.example" },
  pe: { name: "Sam Okafor", email: "sam.okafor@gmail.example" },
  fm: { name: "Sam Okafor", email: FM_EMAIL },
};
const accKeyOf = (id: string): AccKey => (Object.keys(ACC) as AccKey[]).find((k) => ACC[k] === id) ?? "nw";

const P = (name: string, email: string): Address => ({ name, email });
const people = {
  priya: P("Priya Natarajan", "priya@linden.example"),
  marco: P("Marco Bellini", "marco@northwind.example"),
  mike: P("Mike Delgado", "mike.delgado@mailbox.example"),
  dana: P("Dana Whitfield", "dana@northwind.example"),
  ledgerly: P("Ledgerly Billing", "billing@ledgerly.example"),
  theo: P("Theo Laurent", "theo@harborlabs.example"),
  grace: P("Grace Kim", "grace.kim@alderpoint.example"),
  ravi: P("Ravi Menon", "ravi@northwind.example"),
  jonas: P("Jonas Weber", "jonas@tidewater.example"),
  lena: P("Lena Park", "lena.park@mailbox.example"),
  hlcal: P("Harbor Labs Calendar", "calendar@harborlabs.example"),
  kofi: P("Kofi Mensah", "kofi@northwind.example"),
  northline: P("Northline Air", "no-reply@northline.example"),
  ana: P("Ana Sousa", "ana@harborlabs.example"),
  ines: P("Ines Carvalho", "ines@tidewater.example"),
  cedar: P("Cedar & Pine Property", "rent@cedarpine.example"),
};

interface LabelDef { acc: AccKey; id: string; name: string; color: string | null; hidden?: boolean }
const USER_LABELS: LabelDef[] = [
  { acc: "nw", id: "Label_clients", name: "Clients", color: "#4a86e8" },
  { acc: "hl", id: "Label_finance", name: "Finance", color: "#f2b64b" },
  { acc: "pe", id: "Label_travel", name: "Travel", color: "#2da2bb" },
  { acc: "hl", id: "Label_receipts", name: "Receipts", color: null, hidden: true },
  { acc: "pe", id: "Label_home", name: "Home", color: "#e66550" },
  // IMAP folders: ids "f:<percent-encoded path>", names with "/" for nesting, no colors.
  { acc: "fm", id: "f:Family", name: "Family", color: null },
  { acc: "fm", id: "f:Receipts", name: "Receipts", color: null },
  { acc: "fm", id: "f:Projects%2FGarden", name: "Projects/Garden", color: null },
];
const FM_FOLDERS = ["f:Family", "f:Receipts", "f:Projects%2FGarden"];
const SYSTEM_LABELS = ["INBOX", "STARRED", "IMPORTANT", "SENT", "DRAFT", "TRASH", "SPAM", "UNREAD"];

// ---------------------------------------------------------------------------
// Thread model
// ---------------------------------------------------------------------------
interface MsgSpec {
  from: Address;
  to: Address[];
  cc?: Address[];
  daysBefore?: number; // relative to thread lastDate (for generated)
  date: number;
  paragraphs: string[];
  list?: string[];
  signoff?: string;
  sig?: string;
  attachments?: AttachmentMeta[];
  newsletter?: boolean;
  /** Bulk mail that isn't a newsletter (spam): this many blocked pictures and pixels, and a List-Unsubscribe header in its details. */
  bulk?: { images: number; trackers: number };
  /** A reply that quotes the message before it, the way Gmail's plain part does (Copy cuts it). */
  quotesPrevious?: boolean;
}

interface MockThread {
  summary: ThreadSummary;
  /** Lazily generated for bulk threads. */
  messages: MsgSpec[] | null;
  /** Ready-made messages added by another mock module (see mockMail.addThread). */
  views?: MessageView[];
  seed: number;
  kind: "person" | "newsletter" | "notification";
}

const threads = new Map<string, MockThread>(); // key = acc:thread
const key = (accountId: string, threadId: string) => `${accountId}:${threadId}`;

let attSeq = 0;
function att(filename: string, size: number, mimeType: string): AttachmentMeta {
  return { id: `att-${++attSeq}`, filename, size, mimeType, contentId: null, inline: false };
}
const MB = 1024 * 1024;

// ---------------------------------------------------------------------------
// Hand-written threads from the mockups
// ---------------------------------------------------------------------------
interface Handmade {
  acc: AccKey;
  id: string;
  subject: string;
  labels: string[];
  messages: MsgSpec[];
  participants?: Address[];
  kind?: MockThread["kind"];
}

function handmade(): Handmade[] {
  const me = ME;
  return [
    {
      acc: "nw",
      id: "t-priya-q4",
      subject: "Q4 brand refresh — final review deck",
      labels: ["INBOX", "UNREAD", "IMPORTANT", "Label_clients"],
      participants: [people.priya, people.marco, me.nw],
      messages: [
        {
          from: me.nw, to: [people.priya], cc: [people.marco], date: daysAgoAt(8, 9, 2),
          paragraphs: [
            "Hi Priya,",
            "Kicking off the Q4 refresh review. Brief and timeline are in the shared folder — first look Sep 17 if that still works for your team.",
            "Marco will be our point person on product surfaces; I'll own sign-off on the wordmark and palette.",
          ],
          signoff: "Thanks,\nSam",
        },
        {
          from: people.priya, to: [me.nw], cc: [people.marco], date: daysAgoAt(6, 16, 20),
          paragraphs: [
            "Hi Sam,",
            "Round two directions are attached. We pushed the wordmark further in B and kept A as the conservative option.",
            "Would love a gut reaction from both of you before Friday.",
          ],
          signoff: "Priya",
          attachments: [att("Direction_Round_2.pdf", 8.2 * MB, "application/pdf")],
        },
        {
          from: people.marco, to: [people.priya], cc: [me.nw], date: daysAgoAt(2, 11, 5),
          paragraphs: [
            "Direction B for me. The wordmark spacing still feels a touch loose at small sizes, otherwise it's there.",
            "Can we see it at 16px in the nav before we lock?",
          ],
          signoff: "M",
        },
        {
          from: people.priya, to: [me.nw], cc: [people.marco], date: todayAt(9, 41, 12),
          paragraphs: [
            "Hi Sam,",
            "Attached is v7 of the review deck with Tuesday's lockups. Everything from the last round is in, including Marco's wordmark spacing note.",
            "Two open questions before we lock it:",
          ],
          list: [
            "Keep the warm gray in the secondary palette, or drop it for the cooler neutral on slide 14?",
            "Can we move the stakeholder review to Monday the 28th? Thursday is tight on our side.",
          ],
          signoff: "Happy to walk through it live — I'm free after 2 PM your time.\nPriya",
          sig: "Design Director · Linden & Co",
          attachments: [
            att("Northwind_Brand_Review_v7.pdf", 12.4 * MB, "application/pdf"),
            att("Logo_Lockups_v3.png", 2.1 * MB, "image/png"),
            att("Palette_Options.key", 2.4 * MB, "application/x-iwork-keynote-sffkey"),
          ],
        },
      ],
    },
    {
      acc: "pe", id: "t-water-heater", subject: "Re: Water heater service window",
      labels: ["INBOX", "UNREAD", "IMPORTANT", "Label_home"],
      messages: [
        { from: me.pe, to: [people.mike], date: daysAgoAt(1, 19, 30), paragraphs: ["Hey Mike — any word from the plumber on the water heater? Thursday or Friday both work for me."], signoff: "Sam" },
        { from: people.mike, to: [me.pe], date: todayAt(9, 12, 30), paragraphs: ["Plumber confirmed Thursday between 10 and 12. I'll leave the side gate unlocked so they can get to the utility closet.", "They said the anode rod is probably the culprit, so it should be a quick one."], signoff: "Mike", quotesPrevious: true },
      ],
    },
    {
      // Files on the first message, then a short reply of mine: forwarding
      // the conversation must still bring the files.
      acc: "nw", id: "t-scope", subject: "Scope of work v3 (re-timed) + report",
      labels: ["INBOX"],
      messages: [
        {
          from: people.dana, to: [me.nw], date: daysAgoAt(1, 15, 10),
          paragraphs: ["Hi Sam,", "Scope of work v3 is attached, with the milestones re-timed to the new kickoff date and the discovery report."],
          list: ["Discovery moves to weeks 1–3.", "The report now covers both pilot sites."],
          signoff: "Dana",
          attachments: [att("Scope_of_work_v3.pdf", 1.6 * MB, "application/pdf"), att("Discovery_report.docx", 420_000, "application/vnd.openxmlformats-officedocument.wordprocessingml.document")],
        },
        { from: me.nw, to: [people.dana], date: todayAt(8, 5, 0), paragraphs: ["Thanks Dana, this looks right. I'll pass it on to finance."], signoff: "Sam" },
      ],
    },
    {
      // Older than the sync window: stored headers-only until opened.
      acc: "nw", id: "t-lease-2025", subject: "Office lease addendum, signed",
      labels: [],
      messages: [
        { from: people.grace, to: [me.nw], date: daysAgoAt(240, 11, 30), paragraphs: ["Signed addendum attached for your files."], signoff: "Grace", attachments: [att("Lease_addendum_signed.pdf", 640_000, "application/pdf")] },
      ],
    },
    {
      acc: "nw", id: "t-offsite", subject: "Offsite agenda: Oct 14–15",
      labels: ["INBOX", "UNREAD", "IMPORTANT"],
      messages: [
        { from: people.dana, to: [me.nw], date: daysAgoAt(1, 10, 0), paragraphs: ["Starting the offsite agenda. Rough shape: strategy Tuesday morning, team breakouts in the afternoon, retro and planning on Wednesday."], signoff: "Dana" },
        { from: people.dana, to: [me.nw], cc: [people.ravi, people.kofi], date: todayAt(8, 59, 40), paragraphs: ["Draft agenda is in the doc. Can you own the Friday retro block? Need an answer by Wednesday so I can lock the room bookings.", "I kept the two-hour lunch — people asked for it last time."], signoff: "Dana" },
      ],
    },
    {
      acc: "hl", id: "t-ledgerly", subject: "Invoice HL-4821 paid — $1,240.00",
      labels: ["INBOX", "IMPORTANT", "Label_receipts"], kind: "notification",
      messages: [
        { from: people.ledgerly, to: [me.hl], date: todayAt(8, 57, 55), paragraphs: ["Thanks! Your payment to Harbor Labs was received. Receipt attached for your records.", "Amount: $1,240.00 · Paid with Visa ending 4412 · Invoice HL-4821"], signoff: "— Ledgerly", attachments: [att("Receipt_HL-4821.pdf", 184_000, "application/pdf")] },
      ],
    },
    {
      acc: "hl", id: "t-beta-signups", subject: "Beta: 40 new signups overnight",
      labels: ["INBOX", "UNREAD", "IMPORTANT"],
      messages: [
        { from: people.theo, to: [me.hl], cc: [people.ana], date: todayAt(7, 58, 70), paragraphs: ["Mostly from the changelog post. Week-one retention cohort is holding at 62%, which is the best we've seen since launch.", "Onboarding drop-off is still at the workspace-invite step. I'd like to try skipping it for solo users this week."], signoff: "Theo", attachments: [att("signups-overnight.csv", 3_904, "text/csv"), att("cohort-export.zip", 1.8 * MB, "application/zip")] },
      ],
    },
    {
      acc: "nw", id: "t-contract", subject: "Contract countersigned",
      labels: ["INBOX", "IMPORTANT", "Label_clients"],
      messages: [
        { from: people.grace, to: [me.nw], date: daysAgoAt(1, 18, 12), paragraphs: ["All set on our side. Scanned copy attached; originals go out by courier Friday.", "Looking forward to kicking this off."], signoff: "Grace Kim\nAlderpoint Partners", attachments: [att("Alderpoint_MSA_countersigned.pdf", 1.3 * MB, "application/pdf")] },
      ],
    },
    {
      acc: "nw", id: "t-design-crit", subject: "Design crit notes — onboarding flow",
      labels: ["INBOX", "IMPORTANT"],
      messages: [
        { from: people.ravi, to: [me.nw], date: daysAgoAt(2, 15, 0), paragraphs: ["Sharing the Figma link for tomorrow's crit. Focus is the new onboarding flow."], signoff: "Ravi" },
        { from: me.nw, to: [people.ravi], date: daysAgoAt(2, 17, 10), paragraphs: ["Thanks — I'll add a few comments tonight."], signoff: "Sam" },
        { from: people.dana, to: [people.ravi, me.nw], date: daysAgoAt(1, 11, 30), paragraphs: ["Adding one more to the list: the empty state copy on step two reads a bit cold."], signoff: "D" },
        { from: people.ravi, to: [me.nw], cc: [people.dana], date: daysAgoAt(1, 16, 40), paragraphs: ["Cleaned up my notes from today. Biggest theme: step three asks for too much. We should defer the team invite and the calendar connection until after the first project exists.", "Smaller things:"], list: ["Progress indicator is doing too much work — drop the step labels.", "The skip link needs to be a real button.", "Copy on the final screen should say what happens next."], signoff: "Ravi", attachments: [att("onboarding-crit-notes.md", 2_140, "text/markdown"), att("step3-fields.csv", 612, "text/csv")] },
      ],
    },
    {
      acc: "hl", id: "t-intro-jonas", subject: "Intro: Jonas ↔ Sam (seed round)",
      labels: ["IMPORTANT", "STARRED", "INBOX"],
      messages: [
        { from: people.ines, to: [people.jonas, me.hl], date: daysAgoAt(1, 12, 30), paragraphs: ["Jonas, meet Sam — founder at Harbor Labs, the team behind the logistics beta I mentioned. Sam, Jonas leads early-stage at Tidewater.", "I'll let you two take it from here."], signoff: "Ines" },
        { from: people.jonas, to: [me.hl], date: daysAgoAt(1, 14, 5), paragraphs: ["Thanks Ines! Moving Ines to bcc.", "Sam, happy to share our data room whenever you're ready. Would a 30-minute call next week work?"], signoff: "Jonas" },
      ],
    },
    {
      acc: "pe", id: "t-lake-photos", subject: "Photos from the lake weekend",
      labels: ["INBOX", "IMPORTANT"],
      messages: [
        { from: people.lena, to: [me.pe], date: daysAgoAt(1, 11, 18), paragraphs: ["Finally uploaded everything. The sunrise ones from Sunday came out unreal — the fog on the water!", "Album link is below; download whatever you want."], signoff: "Lena" },
      ],
    },
    {
      acc: "hl", id: "t-weekly-sync", subject: "Invitation: Weekly sync",
      labels: ["INBOX", "IMPORTANT"], kind: "notification",
      messages: [
        { from: people.hlcal, to: [me.hl], date: daysAgoAt(3, 9, 0), paragraphs: ["Thu Sep 24, 10:00 – 10:30 AM · Theo Laurent, Ana Sousa, you", "Join with the room link in the calendar event."], attachments: [att("invite.ics", 2_400, "text/calendar")] },
      ],
    },
    {
      acc: "nw", id: "t-pricing-copy", subject: "Re: Pricing page copy",
      labels: ["INBOX", "IMPORTANT"],
      messages: Array.from({ length: 6 }, (_, i): MsgSpec => ({
        from: i % 2 === 0 ? people.kofi : ME.nw, to: [i % 2 === 0 ? ME.nw : people.kofi],
        date: daysAgoAt(3 + (5 - i) * 0.3, 10 + i, 5),
        paragraphs: [
          [
            "Two versions of the pricing copy are in the doc. A is closer to what we have today, B leads with the team plan.",
            "B feels right to me. Can you check the FAQ answers against the new limits?",
            "Updated the limits. The second FAQ answer is now redundant with the table.",
            "Agreed — let's cut it if it doesn't earn its place.",
            "Also rewrote the enterprise blurb, it was trying to say four things.",
            "Version B reads cleaner. I'd cut the second FAQ entirely and move the enterprise line under the table.",
          ][i],
        ],
        signoff: i % 2 === 0 ? "Kofi" : "Sam",
      })),
    },
    {
      acc: "pe", id: "t-lisbon", subject: "Your trip to Lisbon — boarding passes",
      labels: ["INBOX", "IMPORTANT", "Label_travel"], kind: "notification",
      messages: [
        { from: people.northline, to: [me.pe], date: daysAgoAt(4, 16, 0), paragraphs: ["Flight NL 214 departs Oct 9 at 7:25 PM. Seats 14A, 14B confirmed.", "Boarding begins 45 minutes before departure. Your boarding passes are attached."], attachments: [att("NL214_boarding_passes.pdf", 420_000, "application/pdf")] },
      ],
    },
    {
      acc: "nw", id: "t-podcast", subject: "Podcast booking — Oct 2 recording",
      labels: ["INBOX", "IMPORTANT"],
      messages: [
        { from: me.nw, to: [people.ana], date: daysAgoAt(5, 9, 30), paragraphs: ["Does Oct 2 still work for the recording? Any time after lunch is fine."], signoff: "Sam" },
        { from: people.ana, to: [me.nw], date: daysAgoAt(4, 13, 10), paragraphs: ["Locked in 2 PM. I'll send the outline and the mic kit by Friday.", "The host wants to start with the story of the first customer — have that one ready."], signoff: "Ana" },
      ],
    },
    {
      acc: "hl", id: "t-data-room", subject: "Data room access",
      labels: ["INBOX", "IMPORTANT", "Label_finance"],
      messages: [
        { from: people.ines, to: [me.hl], date: daysAgoAt(5, 17, 45), paragraphs: ["You should have view access now. Model is in the Finance folder, v3.", "Ping me if anything is missing before the Jonas call."], signoff: "Ines" },
      ],
    },
    {
      acc: "pe", id: "t-rent", subject: "October rent receipt",
      labels: ["INBOX", "IMPORTANT", "Label_home"], kind: "notification",
      messages: [
        { from: people.cedar, to: [me.pe], date: daysAgoAt(5, 8, 20), paragraphs: ["Payment of $2,450.00 for 418 Alder St, Unit 3B was received on Oct 1. Thank you!", "Your receipt is attached."], attachments: [att("Rent_Receipt_Oct.pdf", 96_000, "application/pdf")] },
      ],
    },
    // Spam (the Spam view, G then !): fictional senders on SPAM_DOMAINS, which fail DMARC in their details.
    {
      acc: "nw", id: "t-spam-prize", subject: "Congratulations! You've been selected for a $1,000 gift card",
      labels: ["SPAM", "UNREAD"], kind: "notification",
      messages: [
        {
          from: P("Prize Center", "claims@prize-center.example"), to: [me.nw], date: todayAt(6, 12, 200),
          paragraphs: ["Dear valued customer,", "Your email was drawn in this month's loyalty sweepstakes. Claim your $1,000 gift card before midnight by confirming your shipping details.", "This offer expires today."],
          bulk: { images: 4, trackers: 2 },
        },
      ],
    },
    {
      acc: "nw", id: "t-spam-verify", subject: "Action required: your mailbox will be suspended in 24 hours",
      labels: ["SPAM", "UNREAD"], kind: "notification",
      messages: [
        {
          from: P("Account Security", "no-reply@secure-verify.example"), to: [me.nw], date: daysAgoAt(1, 22, 40),
          paragraphs: ["We detected unusual sign-in activity on sam@northwind.example.", "To keep your mailbox active, verify your password within 24 hours. Unverified accounts are suspended automatically."],
        },
      ],
    },
    {
      acc: "hl", id: "t-spam-crypto", subject: "Turn $250 into $9,400 by Friday (only 12 spots left)",
      labels: ["SPAM", "UNREAD"], kind: "notification",
      messages: [
        {
          from: P("Vance Holloway", "vance@quantumyield.example"), to: [me.hl], date: daysAgoAt(2, 3, 15),
          paragraphs: ["Hi Sam,", "My trading signals group returned 3,660% last quarter. I'm opening 12 spots to new members this week, no experience needed.", "Reply YES and I'll send the link."],
          bulk: { images: 2, trackers: 1 },
        },
      ],
    },
    {
      acc: "pe", id: "t-spam-parcel", subject: "Your package is on hold: confirm the $1.99 delivery fee",
      labels: ["SPAM"], kind: "notification",
      messages: [
        {
          from: P("Parcel Desk", "notice@parcel-redelivery.example"), to: [me.pe], date: daysAgoAt(3, 11, 2),
          paragraphs: ["We attempted to deliver your package but the address was incomplete.", "Pay the $1.99 redelivery fee to schedule a new delivery date."],
        },
      ],
    },
    {
      acc: "fm", id: "t-spam-seo", subject: "Page 1 of search results in 30 days, guaranteed",
      labels: ["SPAM", "UNREAD"], kind: "notification",
      messages: [
        {
          from: P("Rank Boost Team", "outreach@rankboost-pro.example"), to: [me.fm], date: daysAgoAt(1, 6, 30),
          paragraphs: ["Hello,", "I noticed okafor.example isn't ranking for your main keywords. Our team can get you to page 1 in 30 days or your money back.", "Can I send over a free audit?"],
          bulk: { images: 1, trackers: 1 },
        },
      ],
    },
  ];
}

/** The mock's spam senders (their mail fails SPF and DMARC in Message details). */
const SPAM_DOMAINS = new Set(["prize-center.example", "secure-verify.example", "quantumyield.example", "parcel-redelivery.example", "rankboost-pro.example"]);

// ---------------------------------------------------------------------------
// Generated bulk threads
// ---------------------------------------------------------------------------
const GEN_PEOPLE: Record<AccKey, Address[]> = {
  nw: [
    people.priya, people.marco, people.dana, people.ravi, people.kofi,
    P("Hana Ito", "hana@northwind.example"), P("Owen Brooks", "owen@northwind.example"),
    P("Maya Lindqvist", "maya@linden.example"), P("Rhea Holt", "rhea@alderpoint.example"),
    P("Tomás Ruiz", "tomas@brightfield.example"), P("Elif Aydın", "elif@northwind.example"),
  ],
  hl: [
    people.theo, people.ana, people.ines, people.jonas,
    P("Omar Farouk", "omar@harborlabs.example"), P("Mira Bauer", "mira@quayside.example"),
    P("Felix Grant", "felix@harborlabs.example"), P("June Adeyemi", "june@portside.example"),
  ],
  pe: [
    people.mike, people.lena, P("Chris Okafor", "chris@okafor.example"), P("Nadia Rahman", "nadia@mailbox.example"),
    P("Ben Walsh", "ben.walsh@mailbox.example"), P("Aunt Jo", "jo@okafor.example"),
  ],
  fm: [
    P("Chris Okafor", "chris@okafor.example"), P("Aunt Jo", "jo@okafor.example"), P("Greenleaf Nursery", "orders@greenleaf.example"),
    P("Maple Street School", "office@maplestreet.example"), P("Rosa Lindgren", "rosa@allotment.example"),
  ],
};

const TOPICS: { subject: string; paragraphs: string[] }[] = [
  { subject: "Quick sync on the roadmap", paragraphs: ["Do you have 20 minutes this week to walk through the roadmap? I want to sanity-check the order of the next three bets before planning.", "Mostly worried we're front-loading the migration."] },
  { subject: "Feedback on the proposal", paragraphs: ["Read through the proposal twice. The problem framing is strong; the rollout section needs a clearer owner per phase.", "I left inline comments on the timeline."] },
  { subject: "Hiring loop for the senior designer role", paragraphs: ["We have four candidates through the portfolio round. Can you take the craft interview for two of them on Thursday?", "Scorecards are in the usual folder."] },
  { subject: "Budget check before month end", paragraphs: ["Finance wants final numbers by the 28th. We're about 6% under on tooling and slightly over on contractors.", "Can you confirm the contractor extension?"] },
  { subject: "Notes from the customer call", paragraphs: ["Good call with the ops team today. Their biggest ask is bulk export, and they're happy to pilot it.", "They also mentioned SSO twice — worth a follow-up."] },
  { subject: "Launch checklist review", paragraphs: ["The launch checklist is 80% green. Remaining: support macros, status page copy, and the pricing FAQ.", "I'll chase the macros."] },
  { subject: "Dinner Saturday?", paragraphs: ["We're doing a small dinner Saturday around 7. Bring nothing, or bring dessert if you insist.", "Let me know by Thursday so I know how much to cook."] },
  { subject: "Spec: notification settings", paragraphs: ["First pass at the notification settings spec. Three levels — all, mentions, none — plus a per-project override.", "Open question: should digest emails be on by default?"] },
  { subject: "Travel plans for the conference", paragraphs: ["Booked the hotel for the conference, two blocks from the venue. Flights are still open — want to coordinate?", "Arriving Tuesday night seems safest."] },
  { subject: "Invoice question", paragraphs: ["Quick one on last month's invoice — line 4 looks like it was billed twice. Could you take a look?", "Happy to jump on a call if easier."] },
  { subject: "Moving our 1:1", paragraphs: ["Can we push our 1:1 to Thursday this week? Something came up Tuesday afternoon.", "Same time works on my end."] },
  { subject: "Draft blog post for review", paragraphs: ["Here's the draft for the engineering blog about our sync rewrite. Aimed at ~1,200 words, currently a bit over.", "Especially want eyes on the intro."] },
  { subject: "Recap: quarterly planning", paragraphs: ["Recap from planning: we committed to three outcomes and cut the analytics revamp to next quarter.", "Owners are listed in the doc; shout if yours looks wrong."] },
  { subject: "Photos from the weekend", paragraphs: ["Uploaded the photos from the weekend. A few really good ones of the kids at the pier.", "Grab whichever you want."] },
  { subject: "Contract redlines", paragraphs: ["Our counsel sent redlines back. Mostly the liability cap and the notice period.", "Nothing that should block signing next week."] },
  { subject: "Research readout next week", paragraphs: ["The research readout is set for next Tuesday. Twelve interviews, three clear themes.", "Pre-read goes out Monday."] },
];
const REPLIES = [
  "Sounds good — let's do it.",
  "Thanks, this is really helpful. A couple of small notes inline.",
  "Works for me. I'll send an invite.",
  "Can we push this a day? Tomorrow is packed.",
  "Agreed on all points. Shipping it.",
  "Looping in the team so they have context.",
  "Great, I'll take the first pass and share by end of day.",
];
const NEWSLETTERS: { from: Address; subjects: string[]; blurb: string }[] = [
  { from: P("Tidewater Weekly", "news@tidewater.example"), subjects: ["This week: the quiet return of the office", "Five charts on remote hiring", "The seed market, explained"], blurb: "The stories worth your time this week, in five minutes or less." },
  { from: P("Changelog Digest", "digest@changelog.example"), subjects: ["New: faster builds, better previews", "What shipped in September", "Release notes: v4.2"], blurb: "Everything that shipped this month across the product." },
  { from: P("Fieldnotes", "hello@fieldnotes.example"), subjects: ["On writing short emails", "The case for fewer meetings", "Design reviews that actually help"], blurb: "Essays on work, craft, and tools." },
  { from: P("Brightfield Deals", "offers@brightfield.example"), subjects: ["Your weekend picks are here", "20% off annual plans ends Friday", "Picked for you: new arrivals"], blurb: "Handpicked offers for you." },
  { from: P("Quayside Community", "community@quayside.example"), subjects: ["Meetup recap + slides", "Office hours this Thursday", "Monthly community roundup"], blurb: "News from the Quayside builder community." },
];
const NOTIFY: { from: Address; subjects: string[]; line: string }[] = [
  { from: P("Ledgerly Billing", "billing@ledgerly.example"), subjects: ["Invoice paid", "Payment received", "Your monthly statement"], line: "Your payment was received. Receipt attached for your records." },
  { from: P("Northline Air", "no-reply@northline.example"), subjects: ["Check in now for your flight", "Your itinerary has changed", "Trip receipt"], line: "Your trip details are below. Manage your booking any time." },
  { from: P("Pinecrest Bank", "alerts@pinecrest.example"), subjects: ["Your statement is ready", "Direct deposit received", "Card ending 4412 used online"], line: "Sign in to view your latest account activity." },
];
const ATT_POOL: [string, number, string][] = [
  ["Q3_Report.pdf", 3.4 * MB, "application/pdf"],
  ["Roadmap_v2.key", 6.1 * MB, "application/x-iwork-keynote-sffkey"],
  ["Budget_FY27.xlsx", 420_000, "application/vnd.ms-excel"],
  ["Screenshot 2026-09-10.png", 1.2 * MB, "image/png"],
  ["Interview_notes.docx", 88_000, "application/msword"],
  ["Contract_signed.pdf", 1.1 * MB, "application/pdf"],
  ["IMG_4410.jpg", 3.8 * MB, "image/jpeg"],
];

function generate(count: number) {
  const r = rng(42);
  const accKeys: AccKey[] = ["nw", "hl", "pe"];
  // Spread from ~2 days ago back through ~18 months, denser toward now.
  for (let i = 0; i < count; i++) {
    const acc = pick(r, accKeys);
    const accountId = ACC[acc];
    const seed = Math.floor(r() * 1e9);
    const age = 2 * DAY + Math.pow(i / count, 1.6) * 540 * DAY + r() * 6 * HOUR;
    const lastDate = Math.floor(NOW - age);
    const roll = r();
    const kind: MockThread["kind"] = roll < 0.22 ? "newsletter" : roll < 0.34 ? "notification" : "person";
    const labels = new Set<string>();
    let subject: string;
    let snippet: string;
    let participants: Address[];
    let messageCount = 1;
    let hasAttachments = false;

    if (kind === "newsletter") {
      const n = pick(r, NEWSLETTERS);
      subject = pick(r, n.subjects);
      snippet = n.blurb;
      participants = [n.from];
      labels.add(r() < 0.6 ? "CATEGORY_UPDATES" : "CATEGORY_PROMOTIONS");
    } else if (kind === "notification") {
      const n = pick(r, NOTIFY);
      subject = pick(r, n.subjects);
      snippet = n.line;
      participants = [n.from];
      hasAttachments = r() < 0.5;
      if (acc === "hl" && r() < 0.6) labels.add("Label_receipts");
      if (acc === "pe" && subject.includes("flight")) labels.add("Label_travel");
    } else {
      const t = pick(r, TOPICS);
      const who = pick(r, GEN_PEOPLE[acc]);
      subject = (r() < 0.3 ? "Re: " : "") + t.subject;
      messageCount = 1 + Math.floor(Math.pow(r(), 2) * 7);
      snippet = messageCount > 1 && r() < 0.5 ? pick(r, REPLIES) : t.paragraphs[0];
      const other = pick(r, GEN_PEOPLE[acc]);
      participants = other !== who && messageCount > 2 ? [who, other, ME[acc]] : messageCount > 1 ? [who, ME[acc]] : [who];
      hasAttachments = r() < 0.25;
      if (r() < 0.55) labels.add("IMPORTANT");
      if (acc === "nw" && r() < 0.25) labels.add("Label_clients");
      if (acc === "hl" && r() < 0.15) labels.add("Label_finance");
      if (acc === "pe" && r() < 0.12) labels.add("Label_home");
      if (r() < 0.06) labels.add("STARRED");
    }

    // Where does it live? Recent threads lean toward the inbox.
    const place = r();
    const inboxBias = i < count * 0.2 ? 0.75 : 0.35;
    if (place < inboxBias) labels.add("INBOX");
    else if (place < inboxBias + 0.04) labels.add("TRASH");
    if (kind === "person" && r() < 0.12) labels.add("SENT");
    if (labels.has("INBOX") && r() < (i < count * 0.2 ? 0.45 : 0.12)) labels.add("UNREAD");

    const threadId = `g-${i.toString(36)}-${seed.toString(36)}`;
    threads.set(key(accountId, threadId), {
      seed,
      kind,
      messages: null,
      summary: {
        accountId, threadId, subject, snippet, participants, messageCount,
        unread: labels.has("UNREAD"), starred: labels.has("STARRED"), hasAttachments,
        labelIds: [...labels], lastDate,
      },
    });
  }
  generateFolders(Math.max(12, Math.round(count / 6)));
  // A few drafts per account.
  for (const acc of [...accKeys, "fm" as const]) {
    for (let d = 0; d < 2; d++) {
      const threadId = `draft-${acc}-${d}`;
      const t = TOPICS[(d * 5 + acc.length) % TOPICS.length];
      const to = GEN_PEOPLE[acc][d];
      threads.set(key(ACC[acc], threadId), {
        seed: d + 7, kind: "person", messages: null,
        summary: {
          accountId: ACC[acc], threadId, subject: t.subject, snippet: t.paragraphs[0], participants: [to],
          messageCount: 1, unread: false, starred: false, hasAttachments: false, labelIds: ["DRAFT"], lastDate: NOW - (d + 1) * 5 * HOUR,
        },
      });
    }
  }
}

/**
 * The IMAP account's mail (its own seed, so the other accounts' threads stay
 * as they were): the inbox, or one folder (IMAP has one place per message),
 * no IMPORTANT or CATEGORY_* labels.
 */
function generateFolders(count: number) {
  const r = rng(7);
  for (let i = 0; i < count; i++) {
    const seed = Math.floor(r() * 1e9);
    const age = 3 * HOUR + Math.pow(i / count, 1.4) * 300 * DAY + r() * 6 * HOUR;
    const t = pick(r, TOPICS);
    const who = pick(r, GEN_PEOPLE.fm);
    const messageCount = 1 + Math.floor(Math.pow(r(), 2) * 4);
    const labels = new Set<string>();
    const place = r();
    if (place < (i < count * 0.3 ? 0.7 : 0.25)) labels.add("INBOX");
    else if (place < 0.8) labels.add(pick(r, FM_FOLDERS));
    if (labels.has("INBOX") && r() < 0.4) labels.add("UNREAD");
    if (r() < 0.05) labels.add("STARRED");
    const threadId = `fm-${i.toString(36)}-${seed.toString(36)}`;
    threads.set(key(ACC.fm, threadId), {
      seed,
      kind: "person",
      messages: null,
      summary: {
        accountId: ACC.fm, threadId, subject: (messageCount > 1 ? "Re: " : "") + t.subject, snippet: t.paragraphs[0],
        participants: messageCount > 1 ? [who, ME.fm] : [who], messageCount,
        unread: labels.has("UNREAD"), starred: labels.has("STARRED"), hasAttachments: r() < 0.15,
        labelIds: [...labels], lastDate: Math.floor(NOW - age),
      },
    });
  }
}

function build() {
  for (const h of handmade()) {
    const accountId = ACC[h.acc];
    const last = h.messages[h.messages.length - 1];
    const labels = [...h.labels];
    if (h.messages.some((m) => m.from.email === ME[h.acc].email) && !labels.includes("SENT")) labels.push("SENT");
    threads.set(key(accountId, h.id), {
      seed: 1,
      kind: h.kind ?? "person",
      messages: h.messages,
      summary: {
        accountId,
        threadId: h.id,
        subject: h.subject,
        snippet: last.paragraphs.find((p) => !/^(hi|hey)\b/i.test(p)) ?? last.paragraphs[0],
        participants: h.participants ?? uniqueParticipants(h.messages),
        messageCount: h.messages.length,
        unread: labels.includes("UNREAD"),
        starred: labels.includes("STARRED"),
        hasAttachments: h.messages.some((m) => (m.attachments?.length ?? 0) > 0),
        labelIds: labels,
        lastDate: last.date,
      },
    });
  }
  const n = Number(new URLSearchParams(typeof location !== "undefined" ? location.search : "").get("mockThreads"));
  generate(Number.isFinite(n) && n > 0 ? n : 300);
}

function uniqueParticipants(msgs: MsgSpec[]): Address[] {
  const seen = new Map<string, Address>();
  // Most recent sender first.
  for (let i = msgs.length - 1; i >= 0; i--) if (!seen.has(msgs[i].from.email)) seen.set(msgs[i].from.email, msgs[i].from);
  return [...seen.values()];
}

/** Lazily expand a generated thread into messages consistent with its summary. */
function messagesOf(t: MockThread): MsgSpec[] {
  if (t.messages) return t.messages;
  const s = t.summary;
  const r = rng(t.seed);
  const acc = accKeyOf(s.accountId);
  const me = ME[acc];
  const other = s.participants.find((p) => p.email !== me.email) ?? s.participants[0];
  const msgs: MsgSpec[] = [];
  if (t.kind === "newsletter") {
    msgs.push({ from: other, to: [me], date: s.lastDate, paragraphs: [s.snippet], newsletter: true });
  } else if (t.kind === "notification") {
    msgs.push({
      from: other, to: [me], date: s.lastDate,
      paragraphs: [s.snippet, "If you have questions, reply to this email and our team will get back to you."],
      attachments: s.hasAttachments ? [att("Receipt.pdf", 120_000 + Math.floor(r() * 90_000), "application/pdf")] : undefined,
    });
  } else {
    const topic = TOPICS.find((x) => s.subject.endsWith(x.subject)) ?? TOPICS[0];
    const cast = s.participants.filter((p) => p.email !== me.email);
    for (let i = 0; i < s.messageCount; i++) {
      const last = i === s.messageCount - 1;
      const from = s.messageCount === 1 ? other : i % 2 === 1 ? me : cast[(i / 2) % cast.length | 0] ?? other;
      const paragraphs =
        i === 0 ? [`Hi ${from === me ? firstName(other) : "Sam"},`, ...topic.paragraphs] : [last ? s.snippet : pick(r, REPLIES)];
      msgs.push({
        from,
        to: from === me ? [other] : [me],
        date: s.lastDate - (s.messageCount - 1 - i) * (3 * HOUR + Math.floor(r() * 20 * HOUR)),
        paragraphs,
        signoff: firstName(from),
        attachments: last && s.hasAttachments ? [att(...pick(r, ATT_POOL))] : undefined,
      });
    }
  }
  t.messages = msgs;
  return msgs;
}

const firstName = (a: Address) => (a.name ?? a.email).split(/\s+/)[0];

// ---------------------------------------------------------------------------
// HTML bodies (what penguin-render would hand the iframe)
// ---------------------------------------------------------------------------
function esc(s: string): string {
  return s.replace(/[&<>"']/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;", "'": "&#39;" })[c]!);
}

// Mirrors penguin-render's two document kinds (see MessageBody): plain mail
// renders as "pg-text" (transparent, follows the app theme via the data-theme
// MessageBody stamps on <html>), rich HTML mail as "pg-html" (its own light
// canvas, shown as a card in dark mode).
const TEXT_STYLE = `
  html { color-scheme: light; background: transparent; color: #333; }
  html[data-theme="dark"] { color-scheme: dark; color: #a1a4a5; }
  body { margin: 0; padding: 0; font: 14px/22px Inter, -apple-system, system-ui, sans-serif; overflow-wrap: anywhere; }
  p { margin: 0 0 10px; }
  ol, ul { margin: 0 0 10px; padding-left: 20px; }
  li { margin-bottom: 2px; }
  a { color: #006dcb; }
  html[data-theme="dark"] a { color: #70b8ff; }
  .sig { opacity: .7; font-size: 12px; }
  .pg-quote { margin: 6px 0; white-space: pre-wrap; }
  .pg-quote > summary { display: inline-block; list-style: none; cursor: pointer; padding: 0 7px; border-radius: 6px; font-size: 12px; line-height: 16px; letter-spacing: 1px; background: rgba(127,127,127,.16); opacity: .75; }
  .pg-quote > summary::-webkit-details-marker { display: none; }
  .pg-quote blockquote { margin: 6px 0 0; padding-left: 12px; border-left: 2px solid rgba(127,127,127,.35); opacity: .8; }
  .pg-attribution { opacity: .8; }
`;
const HTML_STYLE = `
  html { color-scheme: light; background: #fff; }
  body { margin: 0; padding: 0; }
  body { font: 14px/22px -apple-system, system-ui, sans-serif; color: #222; overflow-wrap: anywhere; }
  p { margin: 0 0 10px; }
  a { color: #1f6fd6; }
  .nl { max-width: 560px; margin: 0 auto; }
  .nl-hero { padding: 22px 22px 18px; background: linear-gradient(135deg, #5b3fd6, #1f7ae0); color: #fff; }
  .nl-hero h1 { margin: 0 0 6px; font-size: 20px; line-height: 26px; }
  .nl-body { padding: 18px 22px; }
  .nl-btn { display: inline-block; padding: 8px 14px; border-radius: 8px; background: #111; color: #fff; text-decoration: none; }
  .nl-foot { padding: 12px 22px; font-size: 11px; color: #777; border-top: 1px solid #eee; }
`;

/** "On … Name <email> wrote:" and the ">" lines, plus penguin-render's folded form of them. */
function replyQuote(prev: MsgSpec): { text: string; html: string } {
  const when = new Date(prev.date).toLocaleString("en-US", { weekday: "short", month: "short", day: "numeric", year: "numeric", hour: "numeric", minute: "2-digit" });
  const attribution = `On ${when.replace(/, (\d+:)/, " at $1")} ${prev.from.name ?? prev.from.email} <${prev.from.email}> wrote:`;
  const body = textFor(prev);
  return {
    text: `${attribution}\n\n${body.split("\n").map((l) => (l ? `> ${l}` : ">")).join("\n")}`,
    html:
      `<p><span class="pg-attribution">${esc(attribution)}</span></p>` +
      `<details class="pg-quote"><summary title="Show quoted text">…</summary><blockquote>${esc(body).replace(/\n/g, "<br>")}</blockquote></details>`,
  };
}

function htmlFor(m: MsgSpec, subject: string, quote = ""): string {
  let body: string;
  if (m.newsletter) {
    body = `<div class="nl"><div class="nl-hero"><h1>${esc(subject)}</h1><div>${esc(m.paragraphs[0])}</div></div>
      <div class="nl-body"><p>Here's what we've been reading and building. Three short pieces, one long read, and a tool we can't stop using.</p>
      <p>1. <b>Smaller teams ship faster</b> — why the best teams keep their core loop tiny.</p>
      <p>2. <b>Write it down</b> — the async memo template we use for every decision.</p>
      <p>3. <b>The keyboard is back</b> — power tools are having a moment again.</p>
      <p><a class="nl-btn" href="https://example.com/read">Read the issue</a></p></div>
      <div class="nl-foot">You're receiving this because you subscribed. <a href="https://example.com/unsubscribe">Unsubscribe</a></div></div>`;
  } else {
    const ps = m.paragraphs.map((p) => `<p>${esc(p)}</p>`).join("");
    const list = m.list ? `<ol>${m.list.map((li) => `<li>${esc(li)}</li>`).join("")}</ol>` : "";
    const signoff = m.signoff ? `<p>${esc(m.signoff).replace(/\n/g, "<br>")}</p>` : "";
    const sig = m.sig ? `<p class="sig">${esc(m.sig)}</p>` : "";
    body = ps + list + signoff + sig + quote;
  }
  return m.newsletter
    ? `<!DOCTYPE html><html class="pg-html"><head><meta charset="utf-8"><style>${HTML_STYLE}</style></head><body>${body}</body></html>`
    : `<!DOCTYPE html><html class="pg-text"><head><meta charset="utf-8"><style>${TEXT_STYLE}</style></head><body>${body}</body></html>`;
}

function textFor(m: MsgSpec): string {
  return [...m.paragraphs, ...(m.list ?? []).map((l, i) => `${i + 1}. ${l}`), m.signoff ?? "", m.sig ?? ""].filter(Boolean).join("\n\n");
}

function messageView(t: MockThread, m: MsgSpec, i: number, imagesLoaded = false): MessageView {
  const s = t.summary;
  const last = i === (t.messages?.length ?? 1) - 1;
  const prev = m.quotesPrevious && i > 0 ? t.messages?.[i - 1] : undefined;
  const quote = prev ? replyQuote(prev) : null;
  const text = quote ? `${textFor(m)}\n\n${quote.text}` : textFor(m);
  return {
    accountId: s.accountId,
    id: `${s.threadId}-m${i}`,
    threadId: s.threadId,
    date: m.date,
    from: m.from,
    to: m.to,
    cc: m.cc ?? [],
    bcc: [],
    replyTo: [],
    subject: s.subject,
    // Like Gmail's snippet: the body start, minus a bare greeting line.
    snippet: textFor({ ...m, paragraphs: m.paragraphs.filter((p, j) => j > 0 || !/^(hi|hey|hello)\b[^.!?]*,$/i.test(p)) }).replace(/\s+/g, " ").slice(0, 140),
    bodyText: text,
    html: htmlFor(m, s.subject, quote?.html),
    // Newsletters: 3 images, 2 pixels, 3 links carrying per-reader ids, 1 behind a click tracker.
    ...(m.newsletter
      ? mockPrivacy({ trackers: 2, images: 3, links: 3, tracked: 1 }, mockSettings, imagesLoaded)
      : m.bulk
        ? mockPrivacy({ ...m.bulk, links: 1, tracked: 1 }, mockSettings, imagesLoaded)
        : { blockedRemoteImages: 0, trackersRemoved: 0, trackers: [] }),
    labelIds: last ? s.labelIds : s.labelIds.filter((l) => l !== "UNREAD"),
    attachments: m.attachments ?? [],
    unread: last && s.unread,
    starred: s.starred,
    readReceipts: mockReceipts(s.threadId, `${s.threadId}-m${i}`, m.date, m.to, Object.values(ME).some((a) => a.email === m.from.email)),
  };
}

// ---------------------------------------------------------------------------
// Queries
// ---------------------------------------------------------------------------
const NEWSLETTER_LABELS = ["CATEGORY_PROMOTIONS", "CATEGORY_UPDATES", "CATEGORY_FORUMS", "CATEGORY_SOCIAL"];
const isNewsletter = (s: ThreadSummary) => s.labelIds.some((l) => NEWSLETTER_LABELS.includes(l));

function inView(s: ThreadSummary, view: MailboxView, tab: InboxTab | null): boolean {
  const has = (l: string) => s.labelIds.includes(l);
  const junk = has("TRASH") || has("SPAM");
  switch (view.kind) {
    case "inbox": {
      if (!has("INBOX") || junk) return false;
      switch (tab ?? "all") {
        case "newsletters": return isNewsletter(s);
        case "important": return has("IMPORTANT") && !isNewsletter(s);
        case "other": return !has("IMPORTANT") && !isNewsletter(s);
        default: return true;
      }
    }
    case "starred": return has("STARRED") && !junk;
    case "sent": return has("SENT") && !junk;
    case "drafts": return has("DRAFT") && !junk;
    case "done": return !has("INBOX") && !junk && !(s.labelIds.length === 1 && has("DRAFT"));
    case "trash": return has("TRASH");
    case "spam": return has("SPAM");
    case "all": return !junk;
    case "label": return has(view.labelId) && !junk;
    case "snoozed": return !!snoozeOf(s.accountId, s.threadId);
    case "replyLater": {
      const rl = replyLaterLabelId(s.accountId);
      return !!rl && has(rl) && !junk;
    }
    // Served by mockViews (./triage.ts, ./smart.ts).
    case "followUp":
    case "smart":
    case "query":
      return false;
  }
}

/**
 * Views another mock module serves itself (./triage.ts: Follow up), keyed by
 * MailboxView kind. list_threads hands the whole query over.
 */
export const mockViews: Partial<Record<MailboxView["kind"], (q: ListQuery) => ThreadSummary[]>> = {};

/** The account's Reply Later label id, if it has one (lib/types REPLY_LATER_LABEL). */
export function replyLaterLabelId(accountId: string): string | null {
  return USER_LABELS.find((l) => ACC[l.acc] === accountId && l.name.toLowerCase() === "reply later")?.id ?? null;
}

/**
 * Find or create a user label named `name`, as MailProvider::ensure_label
 * does: a Gmail-style id, an IMAP folder `f:` or a Microsoft category `c:`.
 */
export function ensureMockLabel(accountId: string, name: string): string {
  const found = USER_LABELS.find((l) => ACC[l.acc] === accountId && l.name.toLowerCase() === name.toLowerCase());
  if (found) return found.id;
  const a = accounts.find((x) => x.id === accountId);
  const enc = encodeURIComponent(name).replace(/[!'()*]/g, (c) => `%${c.charCodeAt(0).toString(16).toUpperCase()}`);
  const id = a?.provider === "imap" ? `f:${enc}` : a?.provider === "microsoft" ? `c:${enc}` : `Label_${name.replace(/\W+/g, "").toLowerCase()}`;
  USER_LABELS.push({ acc: accKeyOf(accountId), id, name, color: null });
  return id;
}

function sorted(): ThreadSummary[] {
  return [...threads.values()].map((t) => t.summary).sort((a, b) => b.lastDate - a.lastDate);
}

function labelsFor(accountId: string | null, accountIds: string[] | null = null): Label[] {
  const out: Label[] = [];
  const all = [...threads.values()].map((t) => t.summary);
  for (const a of accounts) {
    if (accountId && a.id !== accountId) continue;
    if (accountIds && !accountIds.includes(a.id)) continue;
    const mine = all.filter((s) => s.accountId === a.id);
    // As the backend counts (store.rs list_labels, app/optimistic.ts unreadLabels):
    // a conversation in Trash or Spam counts only there.
    const unreadWith = (id: string) => mine.filter((s) => unreadLabels(s).includes(id)).length;
    for (const id of SYSTEM_LABELS) {
      // Only Gmail has IMPORTANT (docs/PROVIDERS-IMPL.md §4).
      if (id === "IMPORTANT" && !a.capabilities.inboxCategories) continue;
      out.push({ accountId: a.id, id, name: id, kind: "system", color: null, unreadCount: unreadWith(id), hidden: false });
    }
    for (const l of USER_LABELS.filter((u) => ACC[u.acc] === a.id)) {
      out.push({ accountId: a.id, id: l.id, name: l.name, kind: "user", color: l.color, unreadCount: unreadWith(l.id), hidden: !!l.hidden });
    }
  }
  return out;
}

// ---------------------------------------------------------------------------
// Sync simulation: Northwind backfills for ~25s after start, others are idle.
// ---------------------------------------------------------------------------
const TOTAL_NW = 142_318;
const sync: Record<string, SyncStatus> = {
  [ACC.nw]: { accountId: ACC.nw, phase: "backfilling", indexed: 38_112, totalEstimate: TOTAL_NW, lastSyncedAt: NOW - 20_000, error: null, ratePerMin: 1_140, etaSecs: 5_480 },
  [ACC.hl]: { accountId: ACC.hl, phase: "idle", indexed: 21_406, totalEstimate: 21_406, lastSyncedAt: NOW - 8_000, error: null, ratePerMin: null, etaSecs: null },
  [ACC.pe]: { accountId: ACC.pe, phase: "idle", indexed: 9_877, totalEstimate: 9_877, lastSyncedAt: NOW - 12_000, error: null, ratePerMin: null, etaSecs: null },
  [ACC.fm]: { accountId: ACC.fm, phase: "idle", indexed: 3_214, totalEstimate: 3_214, lastSyncedAt: NOW - 15_000, error: null, ratePerMin: null, etaSecs: null },
};
let syncTimer: ReturnType<typeof setInterval> | null = null;

// sync_now: each healthy account "polls" after 0.4–1.6s, sometimes bringing new
// inbox mail (mail-changed, then a sync-status with a fresh lastSyncedAt), like
// the engine's incremental poll. Accounts in error/needsReauth just report back.
// ?mockRefresh=error makes Harbor Labs' poll fail (Retry sync clears it).
let refreshRound = 0;
const NEW_MAIL_PLAN: Record<AccKey, number[]> = { nw: [2, 0, 1, 0], hl: [0, 1, 0, 0], pe: [1, 0, 0, 3], fm: [0, 1, 0, 0] };
function simulateSyncNow() {
  const round = refreshRound++;
  const failHarbor = typeof location !== "undefined" && new URLSearchParams(location.search).get("mockRefresh") === "error";
  accounts.forEach((a, i) => {
    const s = sync[a.id];
    setTimeout(() => {
      if (s.phase === "needsReauth" || s.phase === "error") return emit("penguin://sync-status", { ...s });
      if (failHarbor && a.id === ACC.hl) {
        mockRecordFailure(s, "network", "network: operation timed out", Date.now(), mockBackoff((s.failure?.count ?? 0) + 1));
        return emit("penguin://sync-status", { ...s });
      }
      const acc = accKeyOf(a.id);
      const plan = NEW_MAIL_PLAN[acc];
      const n = plan[round % plan.length];
      const ids: string[] = [];
      for (let k = 0; k < n; k++) {
        const who = GEN_PEOPLE[acc][(round + k) % GEN_PEOPLE[acc].length];
        const topic = TOPICS[(round * 3 + k + i) % TOPICS.length];
        const threadId = `new-${acc}-${round}-${k}`;
        threads.set(key(a.id, threadId), {
          seed: round * 31 + k,
          kind: "person",
          messages: null,
          summary: {
            accountId: a.id, threadId, subject: topic.subject, snippet: topic.paragraphs[0], participants: [who],
            messageCount: 1, unread: true, starred: false, hasAttachments: false,
            labelIds: a.capabilities.inboxCategories ? ["INBOX", "UNREAD", "IMPORTANT"] : ["INBOX", "UNREAD"], lastDate: Date.now() - k * MIN,
          },
        });
        ids.push(threadId);
      }
      if (ids.length) emit("penguin://mail-changed", { accountId: a.id, threadIds: ids });
      s.lastSyncedAt = Date.now();
      emit("penguin://sync-status", { ...s });
    }, 400 + i * 350 + Math.floor(Math.random() * 400));
  });
}

function emit(event: string, payload: unknown) {
  // mockBackend is only touched at call time, so the ./index <-> ./mail cycle is safe.
  mockBackend.emit(event, payload);
}

function startSyncSimulation() {
  if (syncTimer) return;
  // Screenshot mode (?mockSync=idle) keeps everything settled.
  if (typeof location !== "undefined" && new URLSearchParams(location.search).get("mockSync") === "idle") {
    Object.assign(sync[ACC.nw], { phase: "idle", indexed: TOTAL_NW, lastSyncedAt: Date.now(), ratePerMin: null, etaSecs: null });
    return;
  }
  // ?mockSync=busy freezes a mixed state for the sync popover: Northwind
  // backfilling, Harbor Labs still estimating, Personal signed out.
  if (typeof location !== "undefined" && new URLSearchParams(location.search).get("mockSync") === "busy") {
    Object.assign(sync[ACC.hl], { phase: "backfilling", indexed: 1_250, ratePerMin: null, etaSecs: null });
    Object.assign(sync[ACC.pe], { phase: "needsReauth", error: "Token was revoked (invalid_grant)" });
    return;
  }
  syncTimer = setInterval(() => {
    const s = sync[ACC.nw];
    if (s.phase === "backfilling") {
      s.indexed = Math.min(TOTAL_NW, s.indexed + 4_200 + Math.floor(Math.random() * 600));
      // Report a real-world pace (~1.1k/min), not the demo's accelerated one.
      s.ratePerMin = 1_080 + Math.floor(Math.random() * 120);
      s.etaSecs = Math.round(((TOTAL_NW - s.indexed) / s.ratePerMin) * 60);
      if (s.indexed >= TOTAL_NW) {
        Object.assign(s, { phase: "idle", lastSyncedAt: Date.now(), ratePerMin: null, etaSecs: null });
      }
      emit("penguin://sync-status", { ...s });
    } else {
      // Settled: periodic incremental "polls" refresh lastSyncedAt.
      for (const a of accounts) {
        sync[a.id].lastSyncedAt = Date.now();
        emit("penguin://sync-status", { ...sync[a.id] });
      }
      if (syncTimer) clearInterval(syncTimer);
      syncTimer = setInterval(() => {
        for (const a of accounts) {
          sync[a.id].lastSyncedAt = Date.now();
          emit("penguin://sync-status", { ...sync[a.id] });
        }
      }, 30_000);
    }
  }, 900);
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------
function apply(s: ThreadSummary, action: ThreadAction) {
  const set = new Set(s.labelIds);
  switch (action.kind) {
    case "archive": set.delete("INBOX"); break;
    case "moveToInbox": set.add("INBOX"); set.delete("TRASH"); break;
    case "trash": set.add("TRASH"); break;
    case "untrash": set.delete("TRASH"); set.add("INBOX"); break;
    case "reportSpam": set.add("SPAM"); set.delete("INBOX"); break;
    case "notSpam": set.delete("SPAM"); set.add("INBOX"); break;
    case "markRead": set.delete("UNREAD"); break;
    case "markUnread": set.add("UNREAD"); break;
    case "star": set.add("STARRED"); break;
    case "unstar": set.delete("STARRED"); break;
    case "addLabel": set.add(action.labelId); break;
    case "removeLabel": set.delete(action.labelId); break;
    case "replyLater": set.add(action.labelId); set.delete("INBOX"); set.delete("UNREAD"); break;
  }
  s.labelIds = [...set];
  s.unread = set.has("UNREAD");
  s.starred = set.has("STARRED");
}

build();

// ---------------------------------------------------------------------------
// Attachment previews (what preview_attachment would return)
// ---------------------------------------------------------------------------
function b64(bin: string): string {
  return btoa(bin);
}

/** A real one-page PDF (hand-built, correct xref) so WebKit's viewer renders it. */
function samplePdf(title: string): string {
  const safe = title.replace(/[()\\]/g, "");
  const lines = [
    `BT /F1 26 Tf 72 700 Td (${safe}) Tj ET`,
    "BT /F1 13 Tf 72 664 Td (Northwind - brand review, v7) Tj ET",
    "0.55 0.37 1 rg 72 420 468 200 re f",
    "1 1 1 rg BT /F1 40 Tf 110 505 Td (Linden & Co) Tj ET",
    "0.2 0.2 0.2 rg BT /F1 12 Tf 72 380 Td (Wordmark spacing tightened at small sizes; warm gray vs cool neutral on p.14.) Tj ET",
  ].join("\n");
  const objs = [
    "<< /Type /Catalog /Pages 2 0 R >>",
    "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
    "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R /Resources << /Font << /F1 5 0 R >> >> >>",
    `<< /Length ${lines.length} >>\nstream\n${lines}\nendstream`,
    "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
  ];
  let out = "%PDF-1.4\n";
  const offs: number[] = [];
  objs.forEach((o, i) => {
    offs.push(out.length);
    out += `${i + 1} 0 obj\n${o}\nendobj\n`;
  });
  const xref = out.length;
  out += `xref\n0 ${objs.length + 1}\n0000000000 65535 f \n` + offs.map((o) => `${String(o).padStart(10, "0")} 00000 n \n`).join("");
  out += `trailer\n<< /Size ${objs.length + 1} /Root 1 0 R >>\nstartxref\n${xref}\n%%EOF\n`;
  return "data:application/pdf;base64," + b64(out);
}

/** A PNG drawn on a canvas (the mock runs in a browser). */
function sampleImage(label: string): string {
  const c = document.createElement("canvas");
  c.width = 1600;
  c.height = 900;
  const g = c.getContext("2d")!;
  const grad = g.createLinearGradient(0, 0, 1600, 900);
  grad.addColorStop(0, "#5b3fd6");
  grad.addColorStop(1, "#1f7ae0");
  g.fillStyle = grad;
  g.fillRect(0, 0, 1600, 900);
  g.fillStyle = "rgba(255,255,255,.12)";
  for (let i = 0; i < 3; i++) g.fillRect(160 + i * 440, 260, 400, 380);
  g.fillStyle = "#fff";
  g.font = "600 84px -apple-system, Inter, sans-serif";
  g.textAlign = "center";
  ["Northwind", "NW", "northwind"].forEach((w, i) => g.fillText(w, 360 + i * 440, 480));
  g.font = "500 30px -apple-system, Inter, sans-serif";
  g.fillText(label, 800, 780);
  return c.toDataURL("image/png");
}

const SAMPLE_TEXT: Record<string, string> = {
  md: "# Onboarding crit — notes\n\n## Biggest theme\nStep three asks for too much. Defer the team invite and calendar connection until after the first project exists.\n\n## Smaller things\n- Progress indicator is doing too much work: drop the step labels.\n- The skip link needs to be a real button.\n- Final screen copy should say what happens next.\n\n<script>alert('never rendered')</script> ← shown as text, never run\n",
  csv: "date,source,signups,activated\n2026-09-22,changelog,24,17\n2026-09-22,twitter,9,5\n2026-09-22,direct,7,6\n2026-09-23,changelog,31,20\n",
};

function mockPreview(a: AttachmentMeta) {
  const ext = (/\.([a-z0-9]+)$/i.exec(a.filename)?.[1] ?? "").toLowerCase();
  const base = { mimeType: a.mimeType, filename: a.filename, size: a.size, dataUrl: null, text: null, truncated: false, reason: null };
  if (["png", "jpg", "jpeg", "gif", "webp"].includes(ext)) return { ...base, kind: "image", mimeType: "image/png", dataUrl: sampleImage(a.filename) };
  if (ext === "pdf") return { ...base, kind: "pdf", mimeType: "application/pdf", dataUrl: samplePdf(a.filename.replace(/\.pdf$/i, "")) };
  if (["md", "csv", "txt", "json", "log", "html", "ics"].includes(ext)) {
    return { ...base, kind: "text", text: SAMPLE_TEXT[ext] ?? SAMPLE_TEXT[ext === "csv" ? "csv" : "md"] };
  }
  return { ...base, kind: "unsupported", reason: "type" };
}

/** preview_outgoing_file: a composer file from its own bytes (the extension decides here; Rust sniffs the bytes). */
function mockOutgoingPreview(filename: string, mimeType: string, dataBase64: string) {
  const ext = (/\.([a-z0-9]+)$/i.exec(filename)?.[1] ?? "").toLowerCase();
  const bin = atob(dataBase64);
  const base = { mimeType, filename, size: bin.length, dataUrl: null, text: null, truncated: false, reason: null };
  if (["png", "jpg", "jpeg", "gif", "webp"].includes(ext)) return { ...base, kind: "image", dataUrl: `data:${mimeType};base64,${dataBase64}` };
  if (ext === "pdf") return { ...base, kind: "pdf", mimeType: "application/pdf", dataUrl: `data:application/pdf;base64,${dataBase64}` };
  if (["md", "csv", "txt", "json", "log", "ics"].includes(ext) || mimeType.startsWith("text/")) {
    const bytes = Uint8Array.from(bin, (c) => c.charCodeAt(0));
    return { ...base, kind: "text", text: new TextDecoder().decode(bytes) };
  }
  return { ...base, kind: "unsupported", reason: "type" };
}

// ---------------------------------------------------------------------------
// Shared read access for other mock modules (e.g. mock/search.ts).
// ---------------------------------------------------------------------------
export const mockMail = {
  /** Mutable: push to add an account (then call setSync for its status). */
  accounts: () => accounts,
  /** The live (mutable) summary of one thread, for other mock modules (snooze). */
  summary: (accountId: string, threadId: string): ThreadSummary | undefined => threads.get(key(accountId, threadId))?.summary,
  setSync: (s: SyncStatus) => {
    sync[s.accountId] = { ...s };
    emit("penguin://sync-status", { ...s });
  },
  /**
   * Add a thread built from ready-made messages (oldest first), e.g. the
   * search mock's world, so opening a search hit lands on a real thread and it
   * shows up in the lists. Labels are the union of the messages' labels.
   */
  addThread: (view: ThreadView) => {
    const msgs = [...view.messages].sort((a, b) => a.date - b.date);
    const last = msgs[msgs.length - 1];
    if (!last) return;
    const labels = new Set<string>(view.labelIds);
    for (const m of msgs) m.labelIds.forEach((l) => labels.add(l));
    if (msgs.some((m) => m.unread)) labels.add("UNREAD");
    if (msgs.some((m) => m.starred)) labels.add("STARRED");
    const seen = new Map<string, Address>();
    for (let i = msgs.length - 1; i >= 0; i--) if (!seen.has(msgs[i].from.email)) seen.set(msgs[i].from.email, msgs[i].from);
    threads.set(key(view.accountId, view.threadId), {
      seed: 0,
      kind: "person",
      messages: null,
      views: msgs,
      summary: {
        accountId: view.accountId,
        threadId: view.threadId,
        subject: view.subject,
        snippet: last.snippet,
        participants: [...seen.values()],
        messageCount: msgs.length,
        unread: labels.has("UNREAD"),
        starred: labels.has("STARRED"),
        hasAttachments: msgs.some((m) => m.attachments.length > 0),
        labelIds: [...labels],
        lastDate: last.date,
        otp: last.otp ?? null,
      },
    });
  },
  /** All thread summaries, newest first. */
  threads: () => sorted(),
  /** Full thread view (same shape as get_thread). */
  thread: (accountId: string, threadId: string): ThreadView | null => getThread(accountId, threadId),
  labels: (accountId: string | null) => labelsFor(accountId),
};

/**
 * A ready-made view (another mock module's newsletter) as get_thread would
 * render it under the current Privacy settings: its trackers, and a few
 * links with per-reader ids.
 */
function withPrivacy(v: MessageView, imagesLoaded: boolean): MessageView {
  const trackers = v.trackersRemoved + (v.trackersAllowed ?? 0);
  if (trackers === 0) return imagesLoaded ? { ...v, blockedRemoteImages: 0 } : v;
  const images = Math.max(0, v.blockedRemoteImages - (v.trackersAllowed ?? 0));
  return { ...v, ...mockPrivacy({ trackers, images, links: 2, tracked: 1 }, mockSettings, imagesLoaded) };
}

function getThread(accountId: string, threadId: string): ThreadView | null {
  const t = threads.get(key(accountId, threadId));
  if (!t) return null;
  if (t.views) {
    const s = t.summary;
    const lastI = t.views.length - 1;
    return {
      accountId,
      threadId,
      subject: s.subject,
      labelIds: s.labelIds,
      messages: t.views.map((m, i) => ({
        ...withPrivacy(m, false),
        unread: s.unread && (m.unread || i === lastI),
        starred: s.starred,
        // Like penguin_core::unsubscribe::plan: nothing to offer in Spam
        // (unsubscribing tells a spammer the address is live).
        unsubscribe: s.labelIds.includes("SPAM") ? null : m.unsubscribe,
      })),
    };
  }
  const msgs = messagesOf(t);
  const views = msgs.map((m, i) => messageView(t, m, i));
  // Sync window: mail older than ~6 months is stored headers-only. The first
  // open returns bodyPending and "downloads" the body, then mail-changed.
  const k = key(accountId, threadId);
  if (t.summary.lastDate < NOW - SYNC_WINDOW && !bodiesLoaded.has(k)) {
    if (!bodiesLoading.has(k)) {
      bodiesLoading.add(k);
      setTimeout(() => {
        bodiesLoading.delete(k);
        bodiesLoaded.add(k);
        emit("penguin://mail-changed", { accountId, threadIds: [threadId] });
      }, 900);
    }
    return {
      accountId,
      threadId,
      subject: t.summary.subject,
      labelIds: t.summary.labelIds,
      messages: views.map((m) => ({ ...m, html: "", bodyText: "", attachments: [], blockedRemoteImages: 0, trackersRemoved: 0, trackers: [], bodyPending: true })),
    };
  }
  return { accountId, threadId, subject: t.summary.subject, labelIds: t.summary.labelIds, messages: views };
}

const SYNC_WINDOW = 180 * DAY;

// ---------------------------------------------------------------------------
// Message details / "Show original" (what get_message_details would return)
// ---------------------------------------------------------------------------
/** Messages of a thread without the sync-window "download" side effect. */
function mockThreadMessages(t: MockThread): MessageView[] {
  return t.views ?? messagesOf(t).map((m, i) => messageView(t, m, i));
}

/** suggest_recipients: every participant of the demo mailbox, word-prefix matched, most seen first. */
function mockSuggestRecipients(query: string, limit: number): Address[] {
  const words = query.toLowerCase().split(/[\s@.]+/).filter(Boolean);
  if (!words.length) return [];
  const seen = new Map<string, { a: Address; n: number }>();
  for (const t of threads.values())
    for (const a of t.summary.participants) {
      const k = a.email.toLowerCase();
      const cur = seen.get(k);
      if (cur) cur.n++;
      else seen.set(k, { a, n: 1 });
    }
  const hay = (a: Address) => `${a.name ?? ""} ${a.email}`.toLowerCase().split(/[\s@.]+/);
  return [...seen.values()]
    .filter(({ a }) => !/no-?reply|notifications?@/i.test(a.email) && words.every((w) => hay(a).some((h) => h.startsWith(w))))
    .sort((x, y) => y.n - x.n)
    .slice(0, limit)
    .map(({ a }) => a);
}

function mockPersonSummary(raw: string) {
  const email = raw.trim().toLowerCase();
  const same = (a: Address) => a.email.toLowerCase() === email;
  let name: string | null = null;
  let first: number | null = null;
  let last: number | null = null;
  let from = 0;
  let to = 0;
  const perAccount = new Map<string, number>();
  const threadsWith: { accountId: string; threadId: string; subject: string; date: number }[] = [];
  const atts: AttachmentHit[] = [];
  for (const t of [...threads.values()].sort((a, b) => b.summary.lastDate - a.summary.lastDate)) {
    if (!t.summary.participants.some(same) && !t.views?.some((m) => [m.from, ...m.to, ...m.cc].some(same))) continue;
    let hit = false;
    for (const m of mockThreadMessages(t)) {
      const isFrom = same(m.from);
      const isTo = [...m.to, ...m.cc, ...m.bcc].some(same);
      if (!isFrom && !isTo) continue;
      hit = true;
      if (isFrom) {
        from++;
        name ??= m.from.name;
        for (const a of m.attachments.filter((x) => !x.inline))
          atts.push({ accountId: m.accountId, threadId: m.threadId, messageId: m.id, attachment: a, from: m.from, date: m.date });
      } else if (accounts.some((a) => a.email === m.from.email)) to++;
      if (isTo) name ??= [...m.to, ...m.cc].find(same)?.name ?? null;
      perAccount.set(m.accountId, (perAccount.get(m.accountId) ?? 0) + 1);
      first = first == null ? m.date : Math.min(first, m.date);
      last = last == null ? m.date : Math.max(last, m.date);
    }
    if (hit && threadsWith.length < 5)
      threadsWith.push({ accountId: t.summary.accountId, threadId: t.summary.threadId, subject: t.summary.subject, date: t.summary.lastDate });
  }
  const others = new Map<string, Address>();
  if (name) {
    for (const t of threads.values())
      for (const p of t.summary.participants)
        if (p.name === name && !same(p)) others.set(p.email.toLowerCase(), p);
  }
  return {
    email,
    name,
    otherAddresses: [...others.values()].slice(0, 5),
    domain: email.split("@")[1] ?? "",
    firstContact: first,
    lastContact: last,
    messagesFrom: from,
    messagesTo: to,
    accounts: [...perAccount].map(([accountId, count]) => ({ accountId, count })).sort((a, b) => b.count - a.count),
    recentThreads: threadsWith,
    recentAttachments: atts.sort((a, b) => b.date - a.date).slice(0, 5),
  };
}

function findMockMessage(accountId: string, messageId: string): MessageView | undefined {
  const view = getThread(accountId, String(messageId).replace(/-m\d+$/, ""));
  const found = view?.messages.find((m) => m.id === messageId);
  if (found) return found;
  for (const t of threads.values()) {
    const v = t.summary.accountId === accountId ? t.views?.find((m) => m.id === messageId) : undefined;
    if (v) return v;
  }
  return undefined;
}

const domainOf = (email: string) => email.split("@")[1] ?? email;
const rfcAddr = (a: Address) => (a.name ? `"${a.name}" <${a.email}>` : a.email);
const mockMsgId = (m: MessageView) => `<${m.id}.${m.threadId}@mail.${domainOf(m.from.email)}>`;

function mockDetails(m: MessageView) {
  const mine = accounts.some((a) => a.email === m.from.email);
  const d = domainOf(m.from.email);
  const newsletter = m.blockedRemoteImages > 0 || m.trackersRemoved > 0;
  // Mine: no Authentication-Results (sent mail). One sender is a relay that fails SPF.
  const shady = d === "brightfield.example" || SPAM_DOMAINS.has(d);
  const idx = Number(/-m(\d+)$/.exec(m.id)?.[1] ?? 0);
  return {
    accountId: m.accountId,
    messageId: m.id,
    threadId: m.threadId,
    subject: m.subject,
    from: m.from,
    replyTo: newsletter ? [{ name: null, email: `reply@${d}` }] : [],
    to: m.to,
    cc: m.cc,
    bcc: m.bcc,
    date: m.date,
    dateHeader: new Date(m.date).toUTCString().replace("GMT", "+0000"),
    messageIdHeader: mockMsgId(m),
    inReplyTo: idx > 0 ? `<${m.threadId}-m${idx - 1}.${m.threadId}@mail.${d}>` : null,
    references: Array.from({ length: idx }, (_, i) => `<${m.threadId}-m${i}.${m.threadId}@mail.${d}>`),
    labelIds: m.labelIds,
    attachments: m.attachments.filter((a) => !a.inline),
    size: 6_000 + m.bodyText.length * 2 + m.attachments.reduce((n, a) => n + a.size, 0),
    senderAuthenticated: !mine && !shady,
    auth: mine
      ? { spf: null, mailedBy: null, dkim: null, signedBy: [], dmarc: null }
      : shady
        ? { spf: "softfail", mailedBy: "bulk-relay.example", dkim: "none", signedBy: [], dmarc: "fail" }
        : { spf: "pass", mailedBy: newsletter ? `bounce.${d}` : d, dkim: "pass", signedBy: [d], dmarc: "pass" },
    transport: mine ? { tls: null, detail: null } : { tls: true, detail: "TLS1_3 · TLS_AES_256_GCM_SHA384" },
    unsubscribe: newsletter ? [`https://${d}/unsubscribe?u=sam`, `mailto:unsubscribe@${d}?subject=unsubscribe`] : [],
    headersFetched: true,
    headersError: null,
  };
}

function mockSource(m: MessageView): string {
  const d = domainOf(m.from.email);
  const date = new Date(m.date).toUTCString().replace("GMT", "+0000");
  const head = [
    `Delivered-To: ${m.to[0]?.email ?? ""}`,
    `Received: from mail-a.${d} (mail-a.${d}. [192.0.2.10])`,
    `        by mx.google.com with ESMTPS id x1si123456`,
    `        (version=TLS1_3 cipher=TLS_AES_256_GCM_SHA384 bits=256/256);`,
    `        ${date}`,
    `Authentication-Results: mx.google.com;`,
    `       dkim=pass header.i=@${d} header.s=s1;`,
    `       spf=pass (google.com: domain of ${m.from.email} designates 192.0.2.10 as permitted sender) smtp.mailfrom=${m.from.email};`,
    `       dmarc=pass (p=QUARANTINE) header.from=${d}`,
    `From: ${rfcAddr(m.from)}`,
    `To: ${m.to.map(rfcAddr).join(", ")}`,
    ...(m.cc.length ? [`Cc: ${m.cc.map(rfcAddr).join(", ")}`] : []),
    `Subject: ${m.subject}`,
    `Date: ${date}`,
    `Message-ID: ${mockMsgId(m)}`,
    `MIME-Version: 1.0`,
    `Content-Type: multipart/alternative; boundary="000000000000b0undary"`,
  ];
  return `${head.join("\r\n")}\r\n\r\n--000000000000b0undary\r\nContent-Type: text/plain; charset="UTF-8"\r\n\r\n${m.bodyText}\r\n\r\n--000000000000b0undary\r\nContent-Type: text/html; charset="UTF-8"\r\n\r\n<div dir="ltr"><p>${m.bodyText.replace(/</g, "&lt;")}</p><script>alert("shown as text, never run")</script></div>\r\n--000000000000b0undary--\r\n`;
}
const bodiesLoaded = new Set<string>();
const bodiesLoading = new Set<string>();

export const mailHandlers: Record<string, MockHandler> = {
  list_accounts: () => {
    startSyncSimulation();
    return accounts;
  },
  sync_status: ({ accountIds }) => {
    startSyncSimulation();
    return Object.values(sync)
      .filter((s) => !accountIds || (accountIds as string[]).includes(s.accountId))
      .map((s) => ({ ...s }));
  },
  sync_now: () => simulateSyncNow(),
  list_labels: ({ accountId, accountIds }) => labelsFor(accountId ?? null, accountIds ?? null),
  list_threads: ({ query }) => {
    const q = query as ListQuery;
    const custom = mockViews[q.view.kind];
    if (custom) return custom(q);
    const out: ThreadSummary[] = [];
    // Snoozed: one page, soonest wake first (like penguin-core).
    const snoozedView = q.view.kind === "snoozed";
    if (snoozedView && q.before != null) return out;
    const rows = snoozedView
      ? sorted()
          .filter((s) => snoozeOf(s.accountId, s.threadId))
          .sort((a, b) => snoozeOf(a.accountId, a.threadId)!.wakeAt - snoozeOf(b.accountId, b.threadId)!.wakeAt)
      : sorted();
    for (const s of rows) {
      if (q.accountId && s.accountId !== q.accountId) continue;
      if (q.accountIds && !q.accountIds.includes(s.accountId)) continue;
      if (!snoozedView && q.before != null && s.lastDate >= q.before) continue;
      if (q.unreadOnly && !s.unread) continue;
      if (!inView(s, q.view, q.view.kind === "inbox" ? q.tab : null)) continue;
      const z = snoozeOf(s.accountId, s.threadId);
      out.push({ ...s, labelIds: [...s.labelIds], participants: [...s.participants], snoozedUntil: z?.wakeAt ?? null });
      if (!snoozedView && out.length >= q.limit) break;
    }
    return out;
  },
  get_thread: ({ accountId, threadId }) => getThread(accountId, threadId),
  // Like the backend: headers-only originals are downloaded first (~1 s);
  // ?mock=quoteFails (or localStorage penguin.mock.quoteFails=1) makes that fail.
  quote_sources: async ({ accountId, messageIds, withBody }) => {
    const out: QuoteSource[] = [];
    for (const id of messageIds as string[]) {
      const threadId = id.replace(/-m\d+$/, "");
      const k = key(accountId, threadId);
      const t = threads.get(k);
      const v = t ? mockThreadMessages(t).find((m) => m.id === id) : undefined;
      if (!t || !v) throw { code: "notFound", message: "The original message is no longer here" };
      if (t.summary.lastDate < NOW - SYNC_WINDOW && !bodiesLoaded.has(k)) {
        await new Promise((r) => setTimeout(r, 1200));
        if (mockFlag("penguin.mock.quoteFails")) throw { code: "network", message: "Couldn't reach Gmail. Check your connection." };
        bodiesLoaded.add(k);
        emit("penguin://mail-changed", { accountId, threadIds: [threadId] });
      }
      const html = withBody ? (composeHandlers.sanitize_compose_html({ html: v.html }) as string) : null;
      out.push({ messageId: v.id, html, text: withBody ? v.bodyText : null, attachments: v.attachments, inlineImages: [] });
    }
    return out;
  },
  load_remote_images: ({ accountId, messageId }) => {
    for (const t of threads.values()) {
      const v = t.summary.accountId === accountId ? t.views?.find((m) => m.id === messageId) : undefined;
      if (v) return withPrivacy(v, true);
    }
    const threadId = String(messageId).replace(/-m\d+$/, "");
    const t = threads.get(key(accountId, threadId));
    if (!t) throw new Error("mock: unknown message");
    const msgs = messagesOf(t);
    const i = Number(/-m(\d+)$/.exec(messageId)?.[1] ?? 0);
    return messageView(t, msgs[i], i, true);
  },
  modify_threads: ({ targets, action }) => {
    // ?mockFail=modify: every action is refused up front, as when the account
    // can't be reached (the UI must put its optimistic change back).
    if (typeof location !== "undefined" && new URLSearchParams(location.search).get("mockFail") === "modify")
      throw { code: "network", message: "Couldn't reach Gmail. Check your connection." };
    const byAccount = new Map<string, string[]>();
    for (const ref of targets as ThreadRef[]) {
      const t = threads.get(key(ref.accountId, ref.threadId));
      if (!t) continue;
      apply(t.summary, action as ThreadAction);
      reconcileSnooze(ref.accountId, ref.threadId, t.summary.labelIds);
      byAccount.set(ref.accountId, [...(byAccount.get(ref.accountId) ?? []), ref.threadId]);
    }
    for (const [accountId, threadIds] of byAccount) emit("penguin://mail-changed", { accountId, threadIds });
  },
  save_attachment: ({ accountId, messageId, attachmentId }) => {
    const view = getThread(accountId, String(messageId).replace(/-m\d+$/, ""));
    const fromViews = [...threads.values()].flatMap((t) => (t.summary.accountId === accountId ? t.views ?? [] : []));
    const msg = view?.messages.find((m) => m.id === messageId) ?? fromViews.find((m) => m.id === messageId);
    const a = msg?.attachments.find((x) => x.id === attachmentId);
    if (!a) throw { code: "notFound", message: "Attachment not found" };
    return `/Users/sam/Downloads/${a.filename}`;
  },
  prepare_attachment_drag: ({ attachmentId }) => ({ path: `/Users/sam/Library/Caches/penguin/drag-out/mock/0/${String(attachmentId)}`, name: String(attachmentId) }),
  // The mock has no pasteboard: it answers as the Mac app does.
  copy_attachment_file: ({ attachmentId }) => ({ path: `/Users/sam/Library/Caches/penguin/drag-out/mock/1/${String(attachmentId)}`, name: String(attachmentId) }),
  preview_outgoing_file: ({ filename, mimeType, dataBase64 }) => mockOutgoingPreview(String(filename), String(mimeType), String(dataBase64)),
  save_attachment_as: () => null,
  open_path: () => undefined,
  person_summary: ({ email }) => mockPersonSummary(String(email)),
  suggest_recipients: ({ query, limit }) => mockSuggestRecipients(String(query ?? ""), Number(limit ?? 8)),
  get_message_details: ({ accountId, messageId }) => {
    const m = findMockMessage(accountId, messageId);
    if (!m) throw { code: "notFound", message: "message not found" };
    return mockDetails(m);
  },
  get_message_source: ({ accountId, messageId }) => {
    const m = findMockMessage(accountId, messageId);
    if (!m) throw { code: "notFound", message: "message not found in Gmail" };
    return mockSource(m);
  },
  preview_attachment: ({ accountId, messageId, attachmentId }) => {
    const view = getThread(accountId, String(messageId).replace(/-m\d+$/, ""));
    const fromViews = [...threads.values()].flatMap((t) => (t.summary.accountId === accountId ? t.views ?? [] : []));
    const msg = view?.messages.find((m) => m.id === messageId) ?? fromViews.find((m) => m.id === messageId);
    const a = msg?.attachments.find((x) => x.id === attachmentId);
    if (!a) throw { code: "notFound", message: "Attachment not found" };
    return mockPreview(a);
  },
  remove_account: () => undefined,
  update_account: ({ accountId, patch }) => {
    const a = accounts.find((x) => x.id === accountId);
    if (!a) throw { code: "notFound", message: `unknown account ${accountId}` };
    const p = patch as import("../types").AccountPatch;
    if (p.nickname !== undefined) a.nickname = p.nickname?.split(/\s+/).filter(Boolean).join(" ") || null;
    if (p.color !== undefined) {
      if (!/^#[0-9a-f]{6}$/i.test(p.color)) throw { code: "invalidInput", message: `"${p.color}" isn't a #rrggbb color.` };
      a.color = p.color;
    }
    return { ...a };
  },
  open_external: ({ url }) => {
    window.open(url, "_blank", "noopener");
  },
};

// ---------------------------------------------------------------------------
// Settings + diagnostics (OWNER: settings agent). In-memory only; mirrors
// src-tauri/src/settings.rs defaults and the diagnostics payload shape.
// ---------------------------------------------------------------------------
function devQuery(name: string): string | null {
  return typeof location === "undefined" ? null : new URLSearchParams(location.search).get(name);
}
// ?nick=1 gives the mock accounts nicknames (Settings → Accounts screenshots).
if (devQuery("nick") === "1") {
  const nicks = ["Sam NW", "Harbor Hello", "Sam Home"];
  accounts.forEach((a, i) => (a.nickname = nicks[i] ?? null));
}

const mockSettings: import("../types").Settings = {
  theme: "system",
  density: "compact",
  // ?listStyle=cards presets the list style for screenshots.
  listStyle: (devQuery("listStyle") as import("../types").ListStyle | null) ?? "quiet",
  // ?splits=1 starts with the Split Inbox on (screenshots).
  inboxTabs: devQuery("splits") === "1",
  inboxSplits: [
    { id: "important", name: "Important", query: "is:important -is:newsletter -has:invite", hideWhenEmpty: false },
    { id: "calendar", name: "Calendar", query: "has:invite OR from:calendar-notification@google.com OR from:@calendly.com", hideWhenEmpty: false },
    { id: "news", name: "News", query: "is:newsletter", hideWhenEmpty: false },
  ],
  getToZero: true,
  zeroCelebration: true,
  remoteImages: "ask",
  trustedImageSenders: [],
  blockTrackingPixels: devQuery("pixels") !== "allow",
  stripLinkTracking: devQuery("links") === "clean",
  requestReadReceipts: devQuery("receipts") === "on",
  swipeRight: "toggleRead",
  swipeLeft: "archive",
  swipeLeftLong: "trash",
  // Dev screenshots can preset these: ?sidebarTheme=midnight&matchAccent=1
  sidebarTheme: (devQuery("sidebarTheme") as import("../types").SidebarTheme | null) ?? "graphite",
  matchAccent: devQuery("matchAccent") === "1",
  // ?darkShade=navy presets the dark mode shade.
  darkShade: (devQuery("darkShade") as import("../types").DarkShade | null) ?? "black",
  // ?accent=green&corners=square preset the accent color and corner style.
  accentColor: (devQuery("accent") as import("../types").AccentColor | null) ?? "blue",
  corners: (devQuery("corners") as import("../types").CornerStyle | null) ?? "rounded",
  // ?darkBodies=1 turns on the experimental dark email bodies.
  darkEmailBodies: devQuery("darkBodies") === "1",
  // ?floe=1 starts in Floe mode; ?composeFont=literata presets the composer font.
  floeMode: devQuery("floe") === "1",
  composeFont: (devQuery("composeFont") as import("../types").ComposeFont | null) ?? "inter",
  composeFontSize: 15,
  // ?sidebarText=-1 presets it for screenshots.
  sidebarTextSize: Number(devQuery("sidebarText") ?? 0),
  followUpDays: 3,
  snippets: [
    { id: "thanks", trigger: "thx", title: "Thanks + next steps", body: "Thanks, {first_name}. Next steps on my side: {cursor}. I'll follow up by {day}.\n\n{my_name}", uses: 0, subject: "", cc: [], bcc: [], attachments: [] },
    { id: "review", trigger: "review", title: "Review checklist", body: "could you review the deck ahead of time and flag anything blocking by {day before}? That keeps the session focused on decisions.", uses: 0, subject: "", cc: [], bcc: [], attachments: [] },
    {
      id: "pricing",
      trigger: "pricing",
      title: "Pricing sheet",
      body: "Hi {first_name},\n\nThanks for asking about pricing for {company}. The current sheet is attached; the team plan is usually the right fit at your size. {cursor}\n\nBest,\n{my_first_name}",
      uses: 4,
      subject: "Northwind pricing",
      cc: ["Priya Raman <priya@northwind.example>"],
      bcc: [],
      attachments: [{ id: "c".repeat(64), filename: "Northwind pricing 2026.pdf", mimeType: "application/pdf", size: 184_320 }],
    },
  ],
  undoSendSeconds: 10,
  sendLaterHour: 8,
  // ?instant=off hides instant replies; ?aiReplies=1 turns on the on-device suggestions.
  instantReplies: {
    enabled: devQuery("instant") !== "off",
    replies: ["Sounds good, thanks!", "Thanks, got it.", "Let me check and get back to you."],
    aiSuggestions: devQuery("aiReplies") === "1",
  },
  writeWithAi: devQuery("writeAi") !== "off",
  checkSpelling: devQuery("spelling") !== "off",
  checkGrammar: devQuery("grammar") === "on",
  // Composer signatures (?sig=off starts with none).
  signatures:
    devQuery("sig") === "off"
      ? []
      : [
          { id: "sig-nw", name: "Northwind", html: "<p><strong>Sam Okafor</strong></p><p>Product Lead · Northwind</p><p><a href=\"https://northwind.example\">northwind.example</a></p>" },
          { id: "sig-short", name: "Short", html: "<p>— Sam</p>" },
        ],
  signatureDefaults: devQuery("sig") === "off" ? {} : { [ACC.nw]: "sig-nw", [ACC.pe]: "sig-short" },
  signatureInsert: { newMessages: true, replies: true, forwards: true },
  signatureSeparator: devQuery("sigdash") === "1",
  lockReplyAccount: true,
  gmailUnitsPerMin: 6000,
  syncWindowMonths: 6,
  olderMail: "headers",
  // Off by default, as in the app; ?agents=read|draft|send starts at that level.
  mcp: (() => {
    const q = devQuery("agents");
    const access: import("../types").AgentAccess = q === "read" || q === "draft" || q === "send" ? q : "off";
    return { access, enabled: access !== "off", sendDelaySeconds: 60 as const, sendKnownOnly: true };
  })(),
  // Off by default, as in the app; dev screenshots with key caps: ?hints=on
  showShortcutHints: devQuery("hints") === "on",
  shortcutCoach: devQuery("coach") !== "off",
  unsubscribeButton: devQuery("unsub") !== "off",
  semanticSearch: devQuery("semantic") !== "off",
  summaries: true,
  askWithAi: true,
  senderPhotos: { contacts: true, bimi: true, favicons: true, gravatar: false },
  // ?avatars=list|message|both|off presets where sender photos appear.
  avatarPlacement: (devQuery("avatars") as import("../types").AvatarPlacement | null) ?? "both",
  // Settings → You; ?me=photo starts with a photo (lib/mock/me.ts).
  calendar: { pastMonths: 24, futureMonths: 12, nextUp: true, connectOnSignIn: true },
  // Dev screenshots: ?notify=1 starts with new-mail notifications on.
  notifications: { enabled: devQuery("notify") === "1", accounts: {}, knownSendersOnly: false },
  me: { name: "", title: "", company: "", signOff: "", photo: devQuery("me") === "photo" ? "mock-photo" : null },
  // Dev screenshots: ?pending=1 starts with two unfinished Add account setups.
  pendingSetups:
    devQuery("pending") === "1"
      ? [
          { email: "sam@yahoo.example", kind: "yahoo", step: "pw-connect", startedAt: Date.now() - 3_600_000, lastError: "Yahoo refused the app password." },
          { email: "sam@fastmail.example", kind: "fastmail", step: "pw-create", startedAt: Date.now() - 86_400_000, lastError: null },
        ]
      : [],
  // Dev screenshots: ?hideAll=1 leaves the personal account out of All accounts.
  hiddenFromAll: devQuery("hideAll") === "1" ? [ACC.pe] : [],
  // Sidebar drag order; ?order=pe starts with the personal account first.
  accountOrder: devQuery("order") === "pe" ? [ACC.pe] : [],
  // Smart views (features/smart): all off, like the backend. ?views=demo turns
  // several on for screenshots (lib/mock/smart.ts).
  smartViews:
    devQuery("views") === "demo"
      ? {
          shown: ["receipts", "travel", "packages", "bills", "reservations", "subscriptions", "files", "custom:vip"],
          counts: ["travel", "packages", "bills"],
          custom: [{ id: "vip", name: "From Priya", query: "from:priya" }],
        }
      : { shown: [], counts: [], custom: [] },
  // ?welcome=1 opens the Welcome setup as on a new install (features/welcome), with the model download held back.
  welcomeCompleted: devQuery("welcome") !== "1",
  // Account profiles over the mock accounts (an account may be in several).
  profiles: [
    { id: "p-work", name: "Work", color: "#4F7CFF", accountIds: [ACC.nw, ACC.hl], emoji: "💼" },
    { id: "p-northwind", name: "Northwind", color: "#B06AD9", accountIds: [ACC.nw], emoji: null },
    { id: "p-home", name: "Home", color: "#E3A13B", accountIds: [ACC.pe, ACC.fm], emoji: "🏡" },
  ],
};

/** A fresh copy, as the real backend returns (members that aren't accounts pruned). */
function settingsOut(): import("../types").Settings {
  const known = new Set(accounts.map((a) => a.id));
  return {
    ...mockSettings,
    trustedImageSenders: [...mockSettings.trustedImageSenders],
    profiles: mockSettings.profiles.map((p) => ({ ...p, accountIds: p.accountIds.filter((id) => known.has(id)) })),
    hiddenFromAll: mockSettings.hiddenFromAll.filter((id) => known.has(id)),
    accountOrder: mockSettings.accountOrder.filter((id) => known.has(id)),
    pendingSetups: mockSettings.pendingSetups.filter((p) => !known.has(p.email)),
    inboxSplits: mockSettings.inboxSplits.map((x) => ({ ...x })),
    smartViews: {
      shown: [...mockSettings.smartViews.shown],
      counts: [...mockSettings.smartViews.counts],
      custom: mockSettings.smartViews.custom.map((c) => ({ ...c })),
    },
  };
}

Object.assign(mailHandlers, {
  get_settings: () => settingsOut(),
  update_settings: ({ patch: rawPatch }) => {
    // `mcp` merges field by field and never raises the level to send
    // (settings.rs McpPatch); that takes enable_agent_send.
    const { mcp, ...patch } = rawPatch as import("../types").SettingsPatch;
    if (mcp?.access === "send" && mockSettings.mcp.access !== "send")
      throw { code: "invalidInput", message: "Allowing agents to send needs the confirmation in Settings → Developer → Agents" };
    if (mcp) {
      const access = mcp.access ?? mockSettings.mcp.access;
      if (mockSettings.mcp.access === "send" && access !== "send") mockAgentPending.length = 0; // a downgrade cancels queued agent sends
      mockSettings.mcp = { ...mockSettings.mcp, ...mcp, access, enabled: access !== "off" };
    }
    // `me` merges field by field, as in the backend (the photo only via lib/mock/me.ts).
    Object.assign(mockSettings, patch, patch.me ? { me: { ...mockSettings.me, ...patch.me } } : {});
    // Normalized like settings.rs: trimmed, lowercased, first occurrence kept.
    if (patch.followUpDays !== undefined) mockSettings.followUpDays = Math.min(14, Math.max(1, Math.round(Number(patch.followUpDays)) || 3));
    if (patch.accountOrder)
      mockSettings.accountOrder = [...new Set<string>(patch.accountOrder.map((id: string) => id.trim().toLowerCase()).filter(Boolean))];
    if (patch.smartViews) mockSettings.smartViews = normalizeSmartViews(patch.smartViews);
    // As settings.rs normalize_inbox_splits: valid ids, no empty query, names trimmed, ≤12.
    if (patch.inboxSplits)
      mockSettings.inboxSplits = (patch.inboxSplits as import("../types").InboxSplit[])
        .map((x) => ({ ...x, query: x.query.trim().replace(/\s+/g, " "), name: (x.name.trim().replace(/\s+/g, " ") || x.query.trim()).slice(0, 30) }))
        .filter((x, i, all) => /^[A-Za-z0-9_-]{1,64}$/.test(x.id) && x.query && all.findIndex((y) => y.id === x.id) === i)
        .slice(0, 12);
    const out = settingsOut();
    emit("penguin://settings-changed", out);
    return out;
  },
  diagnostics: (): import("../types").Diagnostics => {
    const root = "/Users/sam/Library/Application Support/co.gluska.penguin";
    const perAccount = accounts.map((a, i) => {
      const s = sync[a.id];
      return {
        accountId: a.id,
        email: a.email,
        phase: s.phase,
        messagesStored: s.indexed,
        headersOnlyMessages: Math.round(s.indexed * 0.6),
        gmailTotal: s.totalEstimate,
        backfillDone: s.phase !== "backfilling",
        historyIdPresent: true,
        failedMessageIds: i === 0 ? 2 : 0,
        lastSyncedAt: s.lastSyncedAt,
        msgsPerMinute: s.phase === "backfilling" ? 18_400 : 0,
        keychain: "present" as const,
        quota:
          i === 0
            ? {
                scope: "perAccount" as const,
                unitsPerMin: 6_000,
                getCost: 12.1,
                getsPerSec: 7.8,
                getsLastMin: 452,
                unitsLastMin: 5_470,
                throttleEpisodes: 1,
                accountGetsLastMin: 151,
                activeAccounts: 3,
                accountGetsPerSec: 2.6,
              }
            : null,
        inlineCacheBytes: [4_812_331, 1_204_992, 388_120][i] ?? 0,
        error: s.error,
      };
    });
    const total = perAccount.reduce((n, a) => n + a.messagesStored, 0);
    return {
      appVersion: "0.1.0",
      osVersion: "macOS 26.0.1",
      dataDir: root,
      configDir: root,
      cacheDir: "/Users/sam/Library/Caches/co.gluska.penguin",
      logDir: "/Users/sam/Library/Logs/co.gluska.penguin",
      dbPath: `${root}/penguin.db`,
      dbBytes: Math.round(total * 6_900),
      walBytes: 19_170_392,
      pageSize: 4096,
      pageCount: Math.round((total * 6_900) / 4096),
      freelistCount: 112,
      totalMessages: total,
      totalThreads: Math.round(total * 0.62),
      inlineCacheBytes: perAccount.reduce((n, a) => n + a.inlineCacheBytes, 0),
      logBytes: 1_482_004,
      oauthClientIdTail: "ps.googleusercontent.com".slice(-12),
      accounts: perAccount,
      trackersRemovedSession: 37,
      gmailUnitsEnvOverride: null,
      tookMs: 3.2,
    };
  },
  diagnostics_table_sizes: (): import("../types").TableSizes => ({
    method: "dbstat",
    tookMs: 41.7,
    tables: [
      { name: "message_bodies", kind: "table", bytes: 214_302_720 },
      { name: "messages_fts", kind: "fts", bytes: 131_026_944 },
      { name: "messages", kind: "table", bytes: 38_211_584 },
      { name: "thread_views", kind: "table", bytes: 12_902_400 },
      { name: "threads", kind: "table", bytes: 11_632_640 },
      { name: "sqlite_autoindex_messages_1", kind: "index", bytes: 6_918_144 },
      { name: "attachments", kind: "table", bytes: 3_026_944 },
      { name: "people", kind: "table", bytes: 1_011_712 },
    ],
  }),
  reveal_path: () => undefined,
  optimize_index: () => new Promise((r) => setTimeout(r, 400)),
} satisfies Record<string, MockHandler>);

// Browser sign-in (add_account in ./search.ts, reconnect_account here;
// OWNER: settings agent). Each publishes its link like the backend
// (penguin://sign-in-url) and can be cancelled with cancel_sign_in.
// ?mockBrowser=fail pretends the browser couldn't be opened; ?mockSignIn=wait
// keeps waiting in the "browser" until cancelled; ?mockReconnect=fail makes
// reconnect error.
let mockSignIn: { reject: (e: unknown) => void } | null = null;
let mockLink: import("../types").SignInLink | null = null;

/** Resolves after `ms` as if the user finished in the browser (never with ?mockSignIn=wait); rejects `cancelled` on cancel_sign_in. */
export function mockBrowserSignIn(hint: string | null, ms = 1500): Promise<void> {
  mockSignIn?.reject({ code: "cancelled", message: "Sign-in cancelled" });
  return new Promise((resolve, reject) => {
    const me = { reject };
    mockSignIn = me;
    const url = `https://accounts.google.com/o/oauth2/v2/auth?client_id=mock.apps.googleusercontent.com&redirect_uri=http%3A%2F%2F127.0.0.1%3A49152%2Fcallback&response_type=code&state=mock${hint ? `&login_hint=${encodeURIComponent(hint)}` : ""}`;
    mockLink = { url, error: null };
    emit("penguin://sign-in-url", mockLink);
    if (devQuery("mockBrowser") === "fail") {
      mockLink = { url, error: "launcher exited with status 1" };
      emit("penguin://sign-in-url", mockLink);
    }
    if (devQuery("mockSignIn") === "wait") return;
    setTimeout(() => {
      if (mockSignIn !== me) return;
      mockSignIn = null;
      mockLink = null;
      resolve();
    }, ms);
  });
}

Object.assign(mailHandlers, {
  sign_in_link: () => (mockSignIn ? mockLink : null),
  reopen_sign_in: () => {
    if (!mockSignIn || !mockLink) throw { code: "invalidInput", message: "No sign-in is waiting for the browser. Start it again." };
    if (devQuery("mockBrowser") === "fail")
      throw { code: "other", message: "Penguin couldn't open your browser (launcher exited with status 1)." };
    window.open(mockLink.url, "_blank", "noopener");
  },
  reconnect_account: async ({ accountId }) => {
    await mockBrowserSignIn(accounts.find((a) => a.id === accountId)?.email ?? null);
    if (devQuery("mockReconnect") === "fail") {
      throw { code: "invalidInput", message: "You signed in as someone@else.example; choose " + accountId + " instead" };
    }
    const s = sync[accountId];
    if (s) {
      Object.assign(s, { phase: "idle", error: null, lastSyncedAt: Date.now() });
      emit("penguin://sync-status", { ...s });
    }
    return accounts.find((a) => a.id === accountId);
  },
  cancel_sign_in: () => {
    const pending = mockSignIn;
    mockSignIn = null;
    mockLink = null;
    pending?.reject({ code: "cancelled", message: "Sign-in cancelled" });
    return !!pending;
  },
} satisfies Record<string, MockHandler>);

// Broken-sync states for screenshots (OWNER: settings agent):
// ?mockSync=broken puts Northwind in error (a panicked task) and Harbor Labs
// in needsReauth.
if (typeof location !== "undefined" && new URLSearchParams(location.search).get("mockSync") === "broken") {
  sync[ACC.nw].indexed = TOTAL_NW;
  mockRecordFailure(sync[ACC.nw], "internal", "internal sync error (index out of bounds: the len is 0 but the index is 0); restart sync to retry", NOW - 60_000, null);
  mockRecordFailure(sync[ACC.hl], "auth", "account needs to sign in again: token revoked", NOW - 60_000, null);
}

// A flaky server for the "Can't reach …" states (lib/syncHealth.ts), on the
// Fastmail (IMAP) account. The mock engine retries on the engines' backoff
// (sped up 4× so the states come round quickly):
//   ?mockSync=retrying  one failed attempt, retried quietly; the retry works
//   ?mockSync=failing   3 failed attempts: the alert with its actions
//   ?mockSync=hidden    the same alert, hidden for 6 hours on this device
//   ?mockSync=recovered back in sync after a streak that had alerted
//   ?mockSync=outage    healthy, then the server goes away: quiet tries, then the alert
// ?mockRetry=fail makes Retry now (and the engine's own retries) fail too;
// otherwise a retry works (in failing/hidden: Retry now works, the engine's
// own retries keep failing, so the alert stays until you act).
const SCALE = 0.25;
const FLAKY_ERROR = "network: couldn't reach imap.fastmail.com:993: operation timed out";
const flakyMode = devQuery("mockSync");
const retryFails = devQuery("mockRetry") === "fail";
const attemptTimers = new Map<string, ReturnType<typeof setTimeout>>();

/** The mock engine's next attempt for a failing account, at its nextRetryAt. */
function scheduleAttempt(accountId: string, works: () => boolean) {
  clearTimeout(attemptTimers.get(accountId));
  const s = sync[accountId];
  const at = s?.failure?.nextRetryAt;
  if (!s || !at) return;
  attemptTimers.set(
    accountId,
    setTimeout(() => {
      attemptTimers.delete(accountId);
      const now = Date.now();
      if (works()) mockRecordProgress(s, now);
      else {
        mockRecordFailure(s, "network", FLAKY_ERROR, now, mockBackoff((s.failure?.count ?? 0) + 1, SCALE));
        scheduleAttempt(accountId, works);
      }
      emit("penguin://sync-status", { ...s });
    }, Math.max(0, at - Date.now())),
  );
}

if (flakyMode === "retrying" || flakyMode === "failing" || flakyMode === "hidden" || flakyMode === "recovered") {
  const s = sync[ACC.fm];
  const now = Date.now();
  if (flakyMode === "recovered") {
    for (const ago of [150_000, 140_000, 120_000]) mockRecordFailure(s, "network", FLAKY_ERROR, now - ago, 20_000);
    mockRecordProgress(s, now - 80_000);
  } else if (flakyMode === "retrying") {
    mockRecordFailure(s, "network", FLAKY_ERROR, now - 2_000, 10_000);
    scheduleAttempt(ACC.fm, () => !retryFails);
  } else {
    for (const ago of [45_000, 35_000, 15_000]) mockRecordFailure(s, "network", FLAKY_ERROR, now - ago, 40_000);
    scheduleAttempt(ACC.fm, () => false);
    if (flakyMode === "hidden") hideSyncAlert(s);
  }
} else if (flakyMode === "outage") {
  // Healthy for 3 s, then the server stops answering until Retry now.
  setTimeout(() => {
    const s = sync[ACC.fm];
    mockRecordFailure(s, "network", FLAKY_ERROR, Date.now(), mockBackoff(1, SCALE));
    emit("penguin://sync-status", { ...s });
    scheduleAttempt(ACC.fm, () => false);
  }, 3_000);
}

Object.assign(mailHandlers, {
  // Like the backends: the failure stays (retrying now) until the attempt
  // syncs or fails again; a signed-out account can't be retried into health.
  retry_account_sync: ({ accountId }) => {
    const s = sync[accountId];
    if (!s || !s.failure) return undefined;
    clearTimeout(attemptTimers.get(accountId));
    mockRetryNow(s, Date.now());
    emit("penguin://sync-status", { ...s });
    return new Promise<void>((resolve) => {
      resolve();
      setTimeout(() => {
        const now = Date.now();
        if (s.phase === "needsReauth") mockRecordFailure(s, s.failure?.kind ?? "auth", s.error ?? "account needs to sign in again", now, null);
        else if (retryFails) {
          mockRecordFailure(s, s.failure?.kind ?? "network", s.error ?? FLAKY_ERROR, now, mockBackoff((s.failure?.count ?? 0) + 1, SCALE));
          scheduleAttempt(accountId, () => false);
        } else mockRecordProgress(s, now);
        emit("penguin://sync-status", { ...s });
      }, 1_200);
    });
  },
} satisfies Record<string, MockHandler>);

// Appended by ui-search for the drafts mock (mock/search.ts): a deleted or
// sent draft that was alone in its thread takes the thread with it.
export function removeMockThread(accountId: string, threadId: string) {
  threads.delete(key(accountId, threadId));
}

// Label management for mock/menus.ts (update_label / delete_label).
/** Patch a mock user label in place; null when the account has no such user label. */
export function updateMockLabel(
  accountId: string,
  labelId: string,
  patch: { name?: string; color?: string | null; hidden?: boolean },
): Label | null {
  const def = USER_LABELS.find((l) => ACC[l.acc] === accountId && l.id === labelId);
  if (!def) return null;
  if (patch.name !== undefined) def.name = patch.name;
  if (patch.color !== undefined) def.color = patch.color;
  if (patch.hidden !== undefined) def.hidden = patch.hidden;
  return labelsFor(accountId).find((l) => l.id === labelId) ?? null;
}

/** Delete a mock user label and strip it from the account's threads; returns the touched thread ids, or null if unknown. */
export function deleteMockLabel(accountId: string, labelId: string): string[] | null {
  const i = USER_LABELS.findIndex((l) => ACC[l.acc] === accountId && l.id === labelId);
  if (i < 0) return null;
  USER_LABELS.splice(i, 1);
  const touched: string[] = [];
  for (const t of threads.values()) {
    const s = t.summary;
    if (s.accountId !== accountId || !s.labelIds.includes(labelId)) continue;
    s.labelIds = s.labelIds.filter((l) => l !== labelId);
    touched.push(s.threadId);
  }
  return touched;
}

// MCP + command-line tool (OWNER: settings agent). ?mockCli=installed|offpath
// shows those states; the default is a build without penguin-cli.
Object.assign(mailHandlers, {
  mcp_info: (): import("../types").McpInfo => ({
    enabled: mockSettings.mcp.enabled,
    access: mockSettings.mcp.access,
    cliPath: "/Applications/Penguin.app/Contents/MacOS/penguin-cli",
    claudeCodeCommand: "claude mcp add penguin -- /Applications/Penguin.app/Contents/MacOS/penguin-cli mcp",
    claudeDesktopConfig:
      '{\n  "mcpServers": {\n    "penguin": {\n      "command": "/Applications/Penguin.app/Contents/MacOS/penguin-cli",\n      "args": ["mcp"]\n    }\n  }\n}',
    sshCommand: "claude mcp add penguin -- ssh you@your-mac /Applications/Penguin.app/Contents/MacOS/penguin-cli mcp",
    authorizedKeysPrefix: 'command="/Applications/Penguin.app/Contents/MacOS/penguin-cli mcp",restrict',
    auditLogPath: "~/Library/Logs/co.gluska.penguin/mcp-audit.log",
  }),
  enable_agent_send: ({ acknowledgement }) => {
    if (String(acknowledgement).trim().toLowerCase() !== "i understand")
      throw { code: "invalidInput", message: "Type “I understand” to let agents send" };
    mockSettings.mcp = { ...mockSettings.mcp, access: "send", enabled: true };
    const out = settingsOut();
    emit("penguin://settings-changed", out);
    return out;
  },
  agent_activity: ({ limit }): import("../types").AgentActivity[] =>
    mockSettings.mcp.access === "off" ? [] : mockAgentActivity().slice(0, Number(limit) || 30),
  // The toast's Undo after an agent organized mail: each step through the
  // same mock handlers the UI uses.
  agent_undo: ({ steps }) => {
    for (const s of (steps ?? []) as import("../types").AgentUndoStep[]) {
      const targets = s.arguments.targets;
      const action: Record<string, import("../types").ThreadAction> = {
        archive: { kind: "archive" },
        unarchive: { kind: "moveToInbox" },
        mark_read: { kind: "markRead" },
        mark_unread: { kind: "markUnread" },
        star: { kind: "star" },
        unstar: { kind: "unstar" },
        trash: { kind: "trash" },
        untrash: { kind: "untrash" },
        report_spam: { kind: "reportSpam" },
        not_spam: { kind: "notSpam" },
      };
      if (s.tool === "add_label" && s.arguments.label) mailHandlers.modify_threads({ targets, action: { kind: "addLabel", labelId: s.arguments.label } });
      else if (s.tool === "remove_label" && s.arguments.label) mailHandlers.modify_threads({ targets, action: { kind: "removeLabel", labelId: s.arguments.label } });
      else if (action[s.tool]) mailHandlers.modify_threads({ targets, action: action[s.tool] });
    }
  },
  agent_pending_sends: (): import("../types").AgentPendingSend[] =>
    mockSettings.mcp.access === "send" ? [...mockAgentPending].sort((a, b) => a.sendAt - b.sendAt) : [],
  cli_install_status: (): import("../types").CliLinkStatus => mockCli(),
  install_cli: () =>
    new Promise((resolve) =>
      setTimeout(() => {
        mockCliInstalled = true;
        resolve(mockCli());
      }, 400),
    ),
} satisfies Record<string, MockHandler>);

let mockCliInstalled =
  typeof location !== "undefined" && ["installed", "offpath"].includes(new URLSearchParams(location.search).get("mockCli") ?? "");
function mockCli(): import("../types").CliLinkStatus {
  const offPath = typeof location !== "undefined" && new URLSearchParams(location.search).get("mockCli") === "offpath";
  return {
    linkPath: "~/.local/bin/penguin",
    target: "/Applications/Penguin.app/Contents/MacOS/penguin-cli",
    installed: mockCliInstalled,
    conflict: null,
    onPath: offPath || !mockCliInstalled ? false : true,
    pathLine: "echo 'export PATH=\"$HOME/.local/bin:$PATH\"' >> ~/.zshrc",
  };
}

// Agents (Settings → Developer → Agents): fictional recent activity and, at
// the send level, one send waiting in the outbox. ?agents=send shows both.
const mockAgentPending: import("../types").AgentPendingSend[] =
  devQuery("agents") === "send"
    ? [
        {
          scheduleId: "agent-sched-1",
          accountId: ACC.nw,
          draftId: "agent-draft-1",
          threadId: null,
          sendAt: Date.now() + 45_000,
          to: ["dana.reyes@acme.example"],
          subject: "Re: Q3 vendor review",
        },
      ]
    : [];

function mockAgentActivity(): import("../types").AgentActivity[] {
  const at = (minsAgo: number) => new Date(Date.now() - minsAgo * 60_000).toISOString().replace(/\.\d+Z$/, "Z");
  const row = (p: Partial<import("../types").AgentActivity> & { ts: string; tool: string }): import("../types").AgentActivity => ({
    via: "mcp",
    ok: true,
    errorCode: null,
    account: null,
    recipientCount: null,
    attachmentCount: null,
    draftId: null,
    sendAt: null,
    resultCount: null,
    threadCount: null,
    changedCount: null,
    detail: null,
    ...p,
  });
  const send = mockSettings.mcp.access === "send";
  return [
    ...(send ? [row({ ts: at(0.2), tool: "send_draft", account: "sam@northwind.example", recipientCount: 1, draftId: "agent-draft-1", sendAt: at(-1) })] : []),
    row({ ts: at(0.6), tool: "archive", account: "sam@northwind.example", threadCount: 6, changedCount: 6, resultCount: 6 }),
    row({ ts: at(0.8), tool: "add_label", account: "sam@northwind.example", threadCount: 4, changedCount: 3, resultCount: 3 }),
    row({ ts: at(1), tool: "create_draft", account: "sam@northwind.example", recipientCount: 1, attachmentCount: 1, draftId: "agent-draft-1" }),
    row({ ts: at(1.5), tool: "create_share_link", account: "sam@northwind.example", resultCount: 1 }),
    row({ ts: at(2), tool: "get_attachment", resultCount: 1 }),
    row({ ts: at(2.5), tool: "thread_context", resultCount: 4 }),
    row({ ts: at(3), tool: "search", resultCount: 6, detail: "from:dana vendor review" }),
    row({ ts: at(9), tool: "send_message", via: "cli", ok: false, errorCode: "permissionDenied", account: "sam@harbor-labs.example", recipientCount: 2 }),
    row({ ts: at(14), tool: "trash", via: "cli", ok: false, errorCode: "permissionDenied", account: "sam@northwind.example", threadCount: 30 }),
    row({ ts: at(26), tool: "list_drafts", via: "cli", resultCount: 2 }),
  ];
}

/** cancel_scheduled_send also cancels the agent sends above. */
export function withAgentCancel(inner: MockHandler): MockHandler {
  return (args) => {
    const i = mockAgentPending.findIndex((p) => p.scheduleId === args.id);
    if (i >= 0) {
      mockAgentPending.splice(i, 1);
      return true;
    }
    return inner(args);
  };
}
