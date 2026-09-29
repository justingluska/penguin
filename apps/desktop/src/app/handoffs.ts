// The main window's side of closing windows (the other side is
// app/windowShell.tsx and features/compose/send.ts). A conversation or
// compose window that goes away hands over what must outlive it:
//   send        a message whose Undo countdown hasn't run out: it counts down
//               here (features/compose/send.ts receiveSend);
//   undo        the Undo of the archive/trash/snooze/move that closed a
//               conversation window: offered here as the same toast and Z,
//               and it reopens the conversation's window;
//   draft-kept  a draft that couldn't be saved (offline): kept here, saving
//               again until it reaches Drafts.
// Each is acknowledged, so the sender knows it may close (lib/windowBus.ts busAsk).
// And what only the main window has, asked for from another window: Settings
// (open-settings), signing in again (reconnect), search (open-search). The
// main window comes to the front for them.
import { ackBus, onBus } from "../lib/windowBus";
import type { ThreadRef } from "../lib/types";
import { dismissToast, toast } from "../components/Toast";
import { asComposeSeed, type ComposeSeed } from "../features/compose/seed";
import { openSaverInWindow, receiveSend } from "../features/compose/send";
import { saverFor } from "../features/compose/autosave";
import { asRemoteUndo, type RemoteUndo } from "./remoteUndo";
import { fail, isPendingUndo, runUndoSteps, setUndo, undo } from "./actions";
import { openThreadWindow } from "./windows";
import { setUi } from "../lib/ui";
import { focusMainWindow } from "../lib/windowChrome";
import { asSettingsSection, openSettings } from "../features/settings/state";
import { openReconnect } from "../features/settings/ReconnectModal";
import { asShareTarget } from "../features/share/model";
import { setPendingShare } from "../features/share/state";

/** Longest Undo countdown a hand-off may ask for (Settings offers up to 30 s). */
const MAX_UNDO_SECONDS = 30;

function asRef(x: unknown): ThreadRef | null {
  if (!x || typeof x !== "object") return null;
  const r = x as Record<string, unknown>;
  return typeof r.accountId === "string" && typeof r.threadId === "string" ? { accountId: r.accountId, threadId: r.threadId } : null;
}

/** Listen for hand-offs. Main window only (App.tsx). */
export function installWindowHandoffs(): () => void {
  return onBus((m) => {
    if (m.type === "send") {
      const seed = asComposeSeed(m.seed);
      if (!seed) return;
      ackBus(m);
      const seconds = Math.min(MAX_UNDO_SECONDS, Math.max(0, Math.round(Number(m.seconds) || 0)));
      receiveSend(seed, seconds);
    } else if (m.type === "undo") {
      const u = asRemoteUndo(m.undo);
      if (!u) return;
      ackBus(m);
      offerUndo(u, asRef(m.reopen));
    } else if (m.type === "draft-kept") {
      const seed = asComposeSeed(m.seed);
      if (!seed) return;
      ackBus(m);
      keepDraft(seed);
    } else if (m.type === "open-settings") {
      // Asked from a window without Settings (a link in a message's privacy
      // note, an invitation, the composer's snippet or signature links).
      void focusMainWindow();
      // "Copy Share Link…" before share links were set up: the file waits here.
      if (m.share !== undefined) setPendingShare(asShareTarget(m.share));
      openSettings(asSettingsSection(m.section));
    } else if (m.type === "reconnect" && typeof m.accountId === "string") {
      void focusMainWindow();
      openReconnect(m.accountId);
    } else if (m.type === "open-search") {
      // "Mail from this person" in a conversation window's person card.
      void focusMainWindow();
      setUi({ overlay: "search", searchPrefill: typeof m.prefill === "string" ? m.prefill.slice(0, 500) : null });
    }
  });
}

/** "Archived · Undo" for a conversation window that closed; Undo reopens it. */
function offerUndo(u: RemoteUndo, reopen: ThreadRef | null) {
  let toastId = 0;
  const refs = u.steps.flat().flatMap((op) => op.refs);
  const run = () => {
    dismissToast(toastId);
    runUndoSteps(u.steps).then(
      () => reopen && void openThreadWindow(reopen),
      (e) => fail("undo", e, refs),
    );
  };
  setUndo(u.message, run);
  toastId = toast({
    kind: "action",
    key: `undo:window:${u.message}`,
    message: u.message,
    action: { label: "Undo", keys: "z", run: () => isPendingUndo(run) && undo() },
  });
}

let kept = 0;

/**
 * A draft from a closed window that couldn't be saved: this window keeps
 * saving it (every 10 s while it fails) until it's in Drafts.
 */
function keepDraft(seed: ComposeSeed) {
  const state = { ...seed.state, key: `kept:${++kept}` };
  const saver = saverFor(state);
  saver.markUnsaved();
  saver.update(state);
  void saver.flush();
  const toastId = toast({
    kind: "error",
    message: "Kept a draft from a closed window",
    detail: "It couldn't be saved to Drafts yet; Penguin keeps trying.",
    action: {
      label: "Open",
      run: () => {
        dismissToast(toastId);
        off();
        void openSaverInWindow(saver);
      },
    },
  });
  const off = saver.subscribe(() => {
    if (saver.status.kind !== "saved") return;
    off();
    dismissToast(toastId);
    toast({ kind: "success", message: "The kept draft is saved to Drafts" });
  });
}
