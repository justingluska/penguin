// Lazily loaded screens (src/lib/lazy.ts): nothing until the chunk arrives,
// then the screen; and once preloaded, the very first render is the screen
// itself (no empty frame), which React.lazy can't promise.
import { test } from "node:test";
import assert from "node:assert/strict";
import { Window } from "happy-dom";

const win = new Window();
Object.assign(globalThis, { window: win, document: win.document, HTMLElement: win.HTMLElement, IS_REACT_ACT_ENVIRONMENT: true });
const { createElement, act } = await import("react");
const { createRoot } = await import("react-dom/client");
const { lazyScreen } = await import("../src/lib/lazy.ts");

function mount(el: ReturnType<typeof createElement>) {
  const host = document.createElement("div");
  const root = createRoot(host as unknown as Element);
  act(() => root.render(el));
  return { host, root };
}

test("renders nothing until its chunk loads, then the screen", async () => {
  let release!: () => void;
  const gate = new Promise<void>((r) => (release = r));
  let loads = 0;
  const Screen = lazyScreen(async () => {
    loads++;
    await gate;
    return ({ who }: { who: string }) => createElement("p", null, `hello ${who}`);
  });
  const { host, root } = mount(createElement(Screen, { who: "Sam" }));
  assert.equal(host.innerHTML, "");
  await act(async () => {
    release();
    await gate;
  });
  assert.equal(host.innerHTML, "<p>hello Sam</p>");
  act(() => root.render(createElement(Screen, { who: "Ana" })));
  assert.equal(host.innerHTML, "<p>hello Ana</p>");
  assert.equal(loads, 1, "the chunk is fetched once");
  act(() => root.unmount());
});

test("preloaded: the first render is already the screen", async () => {
  const Screen = lazyScreen(async () => () => createElement("b", null, "ready"));
  await Screen.preload();
  const { host, root } = mount(createElement(Screen, {}));
  assert.equal(host.innerHTML, "<b>ready</b>");
  act(() => root.unmount());
});

test("a failed load is retried on the next render instead of sticking", async () => {
  let tries = 0;
  const Screen = lazyScreen(async () => {
    if (++tries === 1) throw new Error("chunk failed");
    return () => createElement("i", null, "second try");
  });
  await assert.rejects(Screen.preload(), /chunk failed/);
  await Screen.preload();
  const { host, root } = mount(createElement(Screen, {}));
  assert.equal(host.innerHTML, "<i>second try</i>");
  act(() => root.unmount());
});
