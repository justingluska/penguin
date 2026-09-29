// Messages between Penguin's windows (each window runs its own JS). The
// transport is api.ts's (busPost / onBusMessage): inside Tauri, app events
// targeted at a window label, which every window listens for under its own
// label only; in a browser (the developer mock), a BroadcastChannel, which
// same-origin tabs and popups share. Two things ride on it:
//   - hand-offs to the main window (app/handoffs.ts, app/windowShell.tsx,
//     features/compose/send.ts): a send to count down, an Undo to offer, a
//     draft that couldn't be saved;
//   - in mock and demo mode, the mock backend: it lives in the main window,
//     and the other windows call it through here (lib/mockBridge.ts), so
//     every window sees the same mailbox.
import { busPost, onBusMessage } from "./api";
import { thisWindowLabel } from "./thisWindow";

export { isMainWindow, thisWindowLabel, windowRoute } from "./thisWindow";

export interface BusMessage {
  /** A window label, or "*" for every window. */
  to: string;
  from: string;
  type: string;
  [key: string]: unknown;
}

type Handler = (m: BusMessage) => void;
const handlers = new Set<Handler>();
let started = false;

function deliver(m: unknown) {
  if (!m || typeof m !== "object") return;
  const msg = m as BusMessage;
  if (typeof msg.type !== "string" || msg.from === thisWindowLabel) return;
  if (msg.to !== thisWindowLabel && msg.to !== "*") return;
  handlers.forEach((h) => h(msg));
}

function start() {
  if (started) return;
  started = true;
  onBusMessage(deliver);
}

/** Listen for messages to this window. */
export function onBus(h: Handler): () => void {
  start();
  handlers.add(h);
  return () => {
    handlers.delete(h);
  };
}

/** Send a message to another window (or "*": all of them). */
export function busSend(to: string, type: string, body: Record<string, unknown> = {}): Promise<void> {
  start();
  return busPost(to, { ...body, to, from: thisWindowLabel, type });
}

let askSeq = 0;

/**
 * Send a message that must be taken: resolves true once the other window
 * acknowledges it (`ackBus`), false after `timeoutMs` (then the sender keeps
 * what it was handing over).
 */
export function busAsk(to: string, type: string, body: Record<string, unknown>, timeoutMs: number): Promise<boolean> {
  const ask = `${thisWindowLabel}:${++askSeq}`;
  return new Promise((resolve) => {
    const off = onBus((m) => {
      if (m.type === "ack" && m.ask === ask) finish(true);
    });
    const timer = setTimeout(() => finish(false), timeoutMs);
    function finish(ok: boolean) {
      off();
      clearTimeout(timer);
      resolve(ok);
    }
    busSend(to, type, { ...body, ask }).catch(() => finish(false));
  });
}

/** Acknowledge a `busAsk` message. */
export function ackBus(m: BusMessage): void {
  if (typeof m.ask === "string") void busSend(m.from, "ack", { ask: m.ask });
}
