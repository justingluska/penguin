// What the "Read by …" line on a sent message says (pure; tests/receipts.test.ts).
// Receipts come from src-tauri/src/receipts.rs; why Penguin uses receipts and
// not tracking pixels is docs/PRIVACY.md.
import type { Address, MessageView, ReadReceipt } from "../../lib/types";

export interface ReceiptLine {
  /** "Read by Alex Rivera", "Read by Alex and Jo", "Read by 3 people". */
  text: string;
  /** Newest receipt's time (unix ms). */
  at: number;
  /** One line per receipt for the tooltip: "alex@harbor.example: read Tue 3:12 PM". */
  details: { who: string; what: string; at: number }[];
}

/** A receipt's disposition in words. */
export function dispositionText(d: string): string {
  switch (d) {
    case "displayed":
      return "read";
    case "deleted":
      return "deleted without reading";
    case "dispatched":
      return "sent on";
    case "processed":
      return "processed";
    default:
      return d;
  }
}

/** The display name for a receipt's sender, from the message's recipients. */
function nameFor(r: ReadReceipt, recipients: Address[]): string {
  if (!r.recipient) return "a recipient";
  const hit = recipients.find((a) => a.email.toLowerCase() === r.recipient);
  return hit?.name?.trim() || r.recipient;
}

function people(names: string[]): string {
  if (names.length === 1) return names[0];
  if (names.length === 2) return `${names[0]} and ${names[1]}`;
  return `${names.length} people`;
}

/** The line for a sent message, or null without receipts. One entry per person (their latest receipt). */
export function receiptLine(m: Pick<MessageView, "readReceipts" | "to" | "cc" | "bcc">): ReceiptLine | null {
  const all = m.readReceipts ?? [];
  if (all.length === 0) return null;
  const recipients = [...m.to, ...m.cc, ...m.bcc];
  const latest = new Map<string, ReadReceipt>();
  for (const r of all) {
    const k = r.recipient ?? `?${r.messageId}`;
    const prev = latest.get(k);
    if (!prev || r.date >= prev.date) latest.set(k, r);
  }
  const list = [...latest.values()].sort((a, b) => a.date - b.date);
  const read = list.filter((r) => r.disposition === "displayed");
  const text = read.length
    ? `Read by ${people(read.map((r) => nameFor(r, recipients)))}`
    : `${people(list.map((r) => nameFor(r, recipients)))}: ${dispositionText(list[list.length - 1].disposition)}`;
  return {
    text,
    at: list[list.length - 1].date,
    details: list.map((r) => ({ who: r.recipient ?? "A recipient", what: dispositionText(r.disposition), at: r.date })),
  };
}

/** Said with every receipt: what it does and doesn't mean. */
export const RECEIPT_NOTE =
  "Read receipts come back only when the recipient's mail app supports them and they agree to send one. No receipt doesn't mean unread.";
