// <Select> (components/Select.tsx) through the shared menu host: the list
// closes on Esc (without the dialog under it seeing the key), on a press
// outside, and on a second press on its own button; and the extension form
// doesn't wrap it in a <label>, whose forwarded clicks reopened it.
import { test } from "node:test";
import assert from "node:assert/strict";
import { win } from "./componentDom.ts";
const { createElement, act, useState } = await import("react");
const { createRoot } = await import("react-dom/client");
const { flushSync } = await import("react-dom");
const { Select } = await import("../src/components/Select.tsx");
const { ContextMenuHost } = await import("../src/components/ContextMenu.tsx");

function Harness() {
  const [v, setV] = useState("ops");
  return createElement(
    "div",
    null,
    createElement("p", { id: "outside" }, "elsewhere"),
    createElement(Select<string>, {
      label: "Board",
      value: v,
      onChange: setV,
      options: [
        { value: "ops", label: "Operations" },
        { value: "web", label: "Website" },
      ],
    }),
    createElement(ContextMenuHost),
  );
}

function mount() {
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host as unknown as Element);
  act(() => root.render(createElement(Harness)));
  const button = host.querySelector<HTMLButtonElement>("button.select")!;
  return { host, root, button };
}

const press = (el: Element) => {
  // The real order: the window's capture listeners first, then React's.
  act(() => {
    el.dispatchEvent(new win.MouseEvent("mousedown", { bubbles: true, cancelable: true, button: 0, detail: 1 }) as unknown as Event);
  });
  act(() => {
    el.dispatchEvent(new win.MouseEvent("click", { bubbles: true, cancelable: true, button: 0, detail: 1 }) as unknown as Event);
  });
};
const listOpen = () => document.querySelectorAll('[role="listbox"]').length === 1;

test("Esc closes only the list; the dialog's own Esc handler never sees it", () => {
  const { root, button } = mount();
  let dialogSawEsc = false;
  const dialogKeys = (e: KeyboardEvent) => e.key === "Escape" && (dialogSawEsc = true);
  // A dialog (Modal.tsx) listens in the capture phase, added after the menu's.
  window.addEventListener("keydown", dialogKeys, true);
  press(button);
  assert.ok(listOpen());
  act(() => {
    document.body.dispatchEvent(new win.KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }) as unknown as Event);
  });
  assert.ok(!listOpen());
  assert.equal(button.getAttribute("aria-expanded"), "false");
  assert.equal(dialogSawEsc, false);
  window.removeEventListener("keydown", dialogKeys, true);
  act(() => root.unmount());
});

test("a press outside closes the list", () => {
  const { root, host, button } = mount();
  press(button);
  assert.ok(listOpen());
  press(host.querySelector("#outside")!);
  assert.ok(!listOpen());
  act(() => root.unmount());
});

test("a second press on the button closes the list instead of reopening it", () => {
  const { root, button } = mount();
  press(button);
  assert.ok(listOpen());
  // A browser runs microtasks between the window's capture listeners and
  // React's root listener, so React re-renders with whatever the menu's
  // outside-press handler did before the button's onMouseDown runs. Here
  // everything runs in one dispatch, so flush at that point by hand.
  const checkpoint = () => flushSync(() => {});
  window.addEventListener("mousedown", checkpoint, true);
  press(button.querySelector(".select-value")!);
  window.removeEventListener("mousedown", checkpoint, true);
  assert.ok(!listOpen(), "the outside-press handler closed it and the button's press reopened it");
  assert.equal(button.getAttribute("aria-expanded"), "false");
  press(button);
  assert.ok(listOpen(), "and a third press opens it again");
  act(() => root.unmount());
});

test("in the extension form, a click on the field's name doesn't reopen the list", async () => {
  const { openExtensionSheet } = await import("../src/features/extensions/ExtensionForm.tsx");
  const { ModalHost } = await import("../src/components/Modal.tsx");
  const host = document.createElement("div");
  document.body.appendChild(host);
  const root = createRoot(host as unknown as Element);
  act(() => root.render(createElement("div", null, createElement(ModalHost), createElement(ContextMenuHost))));
  const field = (id: string, label: string, type: string, options: { value: string; label: string }[] = []) => ({ id, label, type, default: null, placeholder: null, options, required: false });
  const a = {
    ext: { id: "tasks", name: "Tasks" },
    action: {
      id: "create",
      title: "Create task",
      icon: null,
      iconSvg: null,
      includeBody: false,
      command: ["run.sh"],
      form: [field("title", "Title", "text"), field("board", "Board", "select", [{ value: "ops", label: "Operations" }, { value: "web", label: "Website" }])],
    },
  };
  let answer: unknown = "pending";
  act(() => {
    openExtensionSheet(a as never, { subject: "Hello", from: { name: "Priya", email: "priya@harbor.example" }, snippet: "" } as never, null, (v) => (answer = v));
  });
  const button = document.querySelector<HTMLButtonElement>(".ext-sheet button.select")!;
  const name = [...document.querySelectorAll(".ext-sheet .ext-label")].find((n) => n.textContent === "Board")!;
  press(button);
  assert.ok(listOpen());
  press(name);
  assert.ok(!listOpen(), "the click on the field's name opened the list again");
  assert.ok(document.querySelector(".ext-sheet"), "the dialog stays");
  act(() => {
    document.body.dispatchEvent(new win.KeyboardEvent("keydown", { key: "Escape", bubbles: true, cancelable: true }) as unknown as Event);
  });
  await act(async () => {});
  assert.equal(document.querySelector(".ext-sheet"), null, "with the list closed, Esc cancels the dialog");
  assert.equal(answer, "pending", "and nothing ran");
  act(() => root.unmount());
});
