// Thread summaries in the UI: one card state per thread (model.ts), the
// Summarize action, cancelling, the cached summary on open, availability
// and the ⇧S shortcut. OWNER: summaries. Backend: src-tauri/src/summary/.
import { useEffect, useSyncExternalStore } from "react";
import { api, asCommandError, onMailChanged, onSummaryProgress } from "../../lib/api";
import type { AiAvailability, ThreadRef } from "../../lib/types";
import { currentSettings, subscribeSettings, useSetting } from "../../lib/settings";
import { registerShortcuts } from "../../lib/keyboard";
import { getUi } from "../../lib/ui";
import { EMPTY, reduce, toggleIntent, type CardAction, type CardState } from "./model";

const key = (r: ThreadRef) => r.accountId + "\u0000" + r.threadId;
const states = new Map<string, CardState>();
const subs = new Set<() => void>();

function notify() {
  subs.forEach((f) => f());
}

function dispatch(ref: ThreadRef, action: CardAction) {
  const k = key(ref);
  const before = states.get(k) ?? EMPTY;
  const after = reduce(before, action);
  if (after === before) return;
  states.set(k, after);
  notify();
}

export function cardState(ref: ThreadRef): CardState {
  return states.get(key(ref)) ?? EMPTY;
}

export function useCard(ref: ThreadRef): CardState {
  ensureListening();
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => cardState(ref),
  );
}

// ---------------------------------------------------------------------------
// Events
// ---------------------------------------------------------------------------

let listening = false;
function ensureListening() {
  if (listening) return;
  listening = true;
  void onSummaryProgress((p) => dispatch({ accountId: p.accountId, threadId: p.threadId }, { type: "progress", progress: p }));
  // A reply makes the stored summary stale: read it again for threads that show one.
  void onMailChanged((e) => {
    for (const threadId of e.threadIds) {
      const ref = { accountId: e.accountId, threadId };
      if (cardState(ref).summary) void loadCached(ref);
    }
  });
  subscribeSettings(() => {
    if (!currentSettings().summaries && states.size > 0) {
      states.clear();
      notify();
    }
  });
}

// ---------------------------------------------------------------------------
// Availability: asked once, again when Settings → AI opens, and at most once
// a minute when a thread opens (Apple Intelligence may finish downloading or
// be switched on while Penguin runs).
// ---------------------------------------------------------------------------

let availability: AiAvailability | null = null;
let checkedAt = 0;
let checking: Promise<void> | null = null;
const availSubs = new Set<() => void>();

export function refreshAvailability(force = false): Promise<void> {
  if (checking) return checking;
  if (!force && availability && Date.now() - checkedAt < 60_000) return Promise.resolve();
  checking = api
    .summaryAvailability()
    .then(
      (a) => {
        availability = a;
      },
      () => {
        availability = { available: false, reason: "unknown", contextTokens: 0 };
      },
    )
    .finally(() => {
      checkedAt = Date.now();
      checking = null;
      availSubs.forEach((f) => f());
    });
  return checking;
}

export function useAvailability(): AiAvailability | null {
  useEffect(() => void refreshAvailability(), []);
  return useSyncExternalStore(
    (cb) => {
      availSubs.add(cb);
      return () => availSubs.delete(cb);
    },
    () => availability,
  );
}

/** The Summarize action is offered: the setting is on and the model is available. */
export function canSummarize(): boolean {
  return currentSettings().summaries && availability?.available === true;
}

export function useCanSummarize(): boolean {
  const on = useSetting("summaries");
  const a = useAvailability();
  return on && a?.available === true;
}

// ---------------------------------------------------------------------------
// Actions
// ---------------------------------------------------------------------------

/** Show the stored summary, if there is one (local only). */
export async function loadCached(ref: ThreadRef) {
  if (!currentSettings().summaries) return;
  try {
    const s = await api.cachedSummary(ref.accountId, ref.threadId);
    dispatch(ref, { type: "cached", summary: s });
  } catch {
    // A missing cache is no summary; the Summarize action still works.
  }
}

/** Make the summary (again), streaming into the card. */
export async function summarize(ref: ThreadRef) {
  if (cardState(ref).run) return;
  ensureListening();
  dispatch(ref, { type: "start" });
  try {
    const s = await api.summarizeThread(ref.accountId, ref.threadId);
    dispatch(ref, { type: "done", summary: s });
  } catch (e) {
    const err = asCommandError(e);
    dispatch(ref, err.code === "cancelled" ? { type: "cancelled" } : { type: "failed", message: err.message });
  }
}

export function cancel(ref: ThreadRef) {
  void api.cancelSummary(ref.accountId, ref.threadId).catch(() => {});
}

export function close(ref: ThreadRef) {
  dispatch(ref, { type: "close" });
}

/** ⇧S and the toolbar button: summarize, show the one already made, or hide it. */
export function toggle(ref: ThreadRef) {
  const s = cardState(ref);
  switch (toggleIntent(s)) {
    case "summarize":
      void summarize(ref);
      break;
    case "show":
      dispatch(ref, { type: "show" });
      break;
    case "close":
      close(ref);
      break;
    case "nothing":
      break;
  }
}

let lastPrewarm = 0;
/** The pointer is on Summarize: a strong hint (Apple: prewarm then, with a second or more to spare). */
export function prewarm() {
  if (!canSummarize() || Date.now() - lastPrewarm < 60_000) return;
  lastPrewarm = Date.now();
  void api.prewarmSummarizer().catch(() => {});
}

// ---------------------------------------------------------------------------
// Keyboard: ⇧S while a thread is showing (preview pane or opened). Registered
// by the thread pane, so it exists only while there is a thread to summarize.
// ---------------------------------------------------------------------------

export const SUMMARIZE_KEYS = "shift+s";

export function useSummaryShortcut() {
  useEffect(
    () =>
      registerShortcuts([
        {
          id: "thread.summarize",
          keys: SUMMARIZE_KEYS,
          label: "Summarize conversation",
          group: "Triage",
          when: () => {
            const ui = getUi();
            return (ui.overlay === null || ui.overlay === "command") && ui.selected !== null && canSummarize();
          },
          run: () => {
            const ref = getUi().selected;
            if (ref) toggle(ref);
          },
        },
      ]),
    [],
  );
}
