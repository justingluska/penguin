// The shells of conversation and compose windows (src-tauri/src/windows.rs;
// lib/windowRoute.ts says which one this page is). Each window runs its own
// JS, so here is what a window of its own brings up, and what it leaves to
// the main window:
//
//   Here, per window: keys (a conversation window only its conversation's:
//   app/shortcuts.ts THREAD_WINDOW_KEYS), the menu bar context (app/menu.ts;
//   items that act on a window come to the focused one), text and context
//   menus, the thread cache, kept current by mail-changed like the main
//   window's, settings (lib/settings.ts follows settings-changed), the
//   pickers, previews and toasts a conversation or composer needs.
//
//   Main window only: sync and its toasts, the list and its counts,
//   notifications, the updater, the welcome flow, the shortcut coach,
//   profiles and Floe, rules toasts, scheduled-send and reminder toasts,
//   demo mode switching, and in the mock the mock backend itself
//   (lib/mockBridge.ts). A window that closes hands the main window what must
//   outlive it (app/handoffs.ts): a send still counting down, the Undo of the
//   change that closed it, a draft that couldn't be saved. Settings, signing
//   in again and search are asked of the main window, which comes forward.
//
// A conversation window closes when its conversation leaves it: archived,
// trashed, snoozed, moved, marked unread (as in the main window, that
// leaves the conversation), or Esc / ⌘W. Opening another conversation from
// inside it (a person card, a link) opens that one in its own window.
// A compose window closes when its composer does (sent, closed, discarded,
// scheduled).
import { useEffect, useRef, useState } from "react";
import { api, asCommandError, callsSettled, onActionFailed, onBodyFetchFailed, onMailChanged } from "../lib/api";
import type { ThreadRef, ThreadView } from "../lib/types";
import { getUi, sameThread, setUi, subscribeUi, useUi } from "../lib/ui";
import { installKeyboard } from "../lib/keyboard";
import { getSettings } from "../lib/settings";
import { busAsk, busSend, windowRoute } from "../lib/windowBus";
import { MAIN_LABEL, type WindowRoute } from "../lib/windowRoute";
import { setThisWindowTitle } from "../lib/windowChrome";
import { IconSprite, Icon } from "../components/Icon";
import { ToastHost, toast } from "../components/Toast";
import { ContextMenuHost } from "../components/ContextMenu";
import { ModalHost } from "../components/Modal";
import { ConfirmHost } from "./confirm";
import { installContextMenus } from "./textMenu";
import { CommandPalette } from "../features/command";
import { Compose, openDraftById } from "../features/compose";
import { LabelPicker } from "../features/inbox/LabelPicker";
import { MovePicker } from "../features/inbox/MovePicker";
import { SnoozePicker } from "../features/snooze/SnoozePicker";
import { ThreadPane } from "../features/thread/ThreadView";
import { AttachmentPreviewHost } from "../features/thread/AttachmentPreview";
import { ImageViewerHost } from "../features/image-viewer/ImageViewer";
import { MessageDetailsHost } from "../features/thread/MessageDetails";
import { PersonCardHost } from "../features/people/PersonCard";
import { EventPopoverHost } from "../features/calendar/EventPopover";
import { ShortcutSheet } from "./ShortcutSheet";
import { registerOtpShortcuts } from "../features/otp/otp";
import { contextKey, drafts } from "../features/compose/draft";
import { flushAllSavers, saverFor, unsavedDrafts } from "../features/compose/autosave";
import { handOffPendingSend, handoffsSettled, sendSettled } from "../features/compose/send";
import { asComposeSeed, type ComposeSeed } from "../features/compose/seed";
import { registerAppShortcuts } from "./shortcuts";
import { closeThisWindow, CLOSE_REQUEST, installMenu } from "./menu";
import { fetchThread, invalidateThreads, loadLabels, setAccounts } from "./store";
import { currentRemoteUndo, undoSerial } from "./actions";
import { openThreadWindow } from "./windows";
import { coalesce } from "./coalesce";
import { useTheme } from "./theme";
import type { RemoteUndo } from "./remoteUndo";

