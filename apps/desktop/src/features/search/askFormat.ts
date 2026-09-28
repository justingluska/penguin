// OWNER: ask agent. Pure formatting for the Ask card's fact cards and
// quoted passages (tested in tests/ask.test.ts).

/** Local wall time "2026-10-02T19:05" (or a date) as a Date in local time. */
export function parseLocal(at: string | null | undefined): { date: Date; hasTime: boolean } | null {
  if (!at) return null;
  const m = /^(\d{4})-(\d{2})-(\d{2})(?:T(\d{2}):(\d{2}))?/.exec(at);
  if (!m) return null;
  const [, y, mo, d, h, mi] = m;
  const date = new Date(Number(y), Number(mo) - 1, Number(d), h ? Number(h) : 0, mi ? Number(mi) : 0);
  return { date, hasTime: h !== undefined };
}

/** "7:05 PM" for a local wall time, or "" without one. */
export function fmtTime(at: string | null | undefined): string {
  const p = parseLocal(at);
  if (!p || !p.hasTime) return "";
  return p.date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
}

/** "Fri, Oct 2" (with the year when it isn't this year). */
export function fmtDay(at: string | null | undefined, now = Date.now()): string {
  const p = parseLocal(at);
  if (!p) return at ?? "";
  const sameYear = p.date.getFullYear() === new Date(now).getFullYear();
  return p.date.toLocaleDateString(undefined, { weekday: "short", month: "short", day: "numeric", ...(sameYear ? {} : { year: "numeric" }) });
}

/** "Fri, Oct 2 · 7:05 PM". */
export function fmtWhen(at: string | null | undefined, now = Date.now()): string {
  const t = fmtTime(at);
  return t ? `${fmtDay(at, now)} · ${t}` : fmtDay(at, now);
}

/** Whole days from today to a local date (negative = past). */
export function daysUntil(at: string | null | undefined, now = Date.now()): number | null {
  const p = parseLocal(at);
  if (!p) return null;
  const a = new Date(now);
  const today = new Date(a.getFullYear(), a.getMonth(), a.getDate()).getTime();
  const d = new Date(p.date.getFullYear(), p.date.getMonth(), p.date.getDate()).getTime();
  return Math.round((d - today) / 86_400_000);
}

export function money(v: number, currency: string): string {
  try {
    return new Intl.NumberFormat(undefined, { style: "currency", currency }).format(v);
  } catch {
    return `${v.toFixed(2)} ${currency}`;
  }
}

/** Split `text` into plain and highlighted runs at `marks` (JS offsets). */
export function markSegments(text: string, marks: [number, number][]): { text: string; mark: boolean }[] {
  const sorted = [...marks].filter(([a, b]) => a < b && b <= text.length).sort((x, y) => x[0] - y[0]);
  const out: { text: string; mark: boolean }[] = [];
  let at = 0;
  for (const [a, b] of sorted) {
    if (a < at) continue;
    if (a > at) out.push({ text: text.slice(at, a), mark: false });
    out.push({ text: text.slice(a, b), mark: true });
    at = b;
  }
  if (at < text.length) out.push({ text: text.slice(at), mark: false });
  return out;
}

/** Parcel progress: 0 shipped, 1 in transit, 2 out for delivery, 3 delivered; -1 problem. */
export function shipStep(status: string | null): number {
  switch (status) {
    case "delivered":
      return 3;
    case "outForDelivery":
      return 2;
    case "inTransit":
      return 1;
    case "exception":
      return -1;
    default:
      return 0;
  }
}

export const SHIP_STEPS = ["Shipped", "In transit", "Out for delivery", "Delivered"];

/** Tone of a card's status chip. */
export function statusTone(status: string | null): "green" | "amber" | "red" | "gray" | "blue" {
  if (!status) return "gray";
  const s = status.toLowerCase();
  if (s.startsWith("cancel") || s === "overdue" || s.includes("problem")) return "red";
  if (s === "delivered" || s === "paid") return "green";
  if (s.startsWith("due") || s.includes("tomorrow") || s === "today" || s.includes("out for delivery")) return "amber";
  if (s.includes("ago")) return "gray";
  return "blue";
}
