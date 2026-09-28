// Mock for questions read as queries (penguin-core src/ask/query*.rs):
// counts with every item listed, grouped sums, comparisons, and the
// on-device model's reading (`ask_understand`) and edited readings
// (`ask_query`). OWNER: ask agent. Fictional people, `.example` domains.
import type { AskAnswer, AskCard, AskCite, AskGroup, AskIntent, AskItem, AskQuery, AskUnderstood, Address, FlightFact, QuerySource } from "../types";
import type { MockHandler } from "./index";
import { MOCK_ACCOUNT_IDS } from "./search";

const P = MOCK_ACCOUNT_IDS.personal;
const DAY = 86_400_000;
const NOW = Date.now();

const skyline: Address = { name: "Skyline Air", email: "trips@skylineair.example" };
const cite = (threadId: string): AskCite => ({ accountId: P, threadId, messageId: `${threadId}-m` });

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
    tookMs: 4 + Math.random() * 8,
  };
}

function query(p: Partial<AskQuery> & Pick<AskQuery, "subject" | "op">): AskQuery {
  return { measure: "items", groupBy: null, timeframe: null, compare: [], place: null, merchant: null, person: null, direction: null, field: null, tense: "any", ...p };
}

/** The most recent month `m` (0-11) that has started, as a Date. */
function recentMonth(m: number): Date {
  const now = new Date(NOW);
  const y = m <= now.getMonth() ? now.getFullYear() : now.getFullYear() - 1;
  return new Date(y, m, 1);
}
const monthLabel = (d: Date) => d.toLocaleDateString("en-US", { month: "long", year: "numeric" });
const span = (d: Date) => {
  const end = new Date(d.getFullYear(), d.getMonth() + 1, 0);
  return `${d.toLocaleDateString("en-US", { month: "short", day: "numeric" })} – ${end.toLocaleDateString("en-US", { month: "short", day: "numeric", year: "numeric" })}`;
};
const pad = (n: number) => String(n).padStart(2, "0");
const at = (d: Date, hh: number, mm: number) => `${d.getFullYear()}-${pad(d.getMonth() + 1)}-${pad(d.getDate())}T${pad(hh)}:${pad(mm)}`;

function understood(q: AskQuery, source: QuerySource, rangeLabel: string | null, compareLabels: string[] = []): AskUnderstood {
  const parts: string[] = [q.subject];
  if (q.place) parts.push(`${q.direction === "from" ? "from" : "to"} ${q.place}`);
  if (q.merchant) parts.push(`at ${q.merchant}`);
  if (rangeLabel) parts.push(rangeLabel);
  if (compareLabels.length) parts.push(compareLabels.join(" vs "));
  parts.push(q.op === "sum" ? "total" : q.op);
  if (q.groupBy) parts.push(`by ${q.groupBy}`);
  return { query: q, source, summary: parts.join(" · "), rangeLabel, compareLabels };
}

function flightCard(id: string, from: [string, string], to: [string, string], day: Date, number: string, conf: string): AskCard {
  const fact: FlightFact = {
    kind: "flight",
    airline: "Skyline Air",
    airlineCode: "SL",
    flightNumber: number,
    confirmation: conf,
    passenger: "Sam Okafor",
    departAirport: from[0],
    departName: from[1],
    arriveAirport: to[0],
    arriveName: to[1],
    departTime: at(day, 8 + (number.length % 9), 15),
    arriveTime: null,
    status: "confirmed",
    total: null,
  };
  return { fact, cite: cite(id), from: skyline, subject: `Your Skyline Air trip to ${to[1] === "San Francisco" ? from[1] : to[1]} (${conf})`, date: day.getTime() - 20 * DAY, source: "jsonLd", related: [], status: null };
}

function itemOf(c: AskCard): AskItem {
  return { ...c.cite, subject: c.subject, from: c.from, date: c.date, snippet: "", note: null, amount: null, sent: false };
}

/** "How many times did I fly in August": the count, and every flight. */
function flightsInAugust(question: string, source: QuerySource): AskAnswer {
  const aug = recentMonth(7);
  const d = (n: number) => new Date(aug.getFullYear(), 7, n);
  const a = blank("query", question);
  a.cards = [
    flightCard("t-q-chi", ["SFO", "San Francisco"], ["ORD", "Chicago"], d(4), "SL 318", "Q7RM2T"),
    flightCard("t-q-chi", ["ORD", "Chicago"], ["SFO", "San Francisco"], d(9), "SL 319", "Q7RM2T"),
    flightCard("t-q-bos", ["SFO", "San Francisco"], ["BOS", "Boston"], d(21), "SL 1204", "H2KD8W"),
  ];
  a.items = a.cards.map(itemOf);
  const q = query({ subject: "flights", op: "count", timeframe: "in august", tense: "past" });
  a.understood = understood(q, source, span(aug));
  a.headline = `Flights in ${monthLabel(aug)}: 3`;
  a.detail = "On 2 bookings";
  a.result = { kind: "count", count: 3, totals: [], value: null, yes: null, winner: null, date: null, text: null };
  a.steps = [`Read “in august” as ${span(aug)}`, "Read 41 flight facts extracted from your mail (17 from schema.org markup, 24 from the text)", "Found 41 flights in your mail; counted each once (emails about the same booking merged)"];
  a.searchQuery = `flight after:${aug.getFullYear()}-08-01 before:${aug.getFullYear()}-09-01`;
  a.followups = [
    { label: "By month this year", question: "How many flights did I take per month this year" },
    { label: "Where do I fly the most?", question: "Where do I fly the most" },
  ];
  return a;
}

