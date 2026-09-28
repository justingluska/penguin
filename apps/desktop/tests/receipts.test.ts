// "Read by …" on a sent message (features/receipts/receipts.ts).
import { test } from "node:test";
import assert from "node:assert/strict";
import { RECEIPT_NOTE, dispositionText, receiptLine } from "../src/features/receipts/receipts.ts";
import type { ReadReceipt } from "../src/lib/types.ts";

const alex = { name: "Alex Rivera", email: "Alex@Harbor.example" };
const jo = { name: "Jo Park", email: "jo@harbor.example" };
const r = (recipient: string | null, date: number, disposition = "displayed"): ReadReceipt => ({
  originalMessageId: "abc@mail.example",
  recipient,
  disposition,
  date,
  messageId: `mdn-${recipient}-${date}`,
});

test("no receipts, no line", () => {
  assert.equal(receiptLine({ to: [alex], cc: [], bcc: [] }), null);
  assert.equal(receiptLine({ to: [alex], cc: [], bcc: [], readReceipts: [] }), null);
});

test("one reader is named from the recipients, by address case-insensitively", () => {
  const line = receiptLine({ to: [alex], cc: [], bcc: [], readReceipts: [r("alex@harbor.example", 1000)] })!;
  assert.equal(line.text, "Read by Alex Rivera");
  assert.equal(line.at, 1000);
  assert.deepEqual(line.details, [{ who: "alex@harbor.example", what: "read", at: 1000 }]);
});

test("several readers; repeats count once, the latest wins", () => {
  const two = receiptLine({
    to: [alex],
    cc: [jo],
    bcc: [],
    readReceipts: [r("alex@harbor.example", 1000), r("jo@harbor.example", 2000), r("alex@harbor.example", 3000)],
  })!;
  assert.equal(two.text, "Read by Jo Park and Alex Rivera");
  assert.equal(two.at, 3000);
  const three = receiptLine({ to: [alex, jo], cc: [], bcc: [], readReceipts: [r("a@x.example", 1), r("b@x.example", 2), r("c@x.example", 3)] })!;
  assert.equal(three.text, "Read by 3 people");
});

test("a receipt that isn't a read says what happened, and an unnamed one is 'a recipient'", () => {
  const line = receiptLine({ to: [alex], cc: [], bcc: [], readReceipts: [r(null, 5, "deleted")] })!;
  assert.equal(line.text, "a recipient: deleted without reading");
  assert.equal(dispositionText("displayed"), "read");
  assert.equal(dispositionText("something-new"), "something-new");
  assert.match(RECEIPT_NOTE, /No receipt doesn't mean unread/);
});
