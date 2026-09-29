// Mock answers for the Ask questions added with totals by merchant, first
// contact, contact details and subscriptions (penguin-core src/ask: the
// query layer's merchant sums, `contact`, `contact_details`,
// `subscriptions`). OWNER: ask agent. Shaped like the real answers so the
// Ask card can be reviewed and screenshotted in demo mode. Fictional
// people, `.example` domains.
import type { AskAnswer, AskCite, AskFact, AskIntent, AskItem, AskQuery, Address } from "../types";
import { MOCK_ACCOUNT_IDS } from "./search";

const W = MOCK_ACCOUNT_IDS.work;
const P = MOCK_ACCOUNT_IDS.personal;
const DAY = 86_400_000;
const NOW = Date.now();

const priya: Address = { name: "Priya Natarajan", email: "priya@linden.example" };
const me: Address = { name: "Sam Okafor", email: "sam@northwind.example" };
const tracklet: Address = { name: "Tracklet", email: "receipts@tracklet.example" };
const streamly: Address = { name: "Streamly", email: "account@streamly.example" };
const tunewave: Address = { name: "Tunewave", email: "billing@tunewave.example" };
const PRIYA = { label: "Priya Natarajan", name: "Priya Natarajan", emails: [priya.email, "priya.n@mailbox.example"], domain: "linden.example", company: false };

const cite = (accountId: string, threadId: string, messageId: string): AskCite => ({ accountId, threadId, messageId });
const INTRO = cite(W, "t-priya-q4", "m-q4-1");
const PRICING = cite(W, "t-pricing-copy", "m-pricing-1");

function blank(intent: AskIntent, question: string): AskAnswer {
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
    tookMs: 2 + Math.random() * 5,
  };
}

function item(c: AskCite, subject: string, from: Address, daysAgo: number, note: string | null = null, sent = false): AskItem {
  return { ...c, subject, from, date: NOW - daysAgo * DAY, snippet: "", note, amount: null, sent };
}

const fmt = (ms: number) => new Date(ms).toLocaleDateString("en-US", { month: "short", day: "numeric", year: "numeric" });
const fact = (label: string, value: string, daysAgo: number | null = null, c: AskCite | null = null): AskFact => ({ label, value, date: daysAgo === null ? null : NOW - daysAgo * DAY, cite: c });

/** "When did I hire Priya", "how long have I known Priya", "first email with Priya". */
function firstContact(question: string, q: string): AskAnswer {
  const a = blank("firstContact", question);
  const first = NOW - 620 * DAY;
  const subject = "Intro: Linden × Northwind";
  if (/how long/.test(q)) {
    a.headline = `You've known Priya Natarajan since ${new Date(first).toLocaleDateString("en-US", { month: "short", year: "numeric" })} (1 year 8 months)`;
    a.detail = `From your first email with them: ${fmt(first)}, “${subject}” (they wrote it)`;
  } else {
    a.headline = `Your first email with Priya Natarajan: ${fmt(first)}, “${subject}”`;
    a.detail = /hire|start|sign|onboard|bring on/.test(q)
      ? "Your mail can't show when that started; this is the earliest email between you (they wrote it, 1 year ago)."
      : "1 year ago (they wrote it)";
  }
  a.facts = [
    fact("First from them", `${fmt(first)} · ${subject}`, 620, INTRO),
    fact("First from you", `${fmt(NOW - 618 * DAY)} · Re: ${subject}`, 618, INTRO),
    fact("Latest contact", `${fmt(NOW - 2 * DAY)} · Re: Pricing page copy`, 2, PRICING),
    fact("Messages", "41 from them · 27 from you"),
  ];
  a.items = [item(INTRO, subject, priya, 620), item(INTRO, `Re: ${subject}`, me, 618, null, true), item(PRICING, "Pricing page copy", priya, 470)];
  a.person = PRIYA;
  a.result = { kind: "item", count: 68, totals: [], value: null, yes: null, winner: null, date: new Date(first).toISOString().slice(0, 10), text: null };
  a.steps = ['"priya" matched Priya Natarajan <priya@linden.example>', "Found 68 messages with Priya Natarajan (from them, from you, or copied)"];
  a.searchQuery = "from:priya@linden.example OR to:priya@linden.example";
  a.followups = [
    { label: "Last email with Priya", question: "When did I last email priya@linden.example" },
    { label: "Relationship timeline", question: "How long have I worked with priya@linden.example" },
  ];
  return a;
}

