// Handoff with the iPhone, the UI half (Apple's Handoff; app/handoffs.ts is
// something else: what a closing window hands the main window). Every
// window tells the backend what it offers: the conversation in the reading
// pane, the full view or a conversation window, or an open composer's saved
// Gmail draft (which covers the conversation behind it), or nothing while
// the window is hidden or minimized. The backend advertises the front
// window's (src-tauri/src/handoff.rs). The main window also opens what the
// iPhone hands over. Rules: ./rules.ts.
import { useEffect, useRef } from "react";
import { api, asCommandError, onHandoffOpen } from "../../lib/api";
import type { ThreadView } from "../../lib/types";
import { isMainWindow } from "../../lib/thisWindow";
import { openThread, setUi } from "../../lib/ui";
import { meta } from "../../app/store";
import { toast } from "../../components/Toast";
import { createPublisher, handoffActivityOf, openHandedOver, ShownSlot, type ShownDraft, type ShownThread } from "./rules";

const shownThread = new ShownSlot<ShownThread>();
const shownDraft = new ShownSlot<ShownDraft>();

/** Report the conversation a pane shows (null while it shows none). */
export function useHandoffThread(thread: ThreadView | null) {
  const owner = useRef({}).current;
  const accountId = thread?.accountId ?? null;
  const threadId = thread?.threadId ?? null;
  const subject = thread?.subject ?? "";
  useEffect(() => {
    shownThread.set(owner, accountId && threadId ? { accountId, threadId, subject } : null);
  }, [owner, accountId, threadId, subject]);
  useEffect(() => () => shownThread.set(owner, null), [owner]);
}

/** Report an open composer and its Gmail draft (draftId null until the first save lands). */
export function useHandoffDraft(draft: ShownDraft | null) {
  const owner = useRef({}).current;
  const { accountId, draftId, threadId, subject } = draft ?? { accountId: null, draftId: null, threadId: null, subject: "" };
  useEffect(() => {
    shownDraft.set(owner, accountId ? { accountId, draftId, threadId, subject } : null);
  }, [owner, accountId, draftId, threadId, subject]);
  useEffect(() => () => shownDraft.set(owner, null), [owner]);
}

/** Open what the iPhone handed over (main window). */
async function takeAndOpen() {
  const open = await api.takeHandoffOpen().catch(() => null);
  if (!open) return;
  await openHandedOver(open, {
    accountIds: meta.get().accounts.map((a) => a.id),
    getThread: (ref) => api.getThread(ref.accountId, ref.threadId),
    getDraft: (accountId, draftId) => api.getDraft(accountId, { draftId }),
    openThread: (ref) => {
      // As a clicked notification does (features/notifications).
      setUi({ surface: "mail" });
      openThread(ref);
    },
    openDraftWindow: async (accountId, draftId, title) => {
      await api.openWindow({ kind: "compose", accountId, draftId, title });
    },
    syncNow: () => void api.syncNow().catch(() => {}),
    toast: (t) => toast({ message: t.message, detail: t.detail, tone: t.error ? "error" : "default" }),
  }).catch((e) => toast({ tone: "error", message: asCommandError(e).message }));
}

let started = false;

/** Once per window, after its accounts are loaded. */
export function startHandoff() {
  if (started) return;
  started = true;
  const publish = createPublisher((activity) => api.setHandoffActivity(activity));
  let timer: ReturnType<typeof setTimeout> | null = null;
  const report = () => {
    timer = null;
    publish(
      handoffActivityOf({
        visible: document.visibilityState !== "hidden",
        draft: shownDraft.get(),
        thread: shownThread.get(),
        accounts: meta.get().accounts,
      }),
    );
  };
  // Coalesce bursts (arrowing through the list, typing a subject) into one call.
  const schedule = () => {
    if (!timer) timer = setTimeout(report, 150);
  };
  shownThread.subscribe(schedule);
  shownDraft.subscribe(schedule);
  meta.subscribe(schedule);
  document.addEventListener("visibilitychange", schedule);
  schedule();
  if (isMainWindow) {
    void onHandoffOpen(() => void takeAndOpen());
    // Penguin launched by Handoff: the activity came before this page listened.
    void takeAndOpen();
  }
}
