// Mock smart views (features/smart), mirroring penguin-core store_smart.rs:
// seeded fictional mail (receipts, trips, parcels, bills, bookings, files;
// .example senders only) with the fact each one carries, and the views,
// header figures and counts computed from them the way the backend does
// (simplified). Dates are relative to now so "upcoming" stays upcoming.
import type { Address, ListQuery, Money, MessageView, SmartCount, SmartRow, SmartStat, SmartViewInfo, ThreadSummary } from "../types";
import type { MockHandler } from "./index";
import { mailHandlers, mockMail, mockViews } from "./mail";
import { mockPendingInvites } from "./invites";

const MIN = 60_000;
const HOUR = 60 * MIN;
const DAY = 24 * HOUR;
const NOW = Date.now();

const ME: Record<string, string> = {
  "acc-northwind": "sam@northwind.example",
  "acc-harbor": "sam@harbor-labs.example",
  "acc-personal": "sam.okafor@gmail.example",
  "acc-okafor": "sam@okafor.example",
};

type Group = "receipts" | "travel" | "packages" | "bills" | "reservations" | "files";

interface Seed {
  group: Group;
  accountId: string;
  id: string;
  from: Address;
  subject: string;
  body: string;
  /** Days before now the email came (fractions allowed). */
  ago: number;
  inbox?: boolean;
  unread?: boolean;
  row: Partial<SmartRow> & { kind: string; title: string };
  files?: { filename: string; mimeType: string; size: number }[];
}

/** A local wall time `days` from today at hh:mm ("2026-10-02T19:05"), or a date. */
function local(days: number, hm?: string): string {
  const d = new Date(NOW + days * DAY);
  const iso = `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, "0")}-${String(d.getDate()).padStart(2, "0")}`;
  return hm ? `${iso}T${hm}` : iso;
}
const usd = (value: number): Money => ({ value, currency: "USD" });
const eur = (value: number): Money => ({ value, currency: "EUR" });

const rydeo = { name: "Rydeo Receipts", email: "receipts@rydeo.example" };
const paperleaf = { name: "Paperleaf", email: "orders@paperleaf.example" };
const hearth = { name: "Hearth & Loom", email: "orders@hearthandloom.example" };
const tunely = { name: "Tunely", email: "receipts@tunely.example" };
const ledgerly = { name: "Ledgerly Billing", email: "billing@ledgerly.example" };
const northline = { name: "Northline Air", email: "reservations@northline.example" };
const ups = { name: "UPS", email: "mcinfo@ups.example" };

