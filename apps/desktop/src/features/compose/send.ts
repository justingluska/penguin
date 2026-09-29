// Undo send: a message waits (5/10/20/30 s, from settings; 0 = off) in a countdown toast before it is
// handed to the backend. Z (the app's Undo) or the toast button cancels and
// reopens the draft. The Gmail draft stays saved during the countdown and the
// send goes through it (drafts.send), so it leaves the Drafts view.
//
// Windows (src-tauri/src/windows.rs). The countdown always runs in a window
// that stays open until it ends:
//   - A compose window closes the moment you send, so it hands the send to
//     the main window (the one window that never closes; ⌘W only hides it),
//     which comes to the front and counts down there. Its Undo reopens the
//     message in a compose window, as it was.
//   - A conversation window counts down itself (you may keep reading), and
//     if it closes before the countdown ends (⌘W, archiving the
//     conversation) it hands the rest of the countdown to the main window
//     the same way (handOffPendingSend).
// Handing over waits only for a save already under way, so the main window
// knows the draft id and never makes a second draft; if the main window
// doesn't take it (it isn't there), the countdown runs here after all.
import { api, asCommandError } from "../../lib/api";
import { setUi } from "../../lib/ui";
import { currentSettings } from "../../lib/settings";
import { dismissToast, toast, updateToast } from "../../components/Toast";
import { clearUndo, setUndo } from "../../app/actions";
import { busAsk, windowRoute } from "../../lib/windowBus";
import { focusMainWindow } from "../../lib/windowChrome";
import { MAIN_LABEL } from "../../lib/windowRoute";
import { drafts, toDraft, type EditorState } from "./draft";
import { DraftSaver, forgetSaver } from "./autosave";
import { remindLabel } from "./outbox";
import { composeTitle, remainingUndo, type ComposeSeed } from "./seed";

/** Where a counting-down message came from: this window's composer, or another window's. */
type Origin = "here" | "window";

interface Pending {
  state: EditorState;
  saver: DraftSaver;
  origin: Origin;
  /** The countdown's full length, and when it started (a hand-off passes on what's left). */
  seconds: number;
  startedAt: number;
  remaining: number;
  toastId: number;
  timer: ReturnType<typeof setInterval>;
}

let pending: Pending | null = null;
/** Sends being handed to the main window (a compose window closes after them). */
const handoffs = new Set<Promise<unknown>>();

function reopen(state: EditorState) {
  drafts.set(state.key, state);
  setUi({ overlay: "compose", composeContext: state.ctx });
}

/** The editor state with the draft as the saver last saved it. */
function seedOf(saver: DraftSaver): ComposeSeed {
  const s = saver.current();
  return { v: 1, state: { ...s, draftId: saver.draftId, draftAccountId: saver.state.draftAccountId }, unsaved: !saver.isSaved() };
}

/**
 * Give a message this window holds for another one back to a compose window
 * of its own, as it was: the Undo of a send that came from a compose window,
 * or a draft kept after its window closed (app/handoffs.ts). The draft is
 * saved first, so that window continues this draft instead of making a
 * second one.
 */
export async function openSaverInWindow(saver: DraftSaver) {
  await saver.flush();
  saver.stop();
  forgetSaver(saver);
  const seed = seedOf(saver);
  const { draftId, draftAccountId } = seed.state;
  try {
    await api.openWindow({
      kind: "compose",
      accountId: draftId ? draftAccountId ?? seed.state.accountId : null,
      draftId,
      seed,
      title: composeTitle(seed.state),
    });
  } catch (e) {
    // No window: the main composer has it instead.
    toast({ tone: "error", message: `Couldn't open a window for the message: ${asCommandError(e).message}` });
    reopen(seed.state);
  }
}

function settle(p: Pending) {
  if (pending === p) pending = null;
  clearInterval(p.timer);
  clearUndo(cancelSend);
  dismissToast(p.toastId);
  [...idle].forEach((f) => f());
}

/** A message is counting down to send (its Undo is still open). */
export function sendPending(): boolean {
  return pending !== null;
}

/** Cancel the pending send and reopen its draft. */
export function cancelSend() {
  const p = pending;
  if (!p) return;
  settle(p);
  if (p.origin === "window") void openSaverInWindow(p.saver);
  else reopen(p.state);
}

