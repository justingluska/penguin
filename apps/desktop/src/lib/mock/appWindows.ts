// Conversation and compose windows in the browser mock: `window.open` on the
// same page with the query src-tauri/src/windows.rs builds, so the feature
// can be tried (and driven by Playwright) in `npm run dev:mock`. Runs in the
// main window, which hosts the mock for every window (lib/mockBridge.ts).
import type { OpenedWindow, WindowRequest } from "../types";
import { COMPOSE_PREFIX, threadWindowLabel, windowPagePath } from "../windowRoute";
import type { MockHandler } from "./index";

const open = new Map<string, Window>();
const seeds = new Map<string, unknown>();
let composeSeq = 1;

function bad(message: string): never {
  throw { code: "invalidInput", message: `open_window: ${message}` };
}

function checkId(what: string, v: unknown): string {
  // Same rules as windows.rs `check_id`.
  if (typeof v !== "string" || !v || v.length > 512 || /[\u0000-\u001f\u007f]/.test(v)) bad(`invalid ${what}`);
  return v;
}

export const appWindowHandlers: Record<string, MockHandler> = {
  open_window: ({ request }) => {
    const req = request as WindowRequest;
    let label: string;
    let size: [number, number];
    if (req?.kind === "thread") {
      const thread = { accountId: checkId("account id", req.accountId), threadId: checkId("thread id", req.threadId) };
      label = threadWindowLabel(thread);
      size = [920, 840];
      const w = open.get(label);
      if (w && !w.closed) {
        w.focus();
        return { label, existing: true } satisfies OpenedWindow;
      }
      open.set(label, popup(windowPagePath({ kind: "thread", label, thread }), label, size));
      return { label, existing: false } satisfies OpenedWindow;
    }
    if (req?.kind !== "compose") bad("unknown window kind");
    const accountId = req.accountId == null ? null : checkId("account id", req.accountId);
    const draftId = req.draftId == null ? null : checkId("draft id", req.draftId);
    if (draftId && !accountId) bad("a draft needs its account id");
    if (req.seed !== undefined && req.seed !== null && (typeof req.seed !== "object" || Array.isArray(req.seed))) bad("the seed must be an object");
    label = `${COMPOSE_PREFIX}${composeSeq++}`;
    size = [780, 720];
    if (req.seed != null) seeds.set(label, structuredClone(req.seed));
    const draft = accountId && draftId ? { accountId, draftId } : null;
    open.set(label, popup(windowPagePath({ kind: "compose", label, draft, fromAccount: accountId }), label, size));
    return { label, existing: false } satisfies OpenedWindow;
  },
  take_window_seed: ({ label }) => {
    const seed = seeds.get(String(label)) ?? null;
    seeds.delete(String(label));
    return seed;
  },
};

/** Cascade from this window, like the app does from the main window. */
function popup(path: string, label: string, [w, h]: [number, number]): Window {
  const n = [...open.values()].filter((x) => !x.closed).length;
  const step = 28 * (1 + (n % 8));
  const left = Math.round((window.screenX || 0) + step);
  const top = Math.round((window.screenY || 0) + step);
  const url = new URL(path, location.href).toString();
  const win = window.open(url, label, `popup,width=${w},height=${h},left=${left},top=${top}`);
  if (!win) throw { code: "other", message: "The browser blocked the new window (allow pop-ups for this page)" };
  return win;
}
