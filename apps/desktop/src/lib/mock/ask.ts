// Mock for the `ask` command (penguin-core src/ask). OWNER: ask agent.
// The real answers are computed in Rust; this returns canned answers shaped
// like the real ones, citing threads from the mock search world, so the
// Ask card can be built and reviewed in the browser.
import type { AskAnswer, AskCard, AskCite, AskIntent, AskItem, AskScope, AskTimeline, Address } from "../types";
import type { MockHandler } from "./index";
import { MOCK_ACCOUNT_IDS } from "./search";
import { askQueryHandlers, mockQueryAnswer } from "./askQuery";

const W = MOCK_ACCOUNT_IDS.work;
const P = MOCK_ACCOUNT_IDS.personal;
const DAY = 86_400_000;
const NOW = Date.now();

const priya: Address = { name: "Priya Natarajan", email: "priya@linden.example" };
const dana: Address = { name: "Dana Whitfield", email: "dana@whitfield.example" };
const ledgerly: Address = { name: "Ledgerly Billing", email: "billing@ledgerly.example" };
const me: Address = { name: "Sam Okafor", email: "sam@northwind.example" };

const cite = (accountId: string, threadId: string, messageId: string): AskCite => ({ accountId, threadId, messageId });
const Q4 = cite(W, "t-priya-q4", "m-q4-1");
const PRICING = cite(W, "t-pricing-copy", "m-pricing-1");
const OFFSITE = cite(W, "t-offsite", "m-offsite-1");
const LEASE = cite(P, "t-lease-renewal", "m-lease-1");
const LEASE3 = cite(P, "t-lease-renewal", "m-lease-3");
const RENT = cite(P, "t-rent", "m-receipt-1");

function item(c: AskCite, subject: string, from: Address, daysAgo: number, note: string | null = null, sent = false): AskItem {
  return { ...c, subject, from, date: NOW - daysAgo * DAY, snippet: "", note, amount: null, sent };
}

function base(intent: AskIntent, question: string): AskAnswer {
  return {
    intent,
    question,
    headline: "",
    detail: null,
    facts: [],
    timeline: null,
    items: [],
    person: null,
    candidates: [],
    confidence: "high",
    steps: [],
    searchQuery: null,
    followups: [],
    cards: [],
    passages: [],
    sum: null,
    coverage: null,
    understood: null,
    groups: [],
    result: null,
    tookMs: 3 + Math.random() * 6,
  };
}

const fmt = (ms: number) => new Date(ms).toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
const PRIYA = { label: "Priya Natarajan", name: "Priya Natarajan", emails: [priya.email], domain: "linden.example", company: false };

function timeline(): AskTimeline {
  // Jan (intro), then regular May → Jun of the next year, then quiet.
  const start = new Date(new Date(NOW).getFullYear() - 1, 0, 1);
  const buckets = [];
  for (let i = 0; i < 21; i++) {
    const d = new Date(start.getFullYear(), start.getMonth() + i, 1);
    const regular = i >= 4 && i <= 17;
    buckets.push({
      start: d.getTime(),
      label: d.toLocaleDateString(undefined, { month: "short", year: "numeric" }),
      fromThem: i === 0 ? 1 : regular ? 2 + ((i * 7) % 4) : i === 18 ? 0 : i === 20 ? 1 : 0,
      fromMe: regular ? 1 + ((i * 5) % 3) : i === 18 ? 1 : i === 20 ? 1 : 0,
    });
  }
  return {
    unit: "month",
    buckets,
    markers: [
      { date: buckets[0].start + 14 * DAY, label: "First contact", cite: Q4 },
      { date: buckets[4].start + 4 * DAY, label: "Regular from", cite: PRICING },
      { date: buckets[17].start + 25 * DAY, label: "Handoff", cite: OFFSITE },
      { date: buckets[20].start + 3 * DAY, label: "Last contact", cite: Q4 },
    ],
  };
}

// ---- rich answers: one per card type, from the "Structured" mock mail ----