/** Commands whose change a closing window waits to see accepted (or refused). */
const CHANGES = ["modify_threads", "snooze_threads", "unsnooze_threads", "reply_later", "dismiss_follow_ups"] as const;

// ---------------------------------------------------------------------------
// Closing
// ---------------------------------------------------------------------------

/** The Undo of the change that is closing this window, for the main window. */
let undoOffer: { undo: RemoteUndo; reopen: ThreadRef | null } | null = null;
let finishing: Promise<boolean> | null = null;

/**
 * Everything a window does before it goes. Resolves false when it must stay
 * after all (a compose window whose send the main window didn't take, and
 * whose Undo brought the message back).
 */
function finish(): Promise<boolean> {
  finishing ??= (async () => {
    try {
      // A message counting down here goes on counting in the main window.
      await handOffPendingSend();
      await handoffsSettled();
      await sendSettled();
      if (windowRoute.kind === "compose" && getUi().overlay === "compose") return false;
      // Drafts open here are saved; one that can't be (offline) is kept by the main window.
      await flushAllSavers();
      for (const state of unsavedDrafts()) {
        await busAsk(MAIN_LABEL, "draft-kept", { seed: { v: 1, state, unsaved: true } }, 3000);
      }
      // The last change reaches the backend before this window's JS goes.
      await callsSettled(CHANGES, 2000);
      if (undoOffer) await busAsk(MAIN_LABEL, "undo", { undo: undoOffer.undo, reopen: undoOffer.reopen }, 2000);
      return true;
    } catch (e) {
      console.warn("penguin: closing the window", e);
      return true;
    } finally {
      finishing = null;
    }
  })();
  return finishing;
}

/** Run finish() on every way a window closes: its red button, ⌘W, Esc, and closeThisWindow(). */
function useCloseHandler() {
  useEffect(() => {
    if (!("__TAURI_INTERNALS__" in window)) {
      // A browser has no close request to hold: finish, then close.
      const onRequest = () => void finish().then((go) => go && window.close());
      window.addEventListener(CLOSE_REQUEST, onRequest);
      return () => window.removeEventListener(CLOSE_REQUEST, onRequest);
    }
    let off: (() => void) | null = null;
    let dead = false;
    void import("@tauri-apps/api/window").then(({ getCurrentWindow }) =>
      getCurrentWindow()
        .onCloseRequested(async (e) => {
          // The window closes (destroy) once this resolves, unless prevented.
          if (!(await finish())) e.preventDefault();
        })
        .then((f) => (dead ? f() : (off = f))),
    );
    return () => {
      dead = true;
      off?.();
    };
  }, []);
}

/** Keys, menus and the pointer habit every window shares with the main one. */
function useWindowBasics(scope: "thread" | "compose") {
  useEffect(() => {
    const offs = [
      installKeyboard(),
      scope === "thread" ? registerAppShortcuts("thread") : () => {},
      scope === "thread" ? registerOtpShortcuts() : () => {},
      installMenu(),
      installContextMenus(),
    ];
    // Keyboard-first: a clicked button shouldn't keep focus (App.tsx does the same).
    const onPointerUp = () => {
      const el = document.activeElement;
      if (el instanceof HTMLButtonElement) el.blur();
    };
    window.addEventListener("pointerup", onPointerUp);
    // Search lives in the main window: "Mail from this person" (the person
    // card, the context panel) asks it there.
    offs.push(
      subscribeUi(() => {
        const ui = getUi();
        if (ui.overlay !== "search") return;
        setUi({ overlay: null, searchPrefill: null });
        void busSend(MAIN_LABEL, "open-search", { prefill: ui.searchPrefill });
      }),
    );
    return () => {
      offs.forEach((f) => f());
      window.removeEventListener("pointerup", onPointerUp);
    };
  }, [scope]);
}

