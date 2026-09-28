// OWNER: ask agent. Pure helpers for questions read as queries: the
// "Understood as" chips and their edits, grouped rows, and when to ask the
// on-device model (tested in tests/ask.test.ts). Rendering is AskQuery.tsx.
import type { AskAnswer, AskGroup, AskQuery, AskUnderstood, QueryGroup, QueryMeasure, QueryOp, QuerySubject } from "../../lib/types";
import { money } from "./askFormat.ts";

export const SUBJECTS: { value: QuerySubject; label: string }[] = [
  { value: "flights", label: "Flights" },
  { value: "stays", label: "Hotel stays" },
  { value: "orders", label: "Orders" },
  { value: "parcels", label: "Packages" },
  { value: "bills", label: "Bills" },
  { value: "bookings", label: "Reservations" },
  { value: "spending", label: "Spending" },
  { value: "messages", label: "Emails" },
];

export const OPS: { value: QueryOp; label: string }[] = [
  { value: "count", label: "How many" },
  { value: "sum", label: "Total" },
  { value: "average", label: "Average" },
  { value: "max", label: "Most" },
  { value: "min", label: "Least" },
  { value: "list", label: "List" },
  { value: "first", label: "First" },
  { value: "last", label: "Last" },
  { value: "next", label: "Next" },
  { value: "exists", label: "Any?" },
];

export const GROUPS: { value: QueryGroup | null; label: string }[] = [
  { value: null, label: "No grouping" },
  { value: "month", label: "By month" },
  { value: "year", label: "By year" },
  { value: "merchant", label: "By store" },
  { value: "place", label: "By place" },
  { value: "person", label: "By person" },
];

export const MEASURES: { value: QueryMeasure; label: string }[] = [
  { value: "items", label: "Items" },
  { value: "money", label: "Money" },
  { value: "nights", label: "Nights" },
  { value: "trips", label: "Trips" },
];

/** One chip of "Understood as …": what it shows, and which part of the query it edits. */
export interface QueryChip {
  key: "subject" | "op" | "measure" | "groupBy" | "timeframe" | "compare" | "place" | "merchant" | "person";
  label: string;
  /** Can be removed with ×. */
  removable: boolean;
}

function labelOf<T>(list: { value: T; label: string }[], v: T): string {
  return list.find((x) => x.value === v)?.label ?? String(v);
}

/** The chips for a reading, in reading order: what · where/who · when · how. */
export function queryChips(u: AskUnderstood): QueryChip[] {
  const q = u.query;
  const out: QueryChip[] = [{ key: "subject", label: labelOf(SUBJECTS, q.subject), removable: false }];
  if (q.place) out.push({ key: "place", label: `${q.subject === "flights" ? (q.direction === "from" ? "from" : "to") : "in"} ${q.place}`, removable: true });
  if (q.merchant) out.push({ key: "merchant", label: `${q.subject === "flights" ? "with" : q.subject === "spending" ? "at" : "from"} ${q.merchant}`, removable: true });
  if (q.person) out.push({ key: "person", label: `${q.direction === "to" ? "to" : q.direction === "from" ? "from" : "with"} ${q.person}`, removable: true });
  if (q.timeframe) out.push({ key: "timeframe", label: u.rangeLabel ?? q.timeframe, removable: true });
  if (q.compare.length > 1) out.push({ key: "compare", label: (u.compareLabels.length ? u.compareLabels : q.compare).join(" vs "), removable: true });
  out.push({ key: "op", label: labelOf(OPS, q.op), removable: false });
  if (q.measure !== "items" && !(q.measure === "money" && q.op === "sum")) out.push({ key: "measure", label: labelOf(MEASURES, q.measure), removable: false });
  if (q.groupBy) out.push({ key: "groupBy", label: labelOf(GROUPS, q.groupBy), removable: true });
  return out;
}

/** The query without one part (a chip's ×). */
export function withoutPart(q: AskQuery, key: QueryChip["key"]): AskQuery {
  switch (key) {
    case "place":
      return { ...q, place: null, direction: q.subject === "messages" ? q.direction : null };
    case "merchant":
      return { ...q, merchant: null };
    case "person":
      return { ...q, person: null };
    case "timeframe":
      return { ...q, timeframe: null };
    case "compare":
      return { ...q, compare: [] };
    case "groupBy":
      return { ...q, groupBy: null };
    default:
      return q;
  }
}

/** A subject change keeps what still fits: money only where there are amounts, nights for stays. */
export function withSubject(q: AskQuery, subject: QuerySubject): AskQuery {
  const measure: QueryMeasure =
    q.measure === "nights" && subject !== "stays"
      ? "items"
      : q.measure === "trips" && subject !== "flights"
        ? "items"
        : q.measure === "money" && (subject === "parcels" || subject === "messages")
          ? "items"
          : q.measure;
  return {
    ...q,
    subject,
    measure,
    person: subject === "messages" ? q.person : null,
    groupBy: q.groupBy === "person" && subject !== "messages" ? null : q.groupBy === "place" && subject === "messages" ? null : q.groupBy,
  };
}

/** An operation change: totals and averages are about money unless nights are counted. */
export function withOp(q: AskQuery, op: QueryOp): AskQuery {
  const hasAmounts = q.subject !== "parcels" && q.subject !== "messages";
  let measure = q.measure;
  if ((op === "sum" || op === "average") && measure === "items" && hasAmounts) measure = "money";
  if (op === "count" && measure === "money") measure = "items";
  return { ...q, op, measure };
}

/** Where the reading came from, said plainly. */
export function sourceNote(u: AskUnderstood): string {
  switch (u.source) {
    case "model":
      return "Read by Apple Intelligence on this Mac and checked by Penguin. The answer is counted from your mail.";
    case "edited":
      return "Your edit. The answer is counted from your mail.";
    default:
      return "Read by Penguin. Change any part to ask again.";
  }
}

/** A group's value as shown, and as a number for its bar. */
export function groupValue(g: AskGroup, measure: QueryMeasure | undefined): { text: string; value: number } {
  if (measure === "money") {
    const main = g.totals[0];
    return {
      text: g.totals.length ? g.totals.map((t) => money(t.value, t.currency)).join(" + ") : money(0, "USD"),
      value: main ? main.value : 0,
    };
  }
  if (measure === "nights") return { text: `${g.nights ?? 0} ${g.nights === 1 ? "night" : "nights"}`, value: g.nights ?? 0 };
  return { text: String(g.count), value: g.count };
}

/** An answer no grammar read exactly: worth asking the on-device model. */
export function wantsModel(a: AskAnswer): boolean {
  if (a.understood) return false;
  return a.intent === "passage" || a.intent === "unknown" || a.intent === "find" || (a.intent === "when" && a.confidence === "none");
}