/** "Priya's email", "how do I reach Priya"; "Mike's email" when two Mikes write to you. */
function contactDetails(question: string, q: string): AskAnswer {
  const a = blank("contactInfo", question);
  if (/\bmike\b/.test(q)) {
    a.headline = "2 people match “Mike”: Mike Kestrel <mike@kettleontheknoll.example> and Mike Delgado <mike@delgado.example>";
    a.detail = "Pick one below to see only theirs.";
    a.facts = [
      fact("Mike Kestrel", "mike@kettleontheknoll.example, mike@fernwood.example · 96 emails", 3, INTRO),
      fact("Mike Delgado", "mike@delgado.example · 4 emails", 40, PRICING),
    ];
    a.candidates = [
      { label: "Mike Kestrel <mike@kettleontheknoll.example>", question: "Mike@kettleontheknoll.example's email" },
      { label: "Mike Delgado <mike@delgado.example>", question: "Mike@delgado.example's email" },
    ];
    a.confidence = "medium";
    a.result = { kind: "item", count: null, totals: [], value: null, yes: null, winner: null, date: null, text: "mike@kettleontheknoll.example, mike@fernwood.example, mike@delgado.example" };
    return a;
  }
  const reach = /reach|contact|get in touch/.test(q);
  a.headline = reach ? "Reach Priya Natarajan at priya@linden.example or +1 (415) 555-0142" : "Priya Natarajan's email addresses: priya@linden.example, priya.n@mailbox.example";
  a.detail = `You've exchanged 68 emails at the first (41 from them, 27 from you), the last on ${fmt(NOW - 2 * DAY)}; the others are below`;
  a.facts = [
    fact("Email", `priya@linden.example · 41 from them · 27 from you · last ${fmt(NOW - 2 * DAY)}`, 2, PRICING),
    fact("Email", `priya.n@mailbox.example · 3 from them · 1 from you · last ${fmt(NOW - 200 * DAY)}`, 200, INTRO),
    fact("Phone", `+1 (415) 555-0142 · signature, ${fmt(NOW - 2 * DAY)}`, 2, PRICING),
    fact("Address", "418 Alder St, Suite 200, Portland, OR 97205", 2, PRICING),
  ];
  a.person = PRIYA;
  a.items = [item(PRICING, "Re: Pricing page copy", priya, 2)];
  a.result = { kind: "item", count: 72, totals: [], value: null, yes: null, winner: null, date: null, text: "priya@linden.example, priya.n@mailbox.example" };
  a.steps = ['"priya" matched Priya Natarajan <priya@linden.example, priya.n@mailbox.example>', "Addresses from the people index: every message stored, all accounts"];
  a.searchQuery = "from:priya@linden.example OR from:priya.n@mailbox.example";
  a.followups = [{ label: "Last email with Priya", question: "When did I last email priya@linden.example" }];
  return a;
}

/** "What subscriptions do I pay for". */
function subscriptions(question: string): AskAnswer {
  const a = blank("subscriptions", question);
  const S = cite(P, "t-streamly", "m-streamly-1");
  const T = cite(P, "t-tunewave", "m-tunewave-1");
  const L = cite(W, "t-tracklet", "m-tracklet-1");
  a.headline = "3 subscriptions, $49.72 a month: Tracklet, Streamly, Tunewave";
  a.detail = `Their latest charges. Not charged on schedule: Tunewave (last ${fmt(NOW - 48 * DAY)}); it may have stopped.`;
  a.groups = [
    { label: "Tracklet", count: 14, totals: [{ value: 24, currency: "USD", count: 1 }], nights: null, cites: [L], start: null, best: false },
    { label: "Streamly", count: 26, totals: [{ value: 15.73, currency: "USD", count: 1 }], nights: null, cites: [S], start: null, best: false },
    { label: "Tunewave", count: 25, totals: [{ value: 9.99, currency: "USD", count: 1 }], nights: null, cites: [T], start: null, best: false },
  ];
  a.facts = [
    fact("Tracklet", `$24.00 monthly · last ${fmt(NOW - 6 * DAY)} · next about ${fmt(NOW + 24 * DAY)}`, 6, L),
    fact("Streamly", `$15.73 monthly · last ${fmt(NOW - 21 * DAY)} · next about ${fmt(NOW + 9 * DAY)}`, 21, S),
    fact("Tunewave", `$9.99 monthly · last ${fmt(NOW - 48 * DAY)} · was due about ${fmt(NOW - 18 * DAY)}`, 48, T),
  ];
  a.items = [
    { ...item(L, "Your Tracklet receipt", tracklet, 6, "Monthly · 14 charges"), amount: { value: 24, currency: "USD", source: "Latest monthly charge" } },
    { ...item(S, "Your Streamly receipt", streamly, 21, "Monthly · 26 charges"), amount: { value: 15.73, currency: "USD", source: "Latest monthly charge" } },
    { ...item(T, "Your Tunewave receipt", tunewave, 48, "Monthly · 25 charges"), amount: { value: 9.99, currency: "USD", source: "Latest monthly charge" } },
  ];
  a.confidence = "medium";
  a.result = { kind: "list", count: 3, totals: [{ value: 49.72, currency: "USD", count: 3 }], value: null, yes: null, winner: null, date: null, text: null };
  a.steps = ["Found 3 recurring charges in your receipts: the same merchant charging at least three times (twice for yearly) at a steady weekly, monthly, quarterly or yearly cadence, the last three within 25% of each other"];
  return a;
}

