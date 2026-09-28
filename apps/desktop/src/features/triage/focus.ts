// "Reply to all" in Reply Later (HEY's Focus & Reply): walk the list one
// conversation at a time with the composer open. Closing the composer (sent,
// saved or discarded) moves on to the next one; a sticky toast shows where
// you are, with Skip and Stop. Leaving the Reply Later view stops it.
import type { ThreadRef } from "../../lib/types";
import { getUi, openThread, setUi, subscribeUi } from "../../lib/ui";
import { dismissToast, toast } from "../../components/Toast";
import { openCompose } from "../../app/actions";
import { list } from "../../app/store";

interface Session {
  refs: { ref: ThreadRef; subject: string }[];
  i: number;
  unsub: () => void;
  toastId: number;
}

let session: Session | null = null;

export function replyToAllActive(): boolean {
  return session !== null;
}

/** Start with the conversations the Reply Later list shows, top to bottom. */
export function startReplyToAll() {
  const items = list.get().items;
  if (getUi().view.kind !== "replyLater" || items.length === 0) return;
  stopReplyToAll();
  let composing = false;
  const unsub = subscribeUi(() => {
    const ui = getUi();
    if (ui.view.kind !== "replyLater") return stopReplyToAll();
    if (ui.overlay === "compose") {
      composing = true;
      return;
    }
    if (composing) {
      composing = false;
      // After this update settles (the composer's own close handling first).
      queueMicrotask(next);
    }
  });
  session = {
    refs: items.map((t) => ({ ref: { accountId: t.accountId, threadId: t.threadId }, subject: t.subject || "(no subject)" })),
    i: 0,
    unsub,
    toastId: 0,
  };
  show();
}

function show() {
  const s = session;
  if (!s) return;
  const cur = s.refs[s.i];
  openThread(cur.ref);
  openCompose("reply");
  s.toastId = toast({
    kind: "action",
    key: "reply-to-all",
    message: `Reply to all · ${s.i + 1} of ${s.refs.length}`,
    detail: cur.subject,
    duration: null,
    actions: [
      { label: "Skip", run: skip, keep: true },
      { label: "Stop", run: stopReplyToAll },
    ],
  });
}

function next() {
  const s = session;
  if (!s) return;
  s.i += 1;
  if (s.i < s.refs.length) return show();
  const n = s.refs.length;
  stopReplyToAll();
  toast({ kind: "success", message: `Went through all ${n} in Reply Later` });
}

/** Skip this one: close the composer (its draft is kept) or, if it's closed, move on. */
function skip() {
  if (getUi().overlay === "compose") setUi({ overlay: null });
  else next();
}

export function stopReplyToAll() {
  const s = session;
  if (!s) return;
  session = null;
  s.unsub();
  dismissToast(s.toastId);
}
