// The extension action sheet (features/extensions/ExtensionForm.tsx): it
// runs the action itself, shows a failure inline, shows a confirm the
// program asks (steps, choices, its own button label), and confirming runs
// the same action again with the confirm's state and the picked choice.
import { test } from "node:test";
import assert from "node:assert/strict";
import { win } from "./componentDom.ts";

const { createElement, act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { api } = await import("../src/lib/api.ts");
const { ModalHost } = await import("../src/components/Modal.tsx");
const { ContextMenuHost } = await import("../src/components/ContextMenu.tsx");
const { openExtensionSheet } = await import("../src/features/extensions/ExtensionForm.tsx");
import type { ExtensionResult, RunExtensionRequest } from "../src/lib/types.ts";

const host = document.createElement("div");
document.body.appendChild(host);
const root = createRoot(host as unknown as Element);
act(() => root.render(createElement("div", null, createElement(ModalHost), createElement(ContextMenuHost))));

const action = (form: unknown[]) => ({
  ext: { id: "tasks", name: "Tasks" },
  action: { id: "close", title: "Close task", icon: null, iconSvg: null, includeBody: false, includeThread: true, command: ["run.sh"], form },
});
const comment = { id: "comment", label: "Comment", type: "textarea", default: "", placeholder: null, options: [], required: true };
const message = { accountId: "sam@northwind.example", id: "m1", subject: "Launch", from: { name: "Priya", email: "priya@harbor.example" }, snippet: "" };

const CONFIRM = {
  title: "Close a task with a comment",
  detail: null,
  steps: [{ text: "Comment on OPS-41", url: "https://tasks.example/OPS-41" }, { text: "Move OPS-41 to Done", url: null }],
  choices: [
    { value: "41", label: "Northwind blog brief", detail: "OPS-41" },
    { value: "57", label: "Harbor landing page", detail: null },
  ],
  choiceLabel: "Which task?",
  confirmLabel: "Close task",
  state: '{"n":1}',
};

/** Answer runs from `answers` in order, recording each request. */
function program(answers: (ExtensionResult | Error)[]) {
  const calls: RunExtensionRequest[] = [];
  api.runExtensionAction = async (req: RunExtensionRequest) => {
    calls.push(structuredClone(req));
    const a = answers.shift();
    if (!a) throw new Error("ran too many times");
    if (a instanceof Error) throw { code: "other", message: a.message };
    return a;
  };
  return calls;
}

const $ = <T extends Element = HTMLElement>(sel: string) => document.querySelector<T & HTMLElement>(sel);
const primary = () => $<HTMLButtonElement>(".ext-sheet .ext-run")!;
const settle = () => act(async () => { await new Promise((r) => setTimeout(r, 0)); });
const key = (k: string, opts: KeyboardEventInit = {}) =>
  act(() => {
    (document.activeElement ?? document.body).dispatchEvent(new win.KeyboardEvent("keydown", { key: k, bubbles: true, cancelable: true, ...opts }) as unknown as Event);
  });
const type = (el: HTMLTextAreaElement, text: string) =>
  act(() => {
    const setter = Object.getOwnPropertyDescriptor(win.HTMLTextAreaElement.prototype, "value")!.set!;
    setter.call(el, text);
    el.dispatchEvent(new win.Event("input", { bubbles: true }) as unknown as Event);
  });

test("a form, a confirm with choices, then the answer", async () => {
  const calls = program([
    { message: null, url: null, confirm: CONFIRM },
    { message: "Closed OPS-41", url: "https://tasks.example/OPS-41", confirm: null },
  ]);
  let done: ExtensionResult | null = null;
  act(() => openExtensionSheet(action([comment]) as never, message as never, null, (r) => (done = r)));

  // A one-text-area form: the text area is focused and roomy; running waits for it.
  const area = $<HTMLTextAreaElement>(".ext-sheet textarea")!;
  assert.equal(document.activeElement, area);
  assert.ok(area.classList.contains("is-solo"));
  assert.equal(primary().disabled, true);
  type(area, "Lost the deal.");
  assert.equal(primary().disabled, false);

  // Return in the text area is a new line; ⌘Return runs.
  key("Enter");
  assert.equal(calls.length, 0);
  key("Enter", { metaKey: true });
  await settle();
  assert.equal(calls.length, 1);
  assert.equal(calls[0].continuation, undefined);
  assert.deepEqual(calls[0].form, { comment: "Lost the deal." });

  // The confirm: steps with a link, choices, its own button label, nothing picked yet.
  assert.equal($(".ext-sheet .ext-title")!.textContent, "Close a task with a comment");
  assert.deepEqual([...document.querySelectorAll(".ext-step-text")].map((n) => n.textContent), ["Comment on OPS-41", "Move OPS-41 to Done"]);
  assert.equal(document.querySelectorAll(".ext-step-link").length, 1);
  assert.equal(primary().textContent, "Close task");
  assert.equal(primary().disabled, true, "a choice must be picked first");
  key("Enter");
  await settle();
  assert.equal(calls.length, 1, "Return doesn't confirm without a choice");

  // ↓ picks the first, ↓ again the second; a click picks too.
  key("ArrowDown");
  key("ArrowDown");
  assert.equal($('.ext-choice[aria-checked="true"] .ext-choice-label')!.textContent, "Harbor landing page");
  act(() => $(".ext-choice")!.dispatchEvent(new win.MouseEvent("click", { bubbles: true }) as unknown as Event));
  assert.equal($('.ext-choice[aria-checked="true"] .ext-choice-label')!.textContent, "Northwind blog brief");

  key("Enter");
  await settle();
  assert.equal(calls.length, 2);
  assert.deepEqual(calls[1].continuation, { state: '{"n":1}', choice: "41", round: 1 });
  assert.deepEqual(calls[1].form, calls[0].form, "the same payload again");
  assert.equal(document.querySelector(".ext-sheet"), null);
  assert.deepEqual(done, { message: "Closed OPS-41", url: "https://tasks.example/OPS-41", confirm: null });
});

test("a failed run shows in the sheet, which stays", async () => {
  const calls = program([new Error("Tasks: the board answered 503"), { message: "Closed", url: null, confirm: null }]);
  let done: ExtensionResult | null = null;
  act(() => openExtensionSheet(action([comment]) as never, message as never, null, (r) => (done = r)));
  type($<HTMLTextAreaElement>(".ext-sheet textarea")!, "x");
  act(() => primary().click());
  await settle();
  assert.equal($(".ext-sheet .ext-error")!.textContent, "Tasks: the board answered 503");
  assert.equal(done, null);
  assert.equal(primary().disabled, false, "and can be run again");
  act(() => primary().click());
  await settle();
  assert.equal(calls.length, 2);
  assert.equal(document.querySelector(".ext-sheet"), null);
});

test("each confirm is a round; Esc on a confirm cancels without running", async () => {
  const one = { ...CONFIRM, choices: [], state: "a" };
  const calls = program([
    { message: null, url: null, confirm: one },
    { message: null, url: null, confirm: { ...one, state: "b", confirmLabel: null } },
  ]);
  let done: ExtensionResult | null = null;
  act(() => openExtensionSheet(action([]) as never, message as never, { confirm: one, form: {} }, (r) => (done = r)));
  // No choices: Return confirms straight away.
  assert.equal(primary().disabled, false);
  key("Enter");
  await settle();
  assert.deepEqual(calls[0].continuation, { state: "a", choice: null, round: 1 });
  key("Enter");
  await settle();
  assert.deepEqual(calls[1].continuation, { state: "a", choice: null, round: 2 }, "the second confirm came from round 1's answer");
  // Round 3's confirm (state "b", default button label) is up; Esc leaves it.
  assert.equal(primary().textContent, "Continue");
  key("Escape");
  assert.equal(document.querySelector(".ext-sheet"), null);
  assert.equal(calls.length, 2);
  assert.equal(done, null);
});

test.after(() => act(() => root.unmount()));
