// Report spam / Not spam (src/app/spam.ts): what Undo sends, when ! means
// Not spam, and that the undo survives the hand-off from a conversation
// window (app/remoteUndo.ts).
import { test } from "node:test";
import assert from "node:assert/strict";
import { reportSpamUndo, targetsAreSpam } from "../src/app/spam.ts";
import { asRemoteUndo } from "../src/app/remoteUndo.ts";

const a = { accountId: "acc-1", threadId: "t-inbox" };
const b = { accountId: "acc-1", threadId: "t-done" };
const c = { accountId: "acc-2", threadId: "t-inbox-2" };

test("undoing Report spam puts each conversation back where it was", () => {
  const inbox = new Set([a.threadId, c.threadId]);
  const steps = reportSpamUndo([a, b, c], (r) => inbox.has(r.threadId));
  assert.deepEqual(steps, [
    [
      { cmd: "modify", refs: [a, c], action: { kind: "notSpam" } },
      { cmd: "modify", refs: [b], action: { kind: "removeLabel", labelId: "SPAM" } },
    ],
  ]);
  assert.deepEqual(reportSpamUndo([b], () => false), [[{ cmd: "modify", refs: [b], action: { kind: "removeLabel", labelId: "SPAM" } }]]);
  assert.deepEqual(reportSpamUndo([], () => true), []);
});

test("the undo crosses windows intact", () => {
  const steps = reportSpamUndo([a, b], (r) => r === a);
  assert.deepEqual(asRemoteUndo({ message: "Reported as spam", steps }), { message: "Reported as spam", steps });
  const back = asRemoteUndo({ message: "Not spam: moved to inbox", steps: [[{ cmd: "modify", refs: [a], action: { kind: "reportSpam" } }]] });
  assert.equal(back?.steps[0][0].cmd, "modify");
});

test("! means Not spam in the Spam view, or when every target is spam", () => {
  assert.equal(targetsAreSpam("spam", []), true);
  assert.equal(targetsAreSpam("inbox", [["SPAM", "UNREAD"], ["SPAM"]]), true);
  assert.equal(targetsAreSpam("all", [["SPAM"], ["INBOX"]]), false);
  assert.equal(targetsAreSpam("inbox", [["SPAM"], null]), false);
  assert.equal(targetsAreSpam("inbox", []), false);
});
