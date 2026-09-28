// Undo send: a message waits (5/10/20/30 s, from settings; 0 = off) in a countdown toast before it is
// handed to the backend. Z (the app's Undo) or the toast button cancels and
// reopens the draft. The Gmail draft stays saved during the countdown and the
// send goes through it (drafts.send), so it leaves the Drafts view.
import { api, asCommandError } from "../../lib/api";
import { setUi } from "../../lib/ui";
import { currentSettings } from "../../lib/settings";
import { dismissToast, toast, updateToast } from "../../components/Toast";
import { clearUndo, setUndo } from "../../app/actions";
import { drafts, toDraft, type EditorState } from "./draft";
import { forgetSaver, type DraftSaver } from "./autosave";
import { remindLabel } from "./outbox";


interface Pending {
  state: EditorState;
  saver: DraftSaver;
  remaining: number;
  toastId: number;
  timer: ReturnType<typeof setInterval>;
}

let pending: Pending | null = null;

function reopen(state: EditorState) {
  drafts.set(state.key, state);
  setUi({ overlay: "compose", composeContext: state.ctx });
}

function settle(p: Pending) {
  if (pending === p) pending = null;
  clearInterval(p.timer);
  clearUndo(cancelSend);
  dismissToast(p.toastId);
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
  reopen(p.state);
}

async function deliver(state: EditorState, saver: DraftSaver) {
  try {
    // Let any in-flight save settle so we send through the latest draft id.
    await saver.flush();
    // Saved attachments go as refs to the draft, not as bytes again. Sending a
    // draft also drops any "send later" the backend had for it.
    const sent = await api.sendMessage(toDraft({ ...state, attachments: saver.current().attachments }), saver.draftId);
    saver.stop();
    forgetSaver(saver);
    drafts.delete(state.key);
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
    drafts.set(state.key, state);
    toast({
      tone: "error",
      message: `Couldn't send: ${asCommandError(e).message}`,
      action: { label: "Open draft", run: () => reopen(state) },
    });
  }
}

export function queueSend(state: EditorState, saver: DraftSaver) {
  // A second send while one is pending sends the first right away.
  if (pending) {
    const prev = pending;
    settle(prev);
    void deliver(prev.state, prev.saver);
  }
  drafts.delete(state.key);
  // The draft stays saved (and current) while the countdown runs.
  saver.update(state);
  void saver.flush();
  const seconds = currentSettings().undoSendSeconds;
  // Undo send off (Settings → Compose): it goes now, once the save settles.
  if (seconds <= 0) {
    void deliver(state, saver);
    return;
  }
  const p: Pending = {
    state,
    saver,
    remaining: seconds,
    toastId: toast({
      kind: "progress",
      key: `send:${state.key}`,
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
      void deliver(p.state, p.saver);
    }, 1000),
  };
  pending = p;
  setUndo("Undo send", cancelSend);
}
