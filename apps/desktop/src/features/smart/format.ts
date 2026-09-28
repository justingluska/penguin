// How a smart view row reads: the fact's title, a few short facts, the
// amount and a status chip. Pure (tests/smartViews.test.ts).
import type { Money, SmartRow, SmartStat } from "../../lib/types";
import { daysUntil, fmtDay, fmtWhen, money } from "../search/askFormat.ts";

export type ChipTone = "green" | "amber" | "red" | "gray" | "blue";

export interface SmartLineParts {
  title: string;
  facts: string[];
  amount: string | null;
  /** Negative amounts (refunds) read as credit. */
  credit: boolean;
  chip: { label: string; tone: ChipTone } | null;
}

/** "Today", "Tomorrow", "In 5 days", "Yesterday", "3 days ago". */
export function relDays(n: number): string {
  if (n === 0) return "Today";
  if (n === 1) return "Tomorrow";
  if (n === -1) return "Yesterday";
  if (n > 1) return n < 14 ? `In ${n} days` : n < 60 ? `In ${Math.round(n / 7)} weeks` : `In ${Math.round(n / 30)} months`;
  const a = -n;
  return a < 14 ? `${a} days ago` : a < 60 ? `${Math.round(a / 7)} weeks ago` : `${Math.round(a / 30)} months ago`;
}

export function fmtMoney(m: Money | null | undefined): string | null {
  return m ? money(m.value, m.currency) : null;
}

/** "USD 96.50 · €30.00": each currency on its own (never converted). */
export function fmtAmounts(list: Money[]): string {
  return list.map((m) => money(m.value, m.currency)).join(" · ");
}

const PARCEL: Record<string, [string, ChipTone]> = {
  shipped: ["Shipped", "blue"],
  inTransit: ["In transit", "blue"],
  outForDelivery: ["Out for delivery", "amber"],
  delivered: ["Delivered", "green"],
  exception: ["Problem", "red"],
};

/** A row's parts, or null for kinds whose row shows its own chip (invites, codes). */
export function smartLine(row: SmartRow, now = Date.now()): SmartLineParts | null {
  const facts: string[] = [];
  const push = (s: string | null | undefined) => {
    if (s && s.trim()) facts.push(s.trim());
  };
  let chip: SmartLineParts["chip"] = null;
  const when = (at: string | null) => {
    const n = daysUntil(at, now);
    return n === null ? null : n;
  };
  const upcomingChip = () => {
    if (row.status === "cancelled") return { label: "Cancelled", tone: "red" as const };
    const n = when(row.at);
    if (n === null) return null;
    return { label: relDays(n), tone: (n >= 0 && n <= 2 ? "amber" : n >= 0 ? "blue" : "gray") as ChipTone };
  };
  switch (row.kind) {
    case "invite":
    case "code":
    case "link":
      return null;
    case "receipt":
      push(row.detail);
      if (row.reference) push(`#${row.reference.replace(/^#/, "")}`);
      if (row.status === "refunded" || row.status === "returned") chip = { label: row.status === "refunded" ? "Refund" : "Returned", tone: "green" };
      else if (row.status === "cancelled") chip = { label: "Cancelled", tone: "red" };
      break;
    case "flight":
    case "car":
    case "train":
    case "bus":
      push(fmtWhen(row.at, now));
      push(row.detail);
      push(row.reference);
      chip = upcomingChip();
      break;
    case "stay":
      push(row.end ? `${fmtDay(row.at, now)} → ${fmtDay(row.end, now)}` : fmtDay(row.at, now));
      push(row.reference);
      chip = upcomingChip();
      break;
    case "restaurant":
    case "event":
    case "reservation":
      push(fmtWhen(row.at, now));
      push(row.detail);
      push(row.reference);
      chip = upcomingChip();
      break;
    case "parcel": {
      push(row.detail);
      if (row.at && row.status !== "delivered") push(`Expected ${fmtDay(row.at, now)}`);
      push(row.reference);
      const p = PARCEL[row.status ?? ""];
      if (p) chip = { label: p[0], tone: p[1] };
      break;
    }
    case "bill": {
      const n = when(row.at);
      if (row.status === "paid") chip = { label: "Paid", tone: "green" };
      else if (row.status === "overdue") chip = { label: n === null ? "Overdue" : `Overdue · ${relDays(n).toLowerCase()}`, tone: "red" };
      else if (row.status === "due" && n !== null) chip = { label: n === 0 ? "Due today" : n === 1 ? "Due tomorrow" : `Due in ${n} days`, tone: n <= 3 ? "amber" : "blue" };
      else if (row.status === "due") chip = { label: "Due", tone: "blue" };
      else chip = { label: "No payment seen", tone: "gray" };
      if (row.at && row.status !== "overdue") push(`Due ${fmtDay(row.at, now)}`);
      push(row.reference ? `Invoice ${row.reference}` : null);
      break;
    }
    case "subscription":
      push(row.detail);
      if (row.at) push(`Last ${fmtDay(row.at, now)}`);
      if (row.end) push(`Next ~${fmtDay(row.end, now)}`);
      chip = row.status === "active" ? { label: "Active", tone: "green" } : { label: "Stopped?", tone: "gray" };
      break;
    case "file":
      push(row.detail);
      break;
    default:
      push(row.detail);
  }
  return {
    title: row.title,
    facts,
    amount: fmtMoney(row.amount),
    credit: (row.amount?.value ?? 0) < 0,
    chip,
  };
}

/** A header figure as text. */
export function statText(s: SmartStat): string {
  if (s.amounts.length) return fmtAmounts(s.amounts);
  return s.value ?? "";
}
