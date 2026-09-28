// Read receipts for mock sent messages (shape of penguin_core::store::ReadReceipt).
// Fictional people and .example domains only.
import type { Address, ReadReceipt } from "../types";

/** Threads whose sent messages got a receipt back, and how many minutes after sending. */
const RECEIPT_THREADS: Record<string, number> = {
  "t-pricing-copy": 22,
};

/** Receipts for a message `from` one of your accounts in thread `threadId`, sent at `date`. */
export function mockReceipts(threadId: string, messageId: string, date: number, to: Address[], fromMe: boolean): ReadReceipt[] {
  const after = RECEIPT_THREADS[threadId];
  if (!fromMe || after === undefined || to.length === 0) return [];
  return [
    {
      originalMessageId: `${messageId}@mail.example`,
      recipient: to[0].email.toLowerCase(),
      disposition: "displayed",
      date: date + after * 60_000,
      messageId: `${messageId}-mdn`,
    },
  ];
}