/** "Total cost of my Tracklet receipts this year": the merchant's sum, read as a query. */
function merchantTotal(question: string): AskAnswer {
  const a = blank("spend", question);
  const year = new Date(NOW).getFullYear();
  const months = new Date(NOW).getMonth() + 1;
  const amounts = Array.from({ length: months }, (_, i) => (i === 3 ? 32 : 24));
  const total = amounts.reduce((s, v) => s + v, 0);
  a.headline = `$${total.toFixed(2)} across ${months} Tracklet receipts this year`;
  a.detail = "Summed from each email below (open the list to check every line); nothing is estimated.";
  a.sum = { totals: [{ value: total, currency: "USD", count: months }], basis: "The total on each receipt, order or booking", duplicates: 0, skipped: 1, unpaid: 0 };
  a.items = amounts
    .map((v, i) => ({
      ...item(cite(W, `t-tracklet-${i}`, `m-tracklet-${i}`), `Your Tracklet receipt for ${new Date(year, i, 1).toLocaleDateString("en-US", { month: "long" })}`, tracklet, Math.max(1, Math.round((NOW - new Date(year, i, 3).getTime()) / DAY)), `Amount paid $${v.toFixed(2)}`),
      amount: { value: v, currency: "USD", source: `Amount paid $${v.toFixed(2)}` },
    }))
    .reverse();
  a.facts = [fact("Largest", `$32.00 · ${fmt(new Date(year, 3, 3).getTime())}`, null, cite(W, "t-tracklet-3", "m-tracklet-3")), fact("Average", `$${(total / months).toFixed(2)}`)];
  const query: AskQuery = { subject: "orders", op: "sum", measure: "money", groupBy: null, timeframe: "this year", compare: [], place: null, merchant: "tracklet", person: null, direction: null, field: null, tense: "any" };
  a.understood = { query, source: "grammar", summary: `orders · at tracklet · ${year} · total`, rangeLabel: String(year), compareLabels: [] };
  a.result = { kind: "sum", count: months, totals: [{ value: total, currency: "USD", count: months }], value: null, yes: null, winner: null, date: null, text: null };
  a.confidence = "high";
  a.steps = ["Read “this year” as Jan 1 – Dec 31, " + year, '"tracklet" matched the domain tracklet.example', `Read ${months + 1} messages from Tracklet this year: the amount extracted from each receipt, else its “Total” line`];
  a.searchQuery = `from:tracklet.example after:${year}-01-01`;
  return a;
}

/** The mock's answer for one of these questions, or null. */
export function mockExtraAnswer(question: string): AskAnswer | null {
  const q = question.toLowerCase();
  if (/subscriptions|memberships|pay for every month/.test(q)) return subscriptions(question);
  if (/(total|sum|add up).*receipts|receipts .*total|tracklet total|how much (have i|did i) paid tracklet/.test(q)) return merchantTotal(question);
  if (/'s (email|e-mail)|email address|how (do|can) i (reach|contact|get in touch)/.test(q)) return contactDetails(question, q);
  if (/when did (i|we) (hire|start working|sign with|bring on)|when did .* hire (me|us)|how long have (i|we) known|first email (with|from|i sent)|oldest email/.test(q)) return firstContact(question, q);
  return null;
}
