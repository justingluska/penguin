// Conversation and compose windows: the route a window reads at boot and the
// labels it gets (lib/windowRoute.ts, mirroring src-tauri/src/windows.rs), the
// composer state that moves between windows (features/compose/seed.ts), and
// the Undo a closing conversation window hands to the main one
// (app/remoteUndo.ts).
import { test } from "node:test";
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { parseWindowRoute, threadWindowLabel, windowPagePath } from "../src/lib/windowRoute.ts";
import { asComposeSeed, composeTitle, remainingUndo } from "../src/features/compose/seed.ts";
import { asRemoteUndo } from "../src/app/remoteUndo.ts";

test("the main window has no route; malformed routes fall back to it", () => {
  assert.deepEqual(parseWindowRoute(""), { kind: "main", label: "main" });
  assert.deepEqual(parseWindowRoute("?open=a/b"), { kind: "main", label: "main" });
  // A thread window needs its account, thread and a thread- label.
  assert.equal(parseWindowRoute("?window=thread&label=thread-1&account=a").kind, "main");
  assert.equal(parseWindowRoute("?window=thread&label=main&account=a&thread=t").kind, "main");
  assert.equal(parseWindowRoute("?window=thread&label=thread-1&account=a&thread=t%0A1").kind, "main");
  assert.equal(parseWindowRoute(`?window=thread&label=thread-1&account=a&thread=${"x".repeat(513)}`).kind, "main");
  assert.equal(parseWindowRoute("?window=compose&label=thread-1").kind, "main");
  assert.equal(parseWindowRoute("?window=compose&label=compose-1%20x").kind, "main");
  assert.equal(parseWindowRoute("?window=settings&label=compose-1").kind, "main");
});

test("thread and compose routes read back what the app builds", () => {
  const thread = { accountId: "a+b@northwind.example", threadId: "t 1" };
  const label = threadWindowLabel(thread);
  const path = windowPagePath({ kind: "thread", label, thread });
  assert.deepEqual(parseWindowRoute(path.slice(path.indexOf("?"))), { kind: "thread", label, thread });

  const draft = windowPagePath({ kind: "compose", label: "compose-2", draft: { accountId: "a@x.example", draftId: "r-9" }, fromAccount: "a@x.example" });
  assert.equal(draft, "index.html?window=compose&label=compose-2&account=a%40x.example&draft=r-9");
  assert.deepEqual(parseWindowRoute(draft.slice(draft.indexOf("?"))), {
    kind: "compose",
    label: "compose-2",
    draft: { accountId: "a@x.example", draftId: "r-9" },
    fromAccount: "a@x.example",
  });
  const blank = parseWindowRoute("?window=compose&label=compose-3");
  assert.deepEqual(blank, { kind: "compose", label: "compose-3", draft: null, fromAccount: null });
  const from = parseWindowRoute("?window=compose&label=compose-4&account=b%40x.example");
  assert.deepEqual(from, { kind: "compose", label: "compose-4", draft: null, fromAccount: "b@x.example" });
});

test("a conversation's window label is stable, distinct, and the one windows.rs makes", () => {
  const a = threadWindowLabel({ accountId: "sam@northwind.example", threadId: "t-100" });
  assert.equal(a, "thread-efe6d0e3069c6e6b");
  assert.equal(a, threadWindowLabel({ accountId: "sam@northwind.example", threadId: "t-100" }));
  assert.notEqual(a, threadWindowLabel({ accountId: "sam@northwind.example", threadId: "t-101" }));
  assert.notEqual(threadWindowLabel({ accountId: "ab", threadId: "c" }), threadWindowLabel({ accountId: "a", threadId: "bc" }));
  // windows.rs pins the same value.
  const rust = readFileSync(new URL("../src-tauri/src/windows.rs", import.meta.url), "utf8");
  assert.ok(rust.includes(`"${a}"`));
});

const state = (patch: Record<string, unknown> = {}) => ({
  key: "reply:a@x.example/t1",
  draftId: "r-1",
  draftAccountId: "a@x.example",
  ctx: { mode: "reply", thread: { accountId: "a@x.example", threadId: "t1" }, messageId: "m1" },
  accountId: "a@x.example",
  to: [{ name: "Dana", email: "dana@y.example" }],
  cc: [],
  bcc: [],
  showCc: false,
  showBcc: false,
  subject: "Re: Lunch",
  body: "Friday works.",
  bodyDoc: { type: "doc", content: [] },
  bodyHtml: null,
  quote: { forward: false, header: "On Sep 1, Dana wrote:", text: "Lunch?" },
  attachments: [
    { kind: "file", filename: "menu.pdf", mimeType: "application/pdf", dataBase64: "JVBERg==" },
    { kind: "gmail", messageId: "m9", attachmentId: "a1", filename: "map.png", mimeType: "image/png", size: 10, contentId: "img-1@penguin" },
  ],
  earlier: [],
  original: null,
  remindAfterMs: 86_400_000,
  replyingTo: { name: "Dana", email: "dana@y.example" },
  replyToThreadId: "t1",
  replyToMessageId: "m1",
  touched: true,
  ...patch,
});