const aurora: Address = { name: "Aurora Air", email: "bookings@auroraair.example" };
const alder: Address = { name: "The Alder Lisbon", email: "stay@alderhotels.example" };
const paperleaf: Address = { name: "Paperleaf", email: "orders@paperleaf.example" };
const brightwave: Address = { name: "Brightwave Internet", email: "billing@brightwave.example" };
const fernfig: Address = { name: "Fern & Fig", email: "hello@fernandfig.example" };
const rydeo: Address = { name: "Rydeo", email: "noreply@rydeo.example" };
const grace: Address = { name: "Grace Kim", email: "grace@northwind.example" };
const FLIGHT = cite(P, "t-flight-lisbon", "m-flight-1");
const RETURN = cite(P, "t-flight-lisbon", "m-flight-2");
const HOTEL = cite(P, "t-hotel-lisbon", "m-hotel-1");
const ORDER = cite(P, "t-paperleaf", "m-order-1");
const SHIP = cite(P, "t-paperleaf", "m-ship-1");
const BILL = cite(P, "t-brightwave", "m-bill-1");
const TABLE = cite(P, "t-fernfig", "m-table-1");
const CODE = cite(P, "t-rydeo-code", "m-code-1");
const WIFI = cite(W, "t-offsite", "m-offsite-2");

/** Local wall time `days` from now at hh:mm, as the backend writes it. */
function local(days: number, hh = 0, mm = 0, withTime = true): string {
  const d = new Date(NOW + days * DAY);
  const p = (n: number) => String(n).padStart(2, "0");
  const date = `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}`;
  return withTime ? `${date}T${p(hh)}:${p(mm)}` : date;
}

function card(fact: AskCard["fact"], c: AskCite, from: Address, subject: string, daysAgo: number, status: string | null, source: AskCard["source"] = "jsonLd", related: AskCite[] = []): AskCard {
  return { fact, cite: c, from, subject, date: NOW - daysAgo * DAY, source, related, status };
}