async function deliver(state: EditorState, saver: DraftSaver, origin: Origin) {
  try {
    // Let any in-flight save settle so we send through the latest draft id.
    await saver.flush();
    // Saved attachments go as refs to the draft, not as bytes again. Sending a
    // draft also drops any "send later" the backend had for it.
    const sent = await api.sendMessage(toDraft({ ...state, attachments: saver.current().attachments }), saver.draftId);
    saver.stop();
    forgetSaver(saver);
    // (A message from another window never had a slot in this one's drafts.)
    if (origin === "here") drafts.delete(state.key);
    if (!state.remindAfterMs) {
      toast({ kind: "success", message: "Sent" });
      return;
    }
    try {
      await api.setReminder({
        accountId: state.accountId,
        threadId: sent.threadId,
        sentMessageId: sent.messageId,
        remindAt: Date.now() + state.remindAfterMs,
      });
      toast({ kind: "success", message: "Sent", detail: `Reminding you in ${remindLabel(state.remindAfterMs)} if nobody replies` });
    } catch (e) {
      toast({ tone: "error", message: `Sent, but couldn't set the reminder: ${asCommandError(e).message}` });
    }
  } catch (e) {
    if (origin === "here") drafts.set(state.key, state);
    else void focusMainWindow();
    toast({
      tone: "error",
      message: `Couldn't send: ${asCommandError(e).message}`,
      action: { label: "Open draft", run: () => (origin === "here" ? reopen(state) : void openSaverInWindow(saver)) },
    });
  }
}

/** Send from this window's composer (⌘↵, Send). */
export function queueSend(state: EditorState, saver: DraftSaver) {
  const seconds = currentSettings().undoSendSeconds;
  if (windowRoute.kind === "compose") {
    // This window is about to close: the main window counts down (see top).
    saver.update(state);
    track(handOff(saver, seconds, (sv) => countDown(state, sv, seconds, "here")));
    return;
  }
  countDown(state, saver, seconds, "here");
}

/**
 * Hand a send to the main window. Stops saving here first (a save already
 * under way finishes, so the draft id goes along); `orElse` runs if the
 * main window doesn't take it.
 */
async function handOff(saver: DraftSaver, seconds: number, orElse: (saver: DraftSaver) => void) {
  saver.stop();
  await saver.flush();
  forgetSaver(saver);
  const seed = seedOf(saver);
  if (await busAsk(MAIN_LABEL, "send", { seed, seconds }, 3000)) return;
  // The main window didn't take it: count down here, with a saver that saves.
  const again = new DraftSaver(seed.state);
  if (seed.unsaved) again.markUnsaved();
  orElse(again);
}

function track(p: Promise<unknown>) {
  handoffs.add(p);
  void p.finally(() => handoffs.delete(p));
}

/** Resolves once every send this window handed over has been taken (a compose window closes after it). */
export function handoffsSettled(): Promise<void> {
  return Promise.allSettled([...handoffs]).then(() => undefined);
}

/**
 * This window is closing while a message counts down (a conversation
 * window): the main window takes over what's left of the countdown.
 */
export async function handOffPendingSend(): Promise<void> {
  const p = pending;
  if (!p) return;
  settle(p);
  const left = remainingUndo(p.seconds, Date.now() - p.startedAt);
  await handOff(p.saver, left, (sv) => countDown(p.state, sv, left, p.origin));
}

const idle = new Set<() => void>();

/** Resolves when no message is counting down here (at once if none is). */
export function sendSettled(): Promise<void> {
  if (!pending) return Promise.resolve();
  return new Promise((resolve) => {
    const f = () => {
      if (pending) return;
      idle.delete(f);
      resolve();
    };
    idle.add(f);
  });
}

/**
 * The main window: a send handed over by another window. It comes to the
 * front (the compose window just closed; this is where Undo is) and counts
 * down what's left.
 */
export function receiveSend(seed: ComposeSeed, seconds: number) {
  const saver = new DraftSaver(seed.state);
  if (seed.unsaved) saver.markUnsaved();
  if (seconds > 0) void focusMainWindow();
  countDown(seed.state, saver, seconds, "window");
}

function countDown(state: EditorState, saver: DraftSaver, seconds: number, origin: Origin) {
  // A second send while one is pending sends the first right away.
  if (pending) {
    const prev = pending;
    settle(prev);
    void deliver(prev.state, prev.saver, prev.origin);
  }
  if (origin === "here") drafts.delete(state.key);
  // The draft stays saved (and current) while the countdown runs.
  saver.update(state);
  void saver.flush();
  // Undo send off (Settings → Compose): it goes now, once the save settles.
  if (seconds <= 0) {
    void deliver(state, saver, origin);
    return;
  }
  const p: Pending = {
    state,
    saver,
    origin,
    seconds,
    startedAt: Date.now(),
    remaining: seconds,
    toastId: toast({
      kind: "progress",
      key: `send:${state.key}:${origin}`,
      message: `Sending in ${seconds}s`,
      progress: 1,
      action: { label: "Undo", keys: "z", run: cancelSend },
      // The interval below dismisses it; this is only a backstop.
      duration: (seconds + 2) * 1000,
    }),
    timer: setInterval(() => {
      p.remaining -= 1;
      if (p.remaining > 0) {
        updateToast(p.toastId, { message: `Sending in ${p.remaining}s`, progress: p.remaining / seconds });
        return;
      }
      settle(p);
      void deliver(p.state, p.saver, p.origin);
    }, 1000),
  };
  pending = p;
  setUndo("Undo send", cancelSend);
}