test("a composer seed moves everything: recipients, body, files, From, reply context, reminder", () => {
  const seed = asComposeSeed(structuredClone({ v: 1, unsaved: false, state: state() }));
  assert.ok(seed);
  assert.equal(seed.state.accountId, "a@x.example");
  assert.equal(seed.state.draftId, "r-1");
  assert.deepEqual(seed.state.ctx, { mode: "reply", thread: { accountId: "a@x.example", threadId: "t1" }, messageId: "m1" });
  assert.equal(seed.state.attachments.length, 2);
  assert.equal(seed.state.remindAfterMs, 86_400_000);
  assert.equal(seed.state.replyToMessageId, "m1");
  assert.deepEqual(seed.state.bodyDoc, { type: "doc", content: [] });
  assert.equal(seed.unsaved, false);
  // Survives the JSON the Tauri event bus uses.
  assert.deepEqual(asComposeSeed(JSON.parse(JSON.stringify({ v: 1, unsaved: true, state: state() })))?.unsaved, true);
});

test("seeds from another window are checked before use", () => {
  assert.equal(asComposeSeed(null), null);
  assert.equal(asComposeSeed({ v: 2, unsaved: false, state: state() }), null);
  assert.equal(asComposeSeed({ v: 1, state: state() }), null);
  assert.equal(asComposeSeed({ v: 1, unsaved: false, state: state({ ctx: { mode: "settings" } }) }), null);
  assert.equal(asComposeSeed({ v: 1, unsaved: false, state: state({ to: [{ name: "x" }] }) }), null);
  assert.equal(asComposeSeed({ v: 1, unsaved: false, state: state({ attachments: [{ kind: "url", filename: "x", mimeType: "a/b" }] }) }), null);
  assert.equal(asComposeSeed({ v: 1, unsaved: false, state: state({ draftId: 7 }) }), null);
  assert.equal(asComposeSeed({ v: 1, unsaved: false, state: state({ remindAfterMs: "soon" }) }), null);
  // Loose flags read as off.
  const s = asComposeSeed({ v: 1, unsaved: false, state: state({ showCc: "yes", touched: undefined }) });
  assert.equal(s?.state.showCc, false);
  assert.equal(s?.state.touched, false);
});

test("a compose window is titled with the subject", () => {
  assert.equal(composeTitle({ subject: "  Re: Lunch ", ctx: { mode: "reply" } }), "Re: Lunch");
  assert.equal(composeTitle({ subject: "", ctx: { mode: "new" } }), "New Message");
  assert.equal(composeTitle({ subject: " ", ctx: { mode: "forward" } }), "Forward");
  assert.equal(composeTitle({ subject: "", ctx: { mode: "replyAll" } }), "Reply");
});

test("a send handed to the main window keeps what's left of its Undo, never less than a second", () => {
  assert.equal(remainingUndo(10, 0), 10);
  assert.equal(remainingUndo(10, 3_200), 7);
  assert.equal(remainingUndo(10, 9_900), 1);
  assert.equal(remainingUndo(10, 60_000), 1);
  // Undo send off: it goes at once, wherever it is.
  assert.equal(remainingUndo(0, 0), 0);
});

test("an Undo handed over from a closing window is data the main window can check", () => {
  const refs = [{ accountId: "a@x.example", threadId: "t1" }];
  const u = asRemoteUndo({
    message: "Archived",
    steps: [[{ cmd: "modify", refs, action: { kind: "moveToInbox" } }], [], [{ cmd: "snooze", refs, until: 1_800_000_000_000 }]],
  });
  assert.deepEqual(u, {
    message: "Archived",
    steps: [[{ cmd: "modify", refs, action: { kind: "moveToInbox" } }], [{ cmd: "snooze", refs, until: 1_800_000_000_000 }]],
  });
  assert.ok(asRemoteUndo({ message: "Moved", steps: [[{ cmd: "modify", refs, action: { kind: "addLabel", labelId: "L1" } }]] }));
  assert.ok(asRemoteUndo({ message: "Done", steps: [[{ cmd: "replyLater", refs, on: true }], [{ cmd: "dismissFollowUps", refs, dismissed: false }]] }));
  assert.ok(asRemoteUndo({ message: "Snoozed", steps: [[{ cmd: "unsnooze", refs, toInbox: true }]] }));
  // Anything else is refused whole.
  assert.equal(asRemoteUndo({ message: "Archived", steps: [[{ cmd: "delete", refs }]] }), null);
  assert.equal(asRemoteUndo({ message: "Archived", steps: [[{ cmd: "modify", refs, action: { kind: "addLabel" } }]] }), null);
  assert.equal(asRemoteUndo({ message: "Archived", steps: [[{ cmd: "modify", refs: [], action: { kind: "archive" } }]] }), null);
  assert.equal(asRemoteUndo({ message: "", steps: [[{ cmd: "modify", refs, action: { kind: "archive" } }]] }), null);
  assert.equal(asRemoteUndo({ message: "Archived", steps: [] }), null);
});