function rich(question: string, q: string, scope: AskScope | null): AskAnswer | null {
  if (/flight|fly|vuelo/.test(q)) {
    const a = base("flight", question);
    const confirmation = /confirmation|code|c[oó]digo|localizador/.test(q);
    a.headline = confirmation ? "Confirmation code RXJ34P: AU 238 to Lisbon (LIS), " + new Date(NOW + 6 * DAY).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" }) : `AU 238 to Lisbon (LIS): ${new Date(NOW + 6 * DAY).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })} at 7:05 PM from SFO (in 6 days)`;
    a.detail = "Aurora Air · confirmation RXJ34P · arrives the next day at 3:10 PM";
    a.cards = [
      card(
        { kind: "flight", airline: "Aurora Air", airlineCode: "AU", flightNumber: "AU 238", confirmation: "RXJ34P", passenger: "Sam Okafor", departAirport: "SFO", departName: "San Francisco", arriveAirport: "LIS", arriveName: "Lisbon", departTime: local(6, 19, 5), arriveTime: local(7, 15, 10), status: "confirmed", total: { value: 1184.2, currency: "USD" } },
        FLIGHT, aurora, "Your flight confirmation: Lisbon", 12, "In 6 days", "jsonLd", [cite(P, "t-flight-lisbon", "m-flight-checkin")],
      ),
      card(
        { kind: "flight", airline: "Aurora Air", airlineCode: "AU", flightNumber: "AU 239", confirmation: "RXJ34P", passenger: "Sam Okafor", departAirport: "LIS", departName: "Lisbon", arriveAirport: "SFO", arriveName: "San Francisco", departTime: local(14, 11, 40), arriveTime: local(14, 15, 5), status: "confirmed", total: null },
        RETURN, aurora, "Your flight confirmation: Lisbon", 12, "In 2 weeks",
      ),
    ];
    a.items = [item(FLIGHT, "Your flight confirmation: Lisbon", aurora, 12), item(RETURN, "Your flight confirmation: Lisbon", aurora, 12)];
    a.steps = ["“lisbon” = LIS", "Read 3 flight facts extracted from your mail (3 from schema.org markup, 0 from the text)"];
    a.searchQuery = "flight lisbon";
    a.followups = [
      { label: "Where am I staying in Lisbon?", question: "Where am I staying in Lisbon" },
      { label: "All upcoming flights", question: "My upcoming flights" },
    ];
    return a;
  }
  if (/staying|hotel|check in|hospedo/.test(q)) {
    const a = base("stay", question);
    a.headline = `The Alder Lisbon: check in ${new Date(NOW + 7 * DAY).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })} at 3 PM, check out ${new Date(NOW + 11 * DAY).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })} at 11 AM (in 7 days)`;
    a.detail = "Rua das Flores 12, Lisboa, 1200-192, PT · confirmation 48291";
    a.cards = [
      card({ kind: "lodging", name: "The Alder Lisbon", address: "Rua das Flores 12, Lisboa, 1200-192, PT", phone: "+351 21 555 0100", checkin: local(7, 15, 0), checkout: local(11, 11, 0), confirmation: "48291", guest: "Sam Okafor", status: "confirmed", total: { value: 912, currency: "EUR" } }, HOTEL, alder, "Reservation confirmed", 10, "In 7 days"),
    ];
    a.items = [item(HOTEL, "Reservation confirmed", alder, 10)];
    a.steps = ["Read 2 lodging facts extracted from your mail (2 from schema.org markup, 0 from the text)"];
    return a;
  }
  if (/package|parcel|track|shipped|arrive|paquete|pedido|where('s| is) my/.test(q)) {
    const a = base("package", question);
    a.headline = `Your Paperleaf order is on its way with UPS, arriving ${new Date(NOW + 2 * DAY).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })}`;
    a.detail = "Tracking 1Z879E930346834440";
    a.cards = [
      card({ kind: "shipment", carrier: "UPS", trackingNumber: "1Z879E930346834440", verified: true, trackingUrl: "https://www.ups.com/track?tracknum=1Z879E930346834440", status: "inTransit", expected: local(2, 0, 0, false), merchant: "Paperleaf", orderNumber: "PL-112-7719", items: ["Dot grid notebook", "Brass pen"] }, SHIP, paperleaf, "Your Paperleaf order is on its way", 1, "Arriving " + new Date(NOW + 2 * DAY).toLocaleDateString(undefined, { weekday: "short" }), "pattern"),
    ];
    a.items = [item(SHIP, "Your Paperleaf order is on its way", paperleaf, 1)];
    a.steps = ["Read 4 shipment facts extracted from your mail (1 from schema.org markup, 3 from the text)"];
    a.searchQuery = "paperleaf (shipped OR tracking OR delivery)";
    return a;
  }
  if (/order|bought|buy|compr/.test(q)) {
    const a = base("orders", question);
    a.headline = "Latest Paperleaf order PL-112-7719 (Dot grid notebook, Brass pen), $58.20 on " + fmt(NOW - 3 * DAY);
    a.detail = "3 orders from Paperleaf";
    a.cards = [
      card({ kind: "order", merchant: "Paperleaf", orderNumber: "PL-112-7719", total: { value: 58.2, currency: "USD" }, totalSource: "Order Total: $58.20", items: ["Dot grid notebook", "Brass pen"], status: "shipped" }, ORDER, paperleaf, "Order confirmation", 3, "Shipped", "pattern", [SHIP]),
      card({ kind: "order", merchant: "Paperleaf", orderNumber: "PL-109-2210", total: { value: 24, currency: "USD" }, totalSource: "Order Total: $24.00", items: [], status: null }, cite(P, "t-paperleaf-2", "m-order-2"), paperleaf, "Order confirmation", 40, null, "pattern"),
    ];
    a.items = [{ ...item(ORDER, "Order confirmation", paperleaf, 3), amount: { value: 58.2, currency: "USD", source: "Order Total: $58.20" } }];
    return a;
  }
  if (/bill|invoice|due|factura|vence/.test(q)) {
    const a = base("bills", question);
    a.headline = `2 bills due; next: Brightwave Internet: $64.12 due ${new Date(NOW + 9 * DAY).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })} (in 9 days)`;
    a.detail = "$1,314.12 in total";
    a.cards = [
      card({ kind: "bill", biller: "Brightwave Internet", invoiceNumber: null, amountDue: { value: 64.12, currency: "USD" }, amountSource: "New balance: $64.12", dueDate: local(9, 0, 0, false), status: "due" }, BILL, brightwave, "Your Brightwave statement is ready", 5, "Due in 9 days", "pattern"),
      card({ kind: "bill", biller: "Ledgerly", invoiceNumber: "INV-2041", amountDue: { value: 1250, currency: "USD" }, amountSource: "Amount due: $1,250.00", dueDate: local(19, 0, 0, false), status: "due" }, RENT, ledgerly, "Invoice INV-2041 from Ledgerly", 11, "Due in 19 days", "jsonLd"),
    ];
    a.items = [
      { ...item(BILL, "Your Brightwave statement is ready", brightwave, 5), amount: { value: 64.12, currency: "USD", source: "New balance: $64.12" } },
      { ...item(RENT, "Invoice INV-2041 from Ledgerly", ledgerly, 11), amount: { value: 1250, currency: "USD", source: "Amount due: $1,250.00" } },
    ];
    a.confidence = "medium";
    a.coverage = "Still reading 1,204 emails for bills; this may be missing some.";
    a.steps = ["Read 26 bill facts extracted from your mail (12 from schema.org markup, 14 from the text)", "A bill counts as paid when a later email from the same biller says so for the same invoice number or amount"];
    return a;
  }
  if (/reservation|table|dinner|tickets|reserva|mesa/.test(q)) {
    const a = base("booking", question);
    a.headline = `Fern & Fig: ${new Date(NOW + 3 * DAY).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric" })} at 7:30 PM (in 3 days), 4 people`;
    a.detail = "210 Harbor Way, Oakland · confirmation FF-5521";
    a.cards = [
      card({ kind: "reservation", category: "restaurant", name: "Fern & Fig", start: local(3, 19, 30), end: null, venue: "Fern & Fig", address: "210 Harbor Way, Oakland", confirmation: "FF-5521", partySize: 4, status: "confirmed", total: null }, TABLE, fernfig, "Reservation confirmed at Fern & Fig", 2, "In 3 days", "microdata"),
    ];
    a.items = [item(TABLE, "Reservation confirmed at Fern & Fig", fernfig, 2)];
    return a;
  }
  if (/code|c[oó]digo/.test(q)) {
    const a = base("code", question);
    a.headline = "0357 from Rydeo (4 min ago)";
    a.items = [item(CODE, "0357 is your Rydeo code", rydeo, 0)];
    a.steps = ["Looked at verification codes detected in the last 30 days from “rydeo”"];
    return a;
  }
  if (/phone|number|address|reach|tel[eé]fono|direcci[oó]n/.test(q)) {
    const a = base("contactInfo", question);
    a.headline = "Priya Natarajan's phone number: +1 (415) 555-0142";
    a.detail = "From their signature, " + fmt(NOW - 2 * DAY) + " (in 6 emails)";
    a.person = PRIYA;
    a.facts = [{ label: "Other number", value: "+1 (415) 555-0100 · 1 email · " + fmt(NOW - 200 * DAY), date: NOW - 200 * DAY, cite: Q4 }];
    a.cards = [card({ kind: "contact", phones: ["+1 (415) 555-0142"], addresses: ["418 Alder St, Suite 200, Portland, OR 97205"] }, PRICING, priya, "Re: Pricing page copy", 2, null, "pattern")];
    a.items = [item(PRICING, "Re: Pricing page copy", priya, 2)];
    return a;
  }
  if (/say about|said|think about|decide|dijo|sobre/.test(q)) {
    const a = base("said", question);
    a.headline = "Priya Natarajan on pricing: “Let's keep the annual plan at $12 a seat and drop the setup fee for teams under ten.”";
    a.detail = "Priya Natarajan, " + fmt(NOW - 2 * DAY) + " · Re: Pricing page copy";
    a.person = PRIYA;
    const text1 = "Let's keep the annual plan at $12 a seat and drop the setup fee for teams under ten.";
    const text2 = "On pricing, I'd rather we show the monthly price first; annual can be the toggle.";
    const marks = (t: string, w: string[]): [number, number][] => w.flatMap((x) => { const i = t.toLowerCase().indexOf(x); return i >= 0 ? [[i, i + x.length] as [number, number]] : []; });
    a.passages = [
      { text: text1, marks: marks(text1, ["plan", "seat", "fee"]), cite: PRICING, from: priya, subject: "Re: Pricing page copy", date: NOW - 2 * DAY, sent: false, score: 0.82 },
      { text: text2, marks: marks(text2, ["pricing", "price"]), cite: Q4, from: priya, subject: "Q4 brand refresh — final review deck", date: NOW - 20 * DAY, sent: false, score: 0.64 },
    ];
    a.items = [item(PRICING, "Re: Pricing page copy", priya, 2, text1), item(Q4, "Q4 brand refresh — final review deck", priya, 20, text2)];
    a.confidence = "medium";
    a.steps = ["Searched “pricing (from:priya@linden.example)”: 6 threads", "Searched by meaning (granite-embedding-97m, 18,204 indexed passages): 12 emails", "Fused the two lists by reciprocal rank (k = 60): 4 found by both, 3 by meaning only", "Scored 212 sentences in the top 12 emails by the question's words (weighted by rarity), how close together they are, meaning, and the email's rank", "Quoted as written; nothing is paraphrased or combined"];
    return a;
  }
  if (/wifi|password|how do i|why /.test(q)) {
    const a = base("passage", question);
    const text = "The wifi at the venue is Lanterns-Guest, password harbor2026 (all lowercase).";
    a.headline = `“${text}”`;
    a.detail = "Grace Kim, " + fmt(NOW - 6 * DAY) + " · Offsite logistics";
    const at = (w: string): [number, number] => [text.indexOf(w), text.indexOf(w) + w.length];
    a.passages = [{ text, marks: [at("wifi"), at("password")], cite: WIFI, from: grace, subject: "Offsite logistics", date: NOW - 6 * DAY, sent: false, score: 0.78 }];
    a.items = [item(WIFI, "Offsite logistics", grace, 6, text)];
    a.confidence = "medium";
    return a;
  }
  if (/did .* (reply|respond|get back)/.test(q)) {
    const a = base("didReply", question);
    a.headline = "Not yet: you wrote last on " + fmt(NOW - 4 * DAY) + " (“Q4 brand refresh — final review deck”), nothing from Priya Natarajan since";
    a.detail = "Waiting 4 days.";
    a.person = PRIYA;
    a.items = [item(Q4, "Q4 brand refresh — final review deck", me, 4, "Your last message", true)];
    a.confidence = "medium";
    return a;
  }
  void scope;
  return null;
}

function answer(question: string, scope: AskScope | null): AskAnswer {
  const asQuery = mockQueryAnswer(question);
  if (asQuery) return asQuery;
  const q = question.toLowerCase();
  // Spend first ("how much did I spend on flights" is a sum, not a flight).
  const r = /spen[dt]|gast|how much did|paid|cu[aá]nto/.test(q) ? null : rich(question, q, scope);
  if (r) return r;
  const pronoun = /\b(he|she|they|them|him|her)\b/.test(q) && !!scope?.person?.length;

  if (/let (us|me) go|stop working|part ways|end(ed)? the/.test(q) || (pronoun && /when did/.test(q))) {
    const a = base("relationship", question);
    const tl = timeline();
    a.headline = `It looks like it ended on ${fmt(tl.markers[2].date)}: “Where we're leaving off”`;
    a.detail = "“We've decided to bring the work in-house, so this is where we're leaving off.”";
    a.facts = [
      { label: "Handoff", value: `${fmt(tl.markers[2].date)} · Where we're leaving off`, date: tl.markers[2].date, cite: OFFSITE },
      { label: "Since then", value: "3 messages · last " + fmt(tl.markers[3].date), date: null, cite: null },
    ];
    a.timeline = tl;
    a.items = [item(OFFSITE, "Where we're leaving off", priya, 90, "Handoff")];
    a.person = PRIYA;
    a.confidence = "medium";
    a.steps = ['"he" = Priya Natarajan <priya@linden.example> from the previous answer', "Scanned subject and body for 35 end-of-engagement phrases"];
    a.searchQuery = "from:priya@linden.example OR to:priya@linden.example";
    return a;
  }
  if (/how long|work(ed)? (with|for)|history with|start(ed)? working/.test(q)) {
    const a = base("relationship", question);
    const tl = timeline();
    const m = (i: number) => tl.buckets[i].label;
    a.headline = `You worked with Priya Natarajan from ${m(4)} to ${m(17)}`;
    a.detail = "“We've decided to bring the work in-house, so this is where we're leaving off.”";
    a.facts = [
      { label: "First contact", value: `${fmt(tl.markers[0].date)} · Intro: Linden × Northwind`, date: tl.markers[0].date, cite: Q4 },
      { label: "Regular from", value: `${fmt(tl.markers[1].date)} · Pricing page copy`, date: tl.markers[1].date, cite: PRICING },
      { label: "Handoff", value: `${fmt(tl.markers[2].date)} · Where we're leaving off`, date: tl.markers[2].date, cite: OFFSITE },
      { label: "Last contact", value: `${fmt(tl.markers[3].date)} · Dinner Thursday?`, date: tl.markers[3].date, cite: Q4 },
      { label: "Messages", value: `${tl.buckets.reduce((n, b) => n + b.fromThem, 0)} from them · ${tl.buckets.reduce((n, b) => n + b.fromMe, 0)} from you`, date: null, cite: null },
    ];
    a.timeline = tl;
    a.items = [
      item(OFFSITE, "Where we're leaving off", priya, 90, "Handoff"),
      item(PRICING, "Re: Pricing page copy", priya, 470, "Regular from"),
      item(Q4, "Dinner Thursday?", priya, 4, "Last contact"),
    ];
    a.person = PRIYA;
    a.confidence = "medium";
    a.steps = [
      '"priya linden" matched Priya Natarajan <priya@linden.example> by name and company',
      "Found 68 messages between you and Priya Natarajan",
      "Monthly activity: a month with ≥2 messages is active; the longest active stretch (one quiet month allowed) is the regular period",
      "Scanned subject and body for 35 end-of-engagement phrases (“leaving off”, “parting ways”, “final invoice”, …)",
    ];
    a.searchQuery = "from:priya@linden.example OR to:priya@linden.example";
    a.followups = [
      { label: "When did it end?", question: "When did they let us go" },
      { label: "Last email with Priya", question: "When did I last email priya@linden.example" },
      { label: "Waiting on Priya?", question: "What am I waiting on from priya@linden.example" },
    ];
    return a;
  }
  if (/spen[dt]|paid|pay|charge/.test(q)) {
    const a = base("spend", question);
    const amounts = [89, 89, 129, -20, 89];
    a.headline = "$376.00 across 5 Ledgerly receipts this year";
    a.detail = "Summed from each email below (open the list to check every line); nothing is estimated.";
    a.sum = { totals: [{ value: 376, currency: "USD", count: 5 }], basis: "The total on each receipt, order or booking", duplicates: 1, skipped: 2, unpaid: 0 };
    a.items = amounts.map((v, i) => ({
      ...item(RENT, v < 0 ? "Your refund from Ledgerly" : `Ledgerly receipt #${2040 + i}`, ledgerly, 20 + i * 45, v < 0 ? "Refund total: $20.00" : `Total $${v}.00`),
      amount: { value: v, currency: "USD", source: v < 0 ? "Refund total: $20.00" : `Total $${v}.00` },
    }));
    a.facts = [
      { label: "Largest", value: "$129.00 · " + fmt(NOW - 110 * DAY), date: NOW - 110 * DAY, cite: RENT },
      { label: "Average", value: "$75.20", date: null, cite: null },
    ];
    a.confidence = "medium";
    a.steps = ['"ledgerly" matched the domain ledgerly.example', "Read 7 messages from Ledgerly this year; took the “Total” line of each"];
    a.searchQuery = "from:ledgerly.example after:2026-01-01";
    return a;
  }
  if (/waiting/.test(q)) {
    const a = base("waitingOn", question);
    a.headline = "You're waiting on replies in 2 threads";
    a.detail = "Oldest: “Q4 brand refresh — final review deck”, sent 9 days ago";
    a.items = [item(Q4, "Q4 brand refresh — final review deck", me, 9, "Waiting 9 days · to Priya Natarajan", true), item(OFFSITE, "Offsite agenda: Oct 14–15", me, 3, "Waiting 3 days · to Grace Kim", true)];
    a.steps = ["Threads where your message is the latest, sent 2 days – 2 months ago"];
    a.searchQuery = "in:sent";
    return a;
  }
  if (/owe|reply to|respond to|needs? a reply/.test(q)) {
    const a = base("oweReplies", question);
    a.headline = "You owe replies in 2 threads";
    a.detail = "From Priya Natarajan, Dana Whitfield · 1 unread";
    a.items = [item(PRICING, "Re: Pricing page copy", priya, 2, "Unread · 2 days ago"), item(LEASE3, "Re: Lease renewal — 418 Alder St, Unit 3B", dana, 6, "6 days ago")];
    a.steps = ["Inbox threads from the last 30 days where the latest message is from a person, addressed to you, with no reply from you after it"];
    a.searchQuery = "in:inbox";
    return a;
  }
  if (/^(when (is|was|does)|what (day|date))/.test(q)) {
    const a = base("when", question);
    const due = NOW + 37 * DAY;
    a.headline = `Lease renewal: ${new Date(due).toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric", year: "numeric" })} (in 5 weeks)`;
    a.detail = "“Please return the signed renewal by October 31 so we can hold the current rate.” — Dana Whitfield, " + fmt(NOW - 40 * DAY);
    a.facts = [{ label: fmt(due), value: "Please return the signed renewal by October 31 so we can hold the current rate.", date: due, cite: LEASE }];
    a.items = [item(LEASE, "Lease renewal — 418 Alder St, Unit 3B", dana, 40, "Please return the signed renewal by October 31…")];
    a.confidence = "medium";
    a.steps = ["Searched “lease renewal”: 6 threads", "Read 14 messages in the top threads and extracted the dates written in them"];
    a.searchQuery = "lease renewal";
    return a;
  }
  if (/^who (emailed|sent|mentioned)/.test(q)) {
    const a = base("whoAbout", question);
    a.headline = "2 people emailed you about “pricing”: Priya Natarajan and Marco Bellini";
    a.facts = [
      { label: "Priya Natarajan", value: "3 threads · latest " + fmt(NOW - 2 * DAY), date: NOW - 2 * DAY, cite: PRICING },
      { label: "Marco Bellini", value: "1 thread · latest " + fmt(NOW - 30 * DAY), date: NOW - 30 * DAY, cite: PRICING },
    ];
    a.items = [item(PRICING, "Re: Pricing page copy", priya, 2)];
    a.confidence = "medium";
    a.searchQuery = "pricing";
    return a;
  }
  if (/last|lately|recent/.test(q)) {
    const a = base("lastContact", question);
    a.headline = `You last emailed Priya Natarajan on ${fmt(NOW - 2 * DAY)} (2 days ago)`;
    a.detail = "“Re: Q4 brand refresh — final review deck”";
    a.facts = [
      { label: "Last from them", value: `${fmt(NOW - 4 * DAY)} · Q4 brand refresh — final review deck`, date: NOW - 4 * DAY, cite: Q4 },
      { label: "Last from you", value: `${fmt(NOW - 2 * DAY)} · Re: Q4 brand refresh`, date: NOW - 2 * DAY, cite: Q4 },
      { label: "First contact", value: `${fmt(NOW - 600 * DAY)} · Intro: Linden × Northwind`, date: NOW - 600 * DAY, cite: PRICING },
      { label: "Messages", value: "41 from them · 27 from you", date: null, cite: null },
    ];
    a.items = [item(Q4, "Re: Q4 brand refresh — final review deck", me, 2, null, true), item(Q4, "Q4 brand refresh — final review deck", priya, 4)];
    a.person = PRIYA;
    a.candidates = [{ label: "Priya Raman <priya.r@alpinevc.example>", question: "When did I last email priya.r@alpinevc.example" }];
    a.confidence = "medium";
    a.steps = ['"priya" matched Priya Natarajan <priya@linden.example>', "Found 68 messages with Priya Natarajan (from them, from you, or copied)"];
    a.searchQuery = "from:priya@linden.example OR to:priya@linden.example";
    a.followups = [
      { label: "Relationship timeline", question: "How long have I known priya@linden.example" },
      { label: "Latest attachment from Priya", question: "Latest attachment from priya@linden.example" },
      { label: "Waiting on Priya?", question: "What am I waiting on from priya@linden.example" },
    ];
    return a;
  }
  const a = base("unknown", question);
  a.headline = "I can't answer that one directly";
  a.confidence = "none";
  a.followups = ["When did I last email Priya?", "How long have I known Linden?", "What am I waiting on?", "When is my lease renewal?"].map((x) => ({ label: x, question: x }));
  return a;
}

export const askHandlers: Record<string, MockHandler> = {
  ...askQueryHandlers,
  ask: ({ question, scope }) => answer(String(question ?? ""), (scope as AskScope | null) ?? null),
};