/** Backend events a window's own caches follow (the list and counts are the main window's). */
function useWindowEvents(active: boolean) {
  useEffect(() => {
    if (!active) return;
    const labels = coalesce(loadLabels, { delay: 400, minGap: 1000 });
    const offs = [
      onMailChanged((e) => {
        invalidateThreads(e.accountId, e.threadIds);
        labels.request();
      }),
      onActionFailed((e) => toast({ kind: "error", message: "An action failed and was undone", detail: e.message || undefined, title: e.message || undefined })),
      onBodyFetchFailed((e) =>
        toast({
          kind: "error",
          key: `body-fetch:${e.accountId}`,
          message: "Couldn't download this message from Gmail. Reopen it to try again.",
          detail: e.message || undefined,
          title: e.message || undefined,
        }),
      ),
    ];
    return () => {
      labels.dispose();
      offs.forEach((p) => void p.then((off) => off()));
    };
  }, [active]);
}

/** Accounts and settings before anything renders (local reads, no network). */
async function loadBasics() {
  const [accounts] = await Promise.all([api.listAccounts(), getSettings()]);
  setAccounts(accounts);
  void loadLabels();
}

/** The hosts both kinds of window mount (overlays show themselves from ui.overlay). */
function CommonHosts() {
  return (
    <>
      <ShortcutSheet />
      <AttachmentPreviewHost />
      <ImageViewerHost />
      <MessageDetailsHost />
      <PersonCardHost />
      <EventPopoverHost />
      <ToastHost />
      <ModalHost />
      <ConfirmHost />
      <ContextMenuHost />
    </>
  );
}

// ---------------------------------------------------------------------------
// A conversation
// ---------------------------------------------------------------------------

/**
 * Which view a conversation's actions behave as: where it is now. E archives
 * (and the window closes) when it's in the inbox; # restores it from Trash.
 */
function viewFor(t: ThreadView): { kind: "inbox" } | { kind: "trash" } | { kind: "spam" } | { kind: "all" } {
  if (t.labelIds.includes("TRASH")) return { kind: "trash" };
  if (t.labelIds.includes("SPAM")) return { kind: "spam" };
  if (t.labelIds.includes("INBOX")) return { kind: "inbox" };
  return { kind: "all" };
}

export function ThreadWindowApp({ thread }: { thread: ThreadRef }) {
  const [phase, setPhase] = useState<"loading" | "ready" | "error">("loading");
  const [error, setError] = useState<string | null>(null);
  useTheme();
  useWindowBasics("thread");
  useWindowEvents(phase === "ready");
  useCloseHandler();

  useEffect(() => {
    let alive = true;
    loadBasics()
      .then(async () => {
        if (!alive) return;
        // Opening it here is opening it: it's marked read like a click in the list.
        setUi({ selected: thread, threadOpen: true, focusMessageId: null, selectedByApp: false, overlay: null, view: { kind: "inbox" } });
        setPhase("ready");
        const t = await fetchThread(thread);
        if (!alive) return;
        if (t) {
          setUi({ view: viewFor(t) });
          setThisWindowTitle(t.subject || "(no subject)");
        }
      })
      .catch((e) => {
        if (!alive) return;
        setError(asCommandError(e).message);
        setPhase("error");
      });
    return () => {
      alive = false;
    };
  }, [thread]);

  useLeaveWhenGone(thread, phase === "ready");

  return (
    <>
      <IconSprite />
      <div className="titlebar" data-tauri-drag-region aria-hidden="true" />
      {phase === "error" ? (
        <WindowProblem message={`Couldn't open this conversation: ${error}`} />
      ) : phase === "ready" ? (
        <div className="app win-app win-thread">
          <main className="main">
            <div className="main-body">
              <ThreadPane variant="full" />
            </div>
          </main>
        </div>
      ) : (
        <div className="app app-loading" />
      )}
      <CommandPalette />
      <Compose />
      <LabelPicker />
      <MovePicker />
      <SnoozePicker />
      <CommonHosts />
    </>
  );
}

/**
 * Close the window when its conversation leaves it, and send any other
 * conversation it's asked to show to a window of its own.
 */
