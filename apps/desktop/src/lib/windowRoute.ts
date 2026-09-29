// Which window this page is (pure; tests/windows.test.ts). Every Penguin
// window loads the same index.html; conversation and compose windows carry a
// query naming what they show (src-tauri/src/windows.rs `page_path` builds
// it, and the browser mock builds the same one), and the UI renders a small
// shell for them instead of the mail shell (app/windowShell.tsx).
//
//   index.html                                            the main window
//   index.html?window=thread&label=…&account=…&thread=…    one conversation
//   index.html?window=compose&label=…[&account=…][&draft=…]  one composer
import type { ThreadRef } from "./types";

export type WindowRoute =
  | { kind: "main"; label: "main" }
  | { kind: "thread"; label: string; thread: ThreadRef }
  | {
      kind: "compose";
      label: string;
      /** A saved draft to open (when there's no seed to take). */
      draft: { accountId: string; draftId: string } | null;
      /** A new message's From account (the main window's account filter when it was opened). */
      fromAccount: string | null;
    };

export const MAIN_LABEL = "main";
export const THREAD_PREFIX = "thread-";
export const COMPOSE_PREFIX = "compose-";

/** Same limit as windows.rs: ids longer than this are refused. */
const MAX_ID = 512;
// A Tauri window label: letters, digits, "-", "/", ":" and "_".
const LABEL = /^[A-Za-z0-9\-/:_]{1,64}$/;

const MAIN: WindowRoute = { kind: "main", label: MAIN_LABEL };

function id(v: string | null): string | null {
  // Control characters are never part of an id (windows.rs refuses them too).
  return v && v.length <= MAX_ID && !/[\u0000-\u001f\u007f]/.test(v) ? v : null;
}

/**
 * Read the route from `location.search`. Anything malformed is the main
 * window: a page that can't tell what it should show shows the whole app.
 */
export function parseWindowRoute(search: string): WindowRoute {
  const q = new URLSearchParams(search);
  const kind = q.get("window");
  const label = q.get("label") ?? "";
  if (kind === "thread") {
    const accountId = id(q.get("account"));
    const threadId = id(q.get("thread"));
    if (!accountId || !threadId || !label.startsWith(THREAD_PREFIX) || !LABEL.test(label)) return MAIN;
    return { kind: "thread", label, thread: { accountId, threadId } };
  }
  if (kind === "compose") {
    if (!label.startsWith(COMPOSE_PREFIX) || !LABEL.test(label)) return MAIN;
    const accountId = id(q.get("account"));
    const draftId = id(q.get("draft"));
    return { kind: "compose", label, draft: accountId && draftId ? { accountId, draftId } : null, fromAccount: accountId };
  }
  return MAIN;
}

/** 64-bit FNV-1a of UTF-8 bytes, as 16 hex digits (windows.rs `fnv1a`). */
function fnv1a(s: string): string {
  let h = 0xcbf29ce484222325n;
  for (const b of new TextEncoder().encode(s)) {
    h ^= BigInt(b);
    h = (h * 0x100000001b3n) & 0xffffffffffffffffn;
  }
  return h.toString(16).padStart(16, "0");
}

/** The one window label for a conversation; must equal windows.rs `thread_label`. */
export function threadWindowLabel(ref: ThreadRef): string {
  return THREAD_PREFIX + fnv1a(`${ref.accountId}\u0000${ref.threadId}`);
}

/** The page a window loads (windows.rs `page_path`), for the browser mock. */
export function windowPagePath(route: Exclude<WindowRoute, { kind: "main" }>): string {
  const q = new URLSearchParams();
  q.set("window", route.kind);
  q.set("label", route.label);
  if (route.kind === "thread") {
    q.set("account", route.thread.accountId);
    q.set("thread", route.thread.threadId);
  } else {
    const account = route.draft?.accountId ?? route.fromAccount;
    if (account) q.set("account", account);
    if (route.draft) q.set("draft", route.draft.draftId);
  }
  return `index.html?${q.toString()}`;
}