const SEEDS: Seed[] = [
  // Receipts (and, repeating, subscriptions).
  { group: "receipts", accountId: "acc-personal", id: "sm-rydeo-1", from: rydeo, subject: "Your Thursday evening trip with Rydeo", body: "Thanks for riding, Sam.\n\nTrip fare $15.10\nBooking fee $3.30\nTotal $18.40", ago: 0.4, inbox: true, unread: true, row: { kind: "receipt", title: "Rydeo", amount: usd(18.4) } },
  { group: "receipts", accountId: "acc-personal", id: "sm-paper-refund", from: paperleaf, subject: "Your refund for order #PL-2291", body: "We've refunded $12.00 for the returned Brass pen to your card ending 4417.", ago: 1.2, row: { kind: "receipt", title: "Paperleaf", amount: usd(-12), reference: "PL-2291", status: "refunded", detail: "Brass pen" } },
  { group: "receipts", accountId: "acc-personal", id: "sm-hearth", from: hearth, subject: "Order HL-5521 confirmed", body: "Thanks for your order!\n\nWool rug 5x8, Oat\nOrder total $249.00", ago: 3, row: { kind: "receipt", title: "Hearth & Loom", amount: usd(249), reference: "HL-5521", detail: "Wool rug 5x8, Oat" } },
  { group: "receipts", accountId: "acc-personal", id: "sm-paper", from: paperleaf, subject: "Order #PL-2291 confirmed", body: "Linen notebook $52.00\nBrass pen $12.00\nOrder total $64.00", ago: 5, row: { kind: "receipt", title: "Paperleaf", amount: usd(64), reference: "PL-2291", detail: "Linen notebook, Brass pen" } },
  { group: "receipts", accountId: "acc-personal", id: "sm-rydeo-2", from: rydeo, subject: "Your Saturday trip with Rydeo", body: "Total $23.40", ago: 6, row: { kind: "receipt", title: "Rydeo", amount: usd(23.4) } },
  { group: "receipts", accountId: "acc-personal", id: "sm-lumo", from: { name: "Café Lumo", email: "hola@cafelumo.example" }, subject: "Tu recibo de Café Lumo", body: "Total 30,00 €", ago: 9, row: { kind: "receipt", title: "Café Lumo", amount: eur(30) } },
  { group: "receipts", accountId: "acc-northwind", id: "sm-ledgerly-1", from: ledgerly, subject: "Receipt for your Ledgerly Pro plan", body: "Ledgerly Pro (monthly)\nAmount charged $49.00", ago: 2, row: { kind: "receipt", title: "Ledgerly", amount: usd(49), reference: "LG-8801", detail: "Ledgerly Pro (monthly)" } },
  { group: "receipts", accountId: "acc-northwind", id: "sm-ledgerly-2", from: ledgerly, subject: "Receipt for your Ledgerly Pro plan", body: "Amount charged $49.00", ago: 32, row: { kind: "receipt", title: "Ledgerly", amount: usd(49), reference: "LG-8712" } },
  { group: "receipts", accountId: "acc-northwind", id: "sm-ledgerly-3", from: ledgerly, subject: "Receipt for your Ledgerly Pro plan", body: "Amount charged $49.00", ago: 62, row: { kind: "receipt", title: "Ledgerly", amount: usd(49), reference: "LG-8630" } },
  { group: "receipts", accountId: "acc-personal", id: "sm-tunely-1", from: tunely, subject: "Your Tunely receipt", body: "Tunely Family\nTotal $9.99", ago: 15, row: { kind: "receipt", title: "Tunely", amount: usd(9.99), detail: "Tunely Family" } },
  { group: "receipts", accountId: "acc-personal", id: "sm-tunely-2", from: tunely, subject: "Your Tunely receipt", body: "Total $9.99", ago: 45, row: { kind: "receipt", title: "Tunely", amount: usd(9.99) } },
  { group: "receipts", accountId: "acc-personal", id: "sm-tunely-3", from: tunely, subject: "Your Tunely receipt", body: "Total $9.99", ago: 75, row: { kind: "receipt", title: "Tunely", amount: usd(9.99) } },
  { group: "receipts", accountId: "acc-personal", id: "sm-grocer", from: { name: "Greenleaf Grocer", email: "orders@greenleaf.example" }, subject: "Your delivery receipt", body: "Order total $84.12", ago: 36, row: { kind: "receipt", title: "Greenleaf Grocer", amount: usd(84.12), reference: "GL-3310" } },
  { group: "receipts", accountId: "acc-okafor", id: "sm-cloudbox-0", from: { name: "Cloudbox", email: "billing@cloudbox.example" }, subject: "Cloudbox annual renewal receipt", body: "Cloudbox 2 TB, 1 year\nTotal $99.00", ago: 555, row: { kind: "receipt", title: "Cloudbox", amount: usd(99), reference: "CB-2025", detail: "Cloudbox 2 TB, 1 year" } },
  { group: "receipts", accountId: "acc-okafor", id: "sm-cloudbox-1", from: { name: "Cloudbox", email: "billing@cloudbox.example" }, subject: "Cloudbox annual renewal receipt", body: "Cloudbox 2 TB, 1 year\nTotal $99.00", ago: 190, row: { kind: "receipt", title: "Cloudbox", amount: usd(99), reference: "CB-2026", detail: "Cloudbox 2 TB, 1 year" } },

  // Travel: a trip to Lisbon, a train later, a past flight.
  { group: "travel", accountId: "acc-personal", id: "sm-fl-out", from: northline, subject: "Your trip to Lisbon: booking QX7P2K", body: "Flight NL 238 San Francisco (SFO) → Lisbon (LIS)\nConfirmation code QX7P2K", ago: 20, row: { kind: "flight", title: "SFO → LIS", at: local(5, "19:05"), end: local(6, "14:40"), reference: "QX7P2K", detail: "NL 238 · Northline Air", amount: usd(1184.6) } },
  { group: "travel", accountId: "acc-personal", id: "sm-stay", from: { name: "Casa Alfama", email: "stay@casa-alfama.example" }, subject: "Booking confirmed: Casa Alfama", body: "Check-in: 3:00 PM\nCheck-out: 11:00 AM\nConfirmation CA-551", ago: 18, row: { kind: "stay", title: "Casa Alfama", at: local(6), end: local(12), reference: "CA-551", detail: "Rua dos Remédios 44, Lisbon" } },
  { group: "travel", accountId: "acc-personal", id: "sm-car", from: { name: "Wayfare Rentals", email: "bookings@wayfare.example" }, subject: "Your rental car is reserved", body: "Pick-up: Lisbon Airport\nConfirmation WF-20931", ago: 17, row: { kind: "car", title: "Wayfare Rentals", at: local(6, "15:30"), end: local(9, "10:00"), reference: "WF-20931", detail: "Lisbon Airport" } },
  { group: "travel", accountId: "acc-personal", id: "sm-fl-back", from: northline, subject: "Your return flight: booking QX7P2K", body: "Flight NL 237 Lisbon (LIS) → San Francisco (SFO)", ago: 20, row: { kind: "flight", title: "LIS → SFO", at: local(12, "11:00"), end: local(12, "15:10"), reference: "QX7P2K", detail: "NL 237 · Northline Air" } },
  { group: "travel", accountId: "acc-northwind", id: "sm-train", from: { name: "Railo", email: "tickets@railo.example" }, subject: "Your train tickets: Porto → Lisbon", body: "Departs 08:10, coach 4, seat 22\nBooking RL-88", ago: 8, row: { kind: "train", title: "Porto → Lisbon", at: local(41, "08:10"), reference: "RL-88", detail: "Coach 4, seat 22" } },
  { group: "travel", accountId: "acc-northwind", id: "sm-fl-past", from: northline, subject: "Your trip to Boston: booking ZZ1111", body: "Flight NL 900 SFO → BOS", ago: 95, row: { kind: "flight", title: "SFO → BOS", at: local(-80, "07:00"), end: local(-80, "15:30"), reference: "ZZ1111", detail: "NL 900 · Northline Air" } },

  // Packages.
  { group: "packages", accountId: "acc-personal", id: "sm-pk-rug", from: ups, subject: "Your package is out for delivery", body: "Tracking number 1Z999AA10123456784", ago: 0.2, inbox: true, unread: true, row: { kind: "parcel", title: "Hearth & Loom", status: "outForDelivery", at: local(0), reference: "1Z999AA10123456784", detail: "UPS · Wool rug 5x8, Oat" } },
  { group: "packages", accountId: "acc-personal", id: "sm-pk-paper", from: { name: "USPS", email: "informed@usps.example" }, subject: "Your Paperleaf order is on its way", body: "Tracking 9400 1000 0000 0000 0000 00", ago: 2, row: { kind: "parcel", title: "Paperleaf", status: "inTransit", at: local(2), reference: "9400100000000000000000", detail: "USPS · Linen notebook" } },
  { group: "packages", accountId: "acc-northwind", id: "sm-pk-books", from: { name: "Lumen Books", email: "shipping@lumenbooks.example" }, subject: "Delivered: your Lumen Books order", body: "Delivered to front door.", ago: 2.5, row: { kind: "parcel", title: "Lumen Books", status: "delivered", reference: "794612345678", detail: "FedEx" } },

  // Bills.
  { group: "bills", accountId: "acc-northwind", id: "sm-bill-ledgerly", from: ledgerly, subject: "Invoice INV-311 from Ledgerly", body: "Amount due $240.00\nDue date: see below", ago: 18, inbox: true, row: { kind: "bill", title: "Ledgerly", amount: usd(240), at: local(-3), reference: "INV-311", status: "due" } },
  { group: "bills", accountId: "acc-personal", id: "sm-bill-bright", from: { name: "Brightwave Energy", email: "billing@brightwave.example" }, subject: "Your September bill is ready", body: "Amount due $84.20\nPayment due by Oct 5", ago: 4, inbox: true, unread: true, row: { kind: "bill", title: "Brightwave Energy", amount: usd(84.2), at: local(8), reference: "BW-0925", status: "due" } },
  { group: "bills", accountId: "acc-harbor", id: "sm-bill-studio", from: { name: "Pier 9 Studios", email: "accounts@pier9studios.example" }, subject: "Studio rent: invoice 1042", body: "Amount due $1,850.00", ago: 6, row: { kind: "bill", title: "Pier 9 Studios", amount: usd(1850), at: local(3), reference: "1042", status: "due" } },
  { group: "bills", accountId: "acc-personal", id: "sm-bill-aqua", from: { name: "Aquafon", email: "billing@aquafon.example" }, subject: "Payment received, thank you", body: "We received your payment of $41.00 for invoice AQ-77.", ago: 7, row: { kind: "bill", title: "Aquafon", amount: usd(41), reference: "AQ-77", status: "paid" } },

  // Reservations.
  { group: "reservations", accountId: "acc-personal", id: "sm-res-mesa", from: { name: "Mesa", email: "book@mesa-sf.example" }, subject: "Your table at Mesa is booked", body: "Table for 2, 8:00 PM", ago: 3, row: { kind: "restaurant", title: "Mesa", at: local(2, "20:00"), reference: "M-4471", detail: "Table for 2" } },
  { group: "reservations", accountId: "acc-personal", id: "sm-res-lanterns", from: { name: "Tixly", email: "orders@tixly.example" }, subject: "Your tickets: The Lanterns", body: "2 tickets, General admission", ago: 12, row: { kind: "event", title: "The Lanterns", at: local(19, "20:30"), reference: "TX-99812", detail: "2 tickets · The Fillmore", amount: usd(96) } },
  { group: "reservations", accountId: "acc-personal", id: "sm-res-past", from: { name: "Osteria Fiore", email: "reservations@osteriafiore.example" }, subject: "Reservation confirmed", body: "Table for 4", ago: 25, row: { kind: "restaurant", title: "Osteria Fiore", at: local(-20, "19:30"), detail: "Table for 4" } },

  // Files.
  { group: "files", accountId: "acc-northwind", id: "sm-f-deck", from: { name: "Priya Raman", email: "priya@lumen-legal.example" }, subject: "Q3 board deck (final)", body: "Final deck attached, plus the appendix.", ago: 1, inbox: true, row: { kind: "file", title: "Q3 board deck.pdf", detail: "+1 more" }, files: [{ filename: "Q3 board deck.pdf", mimeType: "application/pdf", size: 2_400_000 }, { filename: "Appendix.xlsx", mimeType: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", size: 81_000 }] },
  { group: "files", accountId: "acc-personal", id: "sm-f-photos", from: { name: "Dana Whitlock", email: "dana@brightforge.example" }, subject: "Photos from Saturday", body: "A few from the hike.", ago: 4, row: { kind: "file", title: "IMG_2041.jpg", detail: "+2 more" }, files: [{ filename: "IMG_2041.jpg", mimeType: "image/jpeg", size: 3_100_000 }, { filename: "IMG_2042.jpg", mimeType: "image/jpeg", size: 2_900_000 }, { filename: "IMG_2050.jpg", mimeType: "image/jpeg", size: 3_300_000 }] },
  { group: "files", accountId: "acc-harbor", id: "sm-f-budget", from: { name: "Marco Ferri", email: "marco@ferri-instruments.example" }, subject: "Budget 2027 draft", body: "First pass at next year's numbers.", ago: 6, row: { kind: "file", title: "Budget 2027.xlsx" }, files: [{ filename: "Budget 2027.xlsx", mimeType: "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet", size: 120_000 }] },
  { group: "files", accountId: "acc-okafor", id: "sm-f-lease", from: { name: "Juniper Stays", email: "hosts@juniperstays.example" }, subject: "Lease renewal", body: "Please review and sign.", ago: 11, row: { kind: "file", title: "Lease renewal.docx" }, files: [{ filename: "Lease renewal.docx", mimeType: "application/vnd.openxmlformats-officedocument.wordprocessingml.document", size: 64_000 }] },
];

const esc = (s: string) => s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;");

function seedMail() {
  for (const s of SEEDS) {
    const date = NOW - s.ago * DAY;
    const labels = s.inbox ? ["INBOX"] : [];
    const m: MessageView = {
      accountId: s.accountId,
      id: `${s.id}-m0`,
      threadId: s.id,
      date,
      from: s.from,
      to: [{ name: "Sam Okafor", email: ME[s.accountId] }],
      cc: [],
      bcc: [],
      replyTo: [],
      subject: s.subject,
      snippet: s.body.replace(/\s+/g, " ").slice(0, 140),
      bodyText: s.body,
      html:
        `<!doctype html><html><head><meta charset="utf-8"><style>:root{color-scheme:light dark}` +
        `body{margin:0;font:14px/22px Inter,system-ui,sans-serif;color:CanvasText}</style></head><body>` +
        s.body.split("\n").map((p) => `<p>${esc(p)}</p>`).join("") +
        `</body></html>`,
      blockedRemoteImages: 0,
      trackersRemoved: 0,
      trackers: [],
      labelIds: labels,
      attachments: (s.files ?? []).map((f, i) => ({ id: `${s.id}-a${i}`, filename: f.filename, mimeType: f.mimeType, size: f.size, contentId: null, inline: false })),
      unread: !!s.unread,
      starred: false,
      senderAuthenticated: true,
      otp: null,
    };
    mockMail.addThread({ accountId: s.accountId, threadId: s.id, subject: s.subject, labelIds: labels, messages: [m] });
  }
}
seedMail();

// ---------------------------------------------------------------------------
// Views
// ---------------------------------------------------------------------------
interface Item {
  accountId: string;
  threadId: string;
  date: number;
  row: SmartRow;
  active: boolean;
}

const full = (r: Partial<SmartRow> & { kind: string; title: string }): SmartRow => ({
  amount: null,
  at: null,
  end: null,
  reference: null,
  status: null,
  detail: null,
  group: null,
  ...r,
});

function inScope(accountId: string, scope: string[] | null | undefined, one?: string | null) {
  return (!one || one === accountId) && (!scope || scope.includes(accountId));
}

/** Seeds of a group still in the mailbox (not trashed), newest first. */
function seeds(group: Group, scope: string[] | null): (Seed & { date: number })[] {
  return SEEDS.filter((s) => s.group === group && inScope(s.accountId, scope))
    .filter((s) => {
      const t = mockMail.summary(s.accountId, s.id);
      return t && !t.labelIds.includes("TRASH") && !t.labelIds.includes("SPAM");
    })
    .map((s) => ({ ...s, date: NOW - s.ago * DAY }))
    .sort((a, b) => b.date - a.date);
}

const today = () => local(0);
const monthName = (ms: number) => new Date(ms).toLocaleDateString("en-US", { month: "long", year: "numeric" });
const shortDay = (iso: string) => new Date(`${iso.slice(0, 10)}T12:00`).toLocaleDateString("en-US", { month: "short", day: "numeric" });
function span(a: string, b: string): string {
  const [x, y] = [shortDay(a), shortDay(b)];
  if (x === y) return x;
  return x.split(" ")[0] === y.split(" ")[0] ? `${x}–${y.split(" ")[1]}` : `${x} – ${y}`;
}

function receipts(scope: string[] | null): Item[] {
  const month = new Date(NOW).getMonth();
  return seeds("receipts", scope)
    .filter((s) => s.ago <= 400)
    .map((s) => ({
      accountId: s.accountId,
      threadId: s.id,
      date: s.date,
      active: new Date(s.date).getMonth() === month && s.ago < 32,
      row: full({ ...s.row, group: monthName(s.date) }),
    }));
}

const CITY: Record<string, string> = { LIS: "Lisbon", SFO: "San Francisco", BOS: "Boston" };

function travel(scope: string[] | null): Item[] {
  const t = today();
  const all = seeds("travel", scope);
  const up = all.filter((s) => (s.row.at ?? "") >= t).sort((a, b) => (a.row.at ?? "").localeCompare(b.row.at ?? ""));
  const past = all.filter((s) => (s.row.at ?? "") < t).sort((a, b) => (b.row.at ?? "").localeCompare(a.row.at ?? ""));
  const out: Item[] = [];
  let i = 0;
  while (i < up.length) {
    const first = up[i].row.at!.slice(0, 10);
    let last = (up[i].row.end ?? first).slice(0, 10);
    let j = i + 1;
    while (j < up.length) {
      const s = up[j].row.at!.slice(0, 10);
      if (new Date(s).getTime() > new Date(last).getTime() + 2 * DAY) break;
      const e = (up[j].row.end ?? s).slice(0, 10);
      if (e > last) last = e;
      if (s > last) last = s;
      j++;
    }
    const dest = up.slice(i, j).find((s) => s.row.kind === "flight")?.row.title.split(" → ")[1];
    const label = dest ? `Trip to ${CITY[dest] ?? dest} · ${span(first, last)}` : `Upcoming · ${span(first, last)}`;
    for (const s of up.slice(i, j))
      out.push({ accountId: s.accountId, threadId: s.id, date: s.date, active: true, row: full({ ...s.row, group: label, status: s.row.status ?? "upcoming" }) });
    i = j;
  }
  for (const s of past) out.push({ accountId: s.accountId, threadId: s.id, date: s.date, active: false, row: full({ ...s.row, group: "Past", status: "past" }) });
  return out;
}

function reservations(scope: string[] | null): Item[] {
  const t = today();
  const all = seeds("reservations", scope);
  const up = all.filter((s) => (s.row.at ?? "") >= t).sort((a, b) => (a.row.at ?? "").localeCompare(b.row.at ?? ""));
  const past = all.filter((s) => (s.row.at ?? "") < t);
  return [
    ...up.map((s) => ({ accountId: s.accountId, threadId: s.id, date: s.date, active: true, row: full({ ...s.row, group: "Upcoming", status: "upcoming" }) })),
    ...past.map((s) => ({ accountId: s.accountId, threadId: s.id, date: s.date, active: false, row: full({ ...s.row, group: "Past", status: "past" }) })),
  ];
}

function packages(scope: string[] | null): Item[] {
  const all = seeds("packages", scope);
  const on = all.filter((s) => s.row.status !== "delivered").sort((a, b) => (a.row.at ?? "~").localeCompare(b.row.at ?? "~"));
  const done = all.filter((s) => s.row.status === "delivered");
  return [
    ...on.map((s) => ({ accountId: s.accountId, threadId: s.id, date: s.date, active: true, row: full({ ...s.row, group: "On the way" }) })),
    ...done.map((s) => ({ accountId: s.accountId, threadId: s.id, date: s.date, active: false, row: full({ ...s.row, group: "Delivered" }) })),
  ];
}

function bills(scope: string[] | null): Item[] {
  const t = today();
  const rows = seeds("bills", scope).map((s) => {
    const paid = s.row.status === "paid";
    const overdue = !paid && !!s.row.at && s.row.at < t;
    const status = paid ? "paid" : overdue ? "overdue" : "due";
    const rank = overdue ? 0 : paid ? 2 : 1;
    return { rank, s, item: { accountId: s.accountId, threadId: s.id, date: s.date, active: !paid, row: full({ ...s.row, status, group: paid ? "Paid" : overdue ? "Overdue" : "Due" }) } };
  });
  rows.sort((a, b) => a.rank - b.rank || (a.rank < 2 ? (a.s.row.at ?? "~").localeCompare(b.s.row.at ?? "~") : b.s.date - a.s.date));
  return rows.map((r) => r.item);
}

/** Same merchant, three or more charges about a month apart (or two a year apart). */
function subscriptions(scope: string[] | null): Item[] {
  const by = new Map<string, (Seed & { date: number })[]>();
  for (const s of seeds("receipts", scope)) {
    if (!s.row.amount || s.row.amount.value <= 0) continue;
    by.set(s.row.title, [...(by.get(s.row.title) ?? []), s]);
  }
  const out: Item[] = [];
  for (const [title, list] of by) {
    const asc = [...list].sort((a, b) => a.date - b.date);
    const gaps = asc.slice(1).map((s, i) => (s.date - asc[i].date) / DAY);
    const monthly = asc.length >= 3 && gaps.every((g) => g >= 26 && g <= 35);
    const yearly = asc.length >= 2 && gaps.every((g) => g >= 350 && g <= 380);
    if (!monthly && !yearly) continue;
    const last = asc[asc.length - 1];
    const nominal = monthly ? 30.44 : 365.25;
    out.push({
      accountId: last.accountId,
      threadId: last.id,
      date: last.date,
      active: true,
      row: full({
        kind: "subscription",
        title,
        amount: last.row.amount ?? null,
        at: local(-last.ago),
        end: local(-last.ago + nominal),
        status: "active",
        detail: `${monthly ? "Monthly" : "Yearly"} · ${asc.length} ${asc.length === 1 ? "charge" : "charges"}`,
        group: "Active",
      }),
    });
  }
  return out.sort((a, b) => (b.row.amount!.value * 365) / (b.row.detail!.startsWith("Monthly") ? 30 : 365) - (a.row.amount!.value * 365) / (a.row.detail!.startsWith("Monthly") ? 30 : 365));
}

function codes(scope: string[] | null): Item[] {
  return mockMail
    .threads()
    .filter((t) => inScope(t.accountId, scope) && t.otp && NOW - t.otp.date < 48 * HOUR)
    .map((t) => ({ accountId: t.accountId, threadId: t.threadId, date: t.otp!.date, active: NOW - t.otp!.date < 24 * HOUR, row: full({ kind: t.otp!.kind, title: "" }) }));
}

function invites(scope: string[] | null): Item[] {
  return mockPendingInvites()
    .filter((i) => inScope(i.accountId, scope))
    .map((i) => ({ accountId: i.accountId, threadId: i.threadId, date: mockMail.summary(i.accountId, i.threadId)?.lastDate ?? NOW, active: true, row: full({ kind: "invite", title: "", group: i.start < NOW + 7 * DAY ? "This week" : "Later" }) }));
}

const FILE_KIND: Record<string, (f: { filename: string; mimeType: string }) => boolean> = {
  pdf: (f) => f.mimeType === "application/pdf",
  images: (f) => f.mimeType.startsWith("image/"),
  docs: (f) => /word|presentation/.test(f.mimeType),
  sheets: (f) => /spreadsheet/.test(f.mimeType),
};

function files(scope: string[] | null, kind: string | null): Item[] {
  const seeded = seeds("files", scope).filter((s) => !kind || (s.files ?? []).some(FILE_KIND[kind]));
  const out: Item[] = seeded.map((s) => {
    const shown = (s.files ?? []).find((f) => !kind || FILE_KIND[kind](f)) ?? s.files![0];
    const n = (s.files ?? []).length;
    return { accountId: s.accountId, threadId: s.id, date: s.date, active: false, row: full({ kind: "file", title: shown.filename, detail: n > 1 ? `+${n - 1} more` : null }) };
  });
  if (!kind) {
    // Other mock mail with attachments (no file names needed for "All").
    for (const t of mockMail.threads()) {
      if (!t.hasAttachments || !inScope(t.accountId, scope) || out.some((o) => o.threadId === t.threadId && o.accountId === t.accountId)) continue;
      if (t.labelIds.includes("TRASH") || t.labelIds.includes("SPAM")) continue;
      out.push({ accountId: t.accountId, threadId: t.threadId, date: t.lastDate, active: false, row: full({ kind: "file", title: t.subject || "Attachment" }) });
    }
    out.sort((a, b) => b.date - a.date);
  }
  return out.slice(0, 300);
}

function people(q: ListQuery): ThreadSummary[] {
  const known = new Set<string>();
  for (const t of mockMail.threads()) if (t.labelIds.includes("SENT")) for (const p of t.participants) known.add(p.email.toLowerCase());
  for (const a of mockMail.accounts()) known.delete(a.email.toLowerCase());
  const inbox = mailHandlers.list_threads({ query: { ...q, view: { kind: "inbox" }, tab: null, limit: 1000 } }) as ThreadSummary[];
  return inbox.filter((t) => t.participants.some((p) => known.has(p.email.toLowerCase()))).slice(0, q.limit || 100);
}

function items(view: string, scope: string[] | null): Item[] {
  const [base, kind] = view.split(":");
  switch (base) {
    case "receipts": return receipts(scope);
    case "travel": return travel(scope);
    case "packages": return packages(scope);
    case "bills": return bills(scope);
    case "reservations": return reservations(scope);
    case "subscriptions": return subscriptions(scope);
    case "codes": return codes(scope);
    case "invites": return invites(scope);
    case "files": return files(scope, kind ?? null);
    default: return [];
  }
}

function scopeOf(q: { accountId?: string | null; accountIds?: string[] | null }): string[] | null {
  if (q.accountId) return q.accountIds ? q.accountIds.filter((a) => a === q.accountId) : [q.accountId];
  return q.accountIds ?? null;
}

function toRows(list: Item[], unreadOnly: boolean | undefined): ThreadSummary[] {
  const out: ThreadSummary[] = [];
  const seen = new Set<string>();
  for (const it of list) {
    const k = `${it.accountId}/${it.threadId}`;
    if (seen.has(k)) continue;
    seen.add(k);
    const t = mockMail.summary(it.accountId, it.threadId);
    if (!t || (unreadOnly && !t.unread)) continue;
    out.push({ ...t, labelIds: [...t.labelIds], participants: [...t.participants], lastDate: it.date, smart: it.row });
  }
  return out;
}

mockViews.smart = (q: ListQuery) => {
  if (q.view.kind !== "smart") return [];
  const view = q.view.labelId;
  if (view === "newsletters") return mailHandlers.list_threads({ query: { ...q, view: { kind: "inbox" }, tab: "newsletters" } }) as ThreadSummary[];
  if (view === "people") return q.before != null ? [] : people(q);
  if (q.before != null) return [];
  return toRows(items(view, scopeOf(q)), q.unreadOnly);
};

/** A saved search, matched simply: from:x, has:attachment, is:unread, and words in the subject or snippet. */
function matches(t: ThreadSummary, query: string): boolean {
  for (const raw of query.toLowerCase().split(/\s+/).filter(Boolean)) {
    const [op, val] = raw.includes(":") ? (raw.split(/:(.*)/s) as [string, string]) : ["", raw];
    if (op === "from") {
      if (!t.participants.some((p) => p.email.toLowerCase().includes(val) || (p.name ?? "").toLowerCase().includes(val))) return false;
    } else if (op === "has") {
      if ((val === "attachment" || val === "pdf") && !t.hasAttachments) return false;
    } else if (op === "is") {
      if (val === "unread" && !t.unread) return false;
      if (val === "starred" && !t.starred) return false;
    } else if (op === "in") {
      if (val === "inbox" && !t.labelIds.includes("INBOX")) return false;
    } else if (!`${t.subject} ${t.snippet}`.toLowerCase().includes(val)) return false;
  }
  return true;
}

mockViews.query = (q: ListQuery) => {
  if (q.view.kind !== "query" || q.before != null) return [];
  const query = q.view.labelId;
  if (!query.trim()) throw { code: "invalidInput", message: "a search needs at least one term" };
  const scope = scopeOf(q);
  return mockMail
    .threads()
    .filter((t) => inScope(t.accountId, scope) && !t.labelIds.includes("TRASH") && !t.labelIds.includes("SPAM"))
    .filter((t) => (!q.unreadOnly || t.unread) && matches(t, query))
    .slice(0, 500)
    .map((t) => ({ ...t, labelIds: [...t.labelIds], participants: [...t.participants] }));
};

// ---------------------------------------------------------------------------
// Header and counts
// ---------------------------------------------------------------------------
function totals(list: (Money | null)[]): Money[] {
  const by = new Map<string, number>();
  for (const m of list) if (m) by.set(m.currency, (by.get(m.currency) ?? 0) + m.value);
  return [...by].map(([currency, value]) => ({ currency, value: Math.round(value * 100) / 100 })).sort((a, b) => Math.abs(b.value) - Math.abs(a.value));
}

const stat = (label: string, value: string | null, amounts: Money[] = [], tone: SmartStat["tone"] = null): SmartStat => ({ label, value, amounts, tone });

function info(view: string, scope: string[] | null): SmartViewInfo {
  const list = items(view, scope);
  const active = list.filter((i) => i.active);
  let stats: SmartStat[] = [];
  switch (view) {
    case "receipts": {
      const last = list.filter((i) => i.row.group === monthName(new Date(new Date(NOW).getFullYear(), new Date(NOW).getMonth() - 1, 15).getTime()));
      stats = [stat("This month", active.length ? null : "Nothing yet", totals(active.map((i) => i.row.amount))), stat("Receipts", String(active.length))];
      if (last.length) stats.push(stat("Last month", null, totals(last.map((i) => i.row.amount))));
      break;
    }
    case "travel":
    case "reservations":
      stats = [stat("Upcoming", String(active.length))];
      if (active[0]) {
        stats.push(stat(view === "travel" ? "Next trip" : "Next booking", active[0].row.title));
        stats.push(stat("When", (active[0].row.group ?? "").split(" · ").pop() ?? null));
      }
      break;
    case "packages": {
      const arriving = active.filter((i) => i.row.status === "outForDelivery" || i.row.at === today()).length;
      stats = [stat("On the way", String(active.length))];
      if (arriving) stats.push(stat("Arriving today", String(arriving), [], "good"));
      const delivered = list.filter((i) => i.row.status === "delivered").length;
      if (delivered) stats.push(stat("Delivered this week", String(delivered)));
      break;
    }
    case "bills": {
      const overdue = active.filter((i) => i.row.status === "overdue").length;
      stats = [stat("Unpaid", active.length ? null : "Nothing due", totals(active.map((i) => i.row.amount)))];
      if (overdue) stats.push(stat("Overdue", String(overdue), [], "warn"));
      const next = active.find((i) => i.row.status === "due" && i.row.at);
      if (next) stats.push(stat("Next due", `${next.row.title} · ${shortDay(next.row.at!)}`));
      break;
    }
    case "subscriptions": {
      const monthly = active.map((i) => (i.row.amount ? { ...i.row.amount, value: i.row.detail?.startsWith("Yearly") ? i.row.amount.value / 12 : i.row.amount.value } : null));
      stats = [stat("Active", String(active.length)), stat("About a month", null, totals(monthly))];
      break;
    }
    case "invites":
      stats = [stat("Waiting for your answer", String(list.length))];
      break;
    case "codes":
      stats = [stat("Last 48 hours", String(list.length))];
      break;
  }
  return { view, stats, note: null };
}

function unreadCount(view: MailboxView_, scope: string[] | null): number {
  const q: ListQuery = { view, tab: null, accountId: null, accountIds: scope, limit: 1000, before: null, unreadOnly: true };
  const f = view.kind === "smart" ? mockViews.smart! : mockViews.query!;
  return f(q).length;
}
type MailboxView_ = ListQuery["view"];

export const smartHandlers: Record<string, MockHandler> = {
  smart_view_info: ({ view, accountIds }) => info(String(view).split(":")[0], (accountIds as string[] | null) ?? null),
  smart_counts: ({ views, accountIds }): SmartCount[] => {
    const scope = (accountIds as string[] | null) ?? null;
    return (views as string[]).map((view) => {
      if (view.startsWith("query:")) {
        try {
          return { view, count: unreadCount({ kind: "query", labelId: view.slice(6) }, scope) };
        } catch {
          return { view, count: 0 };
        }
      }
      if (view === "newsletters" || view === "people" || view.startsWith("files")) return { view, count: unreadCount({ kind: "smart", labelId: view }, scope) };
      return { view, count: new Set(items(view, scope).filter((i) => i.active).map((i) => `${i.accountId}/${i.threadId}`)).size };
    });
  },
};