function useLeaveWhenGone(thread: ThreadRef, active: boolean) {
  const leaving = useRef(false);
  useEffect(() => {
    if (!active) return;
    const check = () => {
      const ui = getUi();
      if (leaving.current) return;
      if (ui.selected && !sameThread(ui.selected, thread)) {
        const other = ui.selected;
        setUi({ selected: thread, threadOpen: true, focusMessageId: null, selectedByApp: false });
        void openThreadWindow(other);
        return;
      }
      if (ui.threadOpen && ui.selected) return;
      // Gone: removed (archive, trash, snooze, move: `selected` cleared), or
      // left (Esc, marked unread: `selected` stays). The action sets its Undo
      // right after the list change that got us here; count undos from now.
      leaving.current = true;
      const removed = ui.selected === null;
      const before = undoSerial();
      void (async () => {
        await new Promise((r) => setTimeout(r, 0));
        const undo = undoSerial() !== before ? currentRemoteUndo() : null;
        if (removed) {
          await callsSettled(CHANGES, 2000);
          // Refused (offline, signed out): the change was put back; stay, with its error toast.
          if (sameThread(getUi().selected, thread)) {
            leaving.current = false;
            setUi({ threadOpen: true });
            return;
          }
        }
        undoOffer = removed && undo ? { undo, reopen: thread } : null;
        closeThisWindow();
      })();
    };
    return subscribeUi(check);
  }, [thread, active]);
}

function WindowProblem({ message }: { message: string }) {
  return (
    <div className="win-problem">
      <Icon name="info" size="sm" />
      <p>{message}</p>
      <button className="btn btn-secondary btn-sm" onClick={closeThisWindow}>
        Close window
      </button>
    </div>
  );
}

// ---------------------------------------------------------------------------
// A message being written
// ---------------------------------------------------------------------------

let seedTaken: Promise<ComposeSeed | null> | null = null;

/** The seed handed to this window (taken from the backend once, however often boot runs). */
function takeSeed(): Promise<ComposeSeed | null> {
  seedTaken ??= api.takeWindowSeed().then(asComposeSeed, () => null);
  return seedTaken;
}

export function ComposeWindowApp({ route }: { route: Extract<WindowRoute, { kind: "compose" }> }) {
  const [problem, setProblem] = useState<string | null>(null);
  useTheme();
  useWindowBasics("compose");
  useCloseHandler();
  const overlay = useUi((s) => s.overlay);

  useEffect(() => {
    let alive = true;
    (async () => {
      await loadBasics();
      if (!alive) return;
      // A new message is from the account the main window would pick.
      if (route.fromAccount) setUi({ accountFilter: route.fromAccount });
      const seed = await takeSeed();
      if (!alive) return;
      if (seed) {
        // Moved here from another window: exactly as it was there. A new
        // message already saved is that draft from now on (like one reopened
        // from Drafts), so it keys by its draft id: the "new" slot starts
        // over whenever a composer holding a saved draft closes.
        const s = seed.state;
        const ctx = s.ctx.mode === "new" && s.draftId && !s.ctx.draftId ? { ...s.ctx, draftId: s.draftId } : s.ctx;
        const key = contextKey(ctx);
        const state = { ...s, ctx, key };
        drafts.set(key, state);
        const saver = saverFor(state);
        if (seed.unsaved) saver.markUnsaved();
        setUi({ overlay: "compose", composeContext: state.ctx });
      } else if (route.draft) {
        await openDraftById(route.draft.accountId, route.draft.draftId);
        if (alive && getUi().overlay !== "compose") setProblem("Couldn't open that draft. It may have been sent or deleted.");
      } else {
        setUi({ overlay: "compose", composeContext: { mode: "new" } });
      }
    })().catch((e) => alive && setProblem(`Couldn't open the message: ${asCommandError(e).message}`));
    return () => {
      alive = false;
    };
  }, [route]);

  // The composer closed (sent, closed with Esc or ⌘W, discarded, scheduled): so does the window.
  const opened = useRef(false);
  useEffect(() => {
    if (overlay === "compose") opened.current = true;
    else if (opened.current) {
      opened.current = false;
      closeThisWindow();
    }
  }, [overlay]);

  return (
    <>
      <IconSprite />
      <div className="titlebar" data-tauri-drag-region aria-hidden="true" />
      {problem ? <WindowProblem message={problem} /> : overlay !== "compose" && <div className="app app-loading" />}
      <Compose />
      <CommonHosts />
    </>
  );
}