function groups(labels: string[], values: number[], best: number, money: boolean, cites: AskCite[] = []): AskGroup[] {
  return labels.map((label, i) => ({
    label,
    count: money ? 3 + ((i * 5) % 7) : values[i],
    totals: money ? [{ value: values[i], currency: "USD", count: 3 + ((i * 5) % 7) }] : [],
    nights: null,
    cites: cites.slice(0, 1),
    start: null,
    best: i === best,
  }));
}

function spendByMonth(question: string): AskAnswer {
  const a = blank("query", question);
  const labels: string[] = [];
  for (let m = 0; m < 6; m++) {
    const d = new Date(new Date(NOW).getFullYear(), new Date(NOW).getMonth() - 5 + m, 1);
    labels.push(monthLabel(d));
  }
  const values = [412.18, 388.4, 951.06, 297.75, 640.2, 503.9];
  a.groups = groups(labels, values, 2, true, [cite("t-paperleaf")]);
  a.understood = understood(query({ subject: "spending", op: "max", measure: "money", groupBy: "month" }), "grammar", null);
  a.headline = `${labels[2]} had the most: $951.06`;
  a.detail = "Receipts, orders and booking totals; bills not marked paid are left out.";
  a.result = { kind: "groups", count: 31, totals: [], value: null, yes: null, winner: labels[2], date: null, text: null };
  a.steps = ["Read 214 order facts, 12 booking facts and 64 bill facts extracted from your mail", "Grouped every purchase by the month of its email"];
  return a;
}

function julyOrAugust(question: string): AskAnswer {
  const a = blank("query", question);
  const jul = monthLabel(recentMonth(6));
  const aug = monthLabel(recentMonth(7));
  a.groups = groups([jul, aug], [1184.6, 1422.35], 1, true, [cite("t-paperleaf")]);
  a.understood = understood(query({ subject: "spending", op: "sum", measure: "money", compare: ["july", "august"] }), "grammar", null, [jul, aug]);
  a.headline = `More in ${aug}: $1,422.35 vs $1,184.60 in ${jul}`;
  a.detail = "$237.75 more";
  a.result = { kind: "compare", count: null, totals: [], value: null, yes: null, winner: aug, date: null, text: null };
  return a;
}

/** The grammar can't read this one; the model can (mock of `ask_understand`). */
const MODEL_QUESTION = /how often .*(end up|wind up) (flying|going) to lisbon/;

function lisbonByModel(question: string): AskAnswer {
  const a = blank("query", question);
  const last = new Date(NOW).getFullYear() - 1;
  a.cards = [
    flightCard("t-q-lis1", ["SFO", "San Francisco"], ["LIS", "Lisbon"], new Date(last, 3, 11), "SL 76", "LX4P9Q"),
    flightCard("t-q-lis2", ["SFO", "San Francisco"], ["LIS", "Lisbon"], new Date(last, 9, 2), "SL 76", "M3VZ7K"),
  ];
  a.items = a.cards.map(itemOf);
  a.understood = understood(query({ subject: "flights", op: "count", place: "Lisbon", direction: "to", timeframe: "last year", tense: "past" }), "model", String(last));
  a.headline = `Flights to Lisbon in ${last}: 2`;
  a.result = { kind: "count", count: 2, totals: [], value: null, yes: null, winner: null, date: null, text: null };
  a.steps = [
    "Read the question with Apple Intelligence (on this Mac), then checked its reading against the query schema and your mail",
    `Read “last year” as Jan 1 – Dec 31, ${last}`,
    "“lisbon” = LIS",
  ];
  return a;
}

/** A query answer for the mock, when the question reads as one. */
export function mockQueryAnswer(question: string): AskAnswer | null {
  const q = question.toLowerCase();
  if (/how many (times did i fly|flights did i take) in august|cu[aá]ntas veces vol[eé]/.test(q)) return flightsInAugust(question, "grammar");
  if (/which month did i spend the most/.test(q)) return spendByMonth(question);
  if (/spend more in july or august/.test(q)) return julyOrAugust(question);
  if (MODEL_QUESTION.test(q)) {
    const a = blank("passage", question);
    a.headline = "No sentence in your mail clearly answers that";
    a.confidence = "none";
    return a;
  }
  return null;
}

/** An edited reading: the same shape, counted again (the mock's numbers are made up). */
function edited(question: string, q: AskQuery): AskAnswer {
  if (q.subject === "flights" && q.op === "count" && !q.place && !q.groupBy) {
    const a = flightsInAugust(question, "edited");
    a.understood = understood(q, "edited", q.timeframe ? a.understood?.rangeLabel ?? q.timeframe : null);
    return a;
  }
  const a = blank("query", question);
  const n = (question.length + q.subject.length * 7 + (q.timeframe?.length ?? 0)) % 23;
  const name = q.subject[0].toUpperCase() + q.subject.slice(1);
  a.understood = understood(q, "edited", q.timeframe);
  a.headline = `${name}${q.place ? ` to ${q.place}` : ""}${q.timeframe ? ` in ${q.timeframe}` : ""}: ${n}`;
  a.result = { kind: "count", count: n, totals: [], value: null, yes: null, winner: null, date: null, text: null };
  return a;
}

export const askQueryHandlers: Record<string, MockHandler> = {
  ask_understand: ({ question }) => {
    const q = String(question ?? "");
    return MODEL_QUESTION.test(q.toLowerCase()) ? lisbonByModel(q) : null;
  },
  ask_query: ({ question, query: q }) => edited(String(question ?? ""), q as AskQuery),
};
