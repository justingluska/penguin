// App shell: onboarding until an OAuth client and an account exist, then the
// three-pane mail UI (sidebar | list | reading pane) or, with a thread open,
// sidebar | thread | context panel. Floe mode is a state of the same shell
// (features/floe). Overlays mount once and show themselves from ui.overlay.
import { useCallback, useEffect, useRef, useState, type CSSProperties } from "react";
import { api, asCommandError, isMock, onActionFailed, onBodyFetchFailed, onMailChanged, onSyncStatus } from "./lib/api";
import { getUi, openThread, setUi, useUi } from "./lib/ui";
import { installKeyboard } from "./lib/keyboard";
import { getSettings } from "./lib/settings";
import { LIST_W, READING_MIN_W, SIDEBAR_W, getLayout, setLayout, useLayout } from "./lib/layout";
import { IconSprite } from "./components/Icon";
import { ToastHost, toast } from "./components/Toast";
import { ContextMenuHost } from "./components/ContextMenu";
import { installContextMenus } from "./app/textMenu";
import { ConfirmHost } from "./app/confirm";
import { ModalHost } from "./components/Modal";
import { Splitter } from "./components/Splitter";
import { SearchOverlay } from "./features/search";
import { CommandPalette } from "./features/command";
import { Compose, preloadComposeEditor } from "./features/compose";
import { Sidebar } from "./features/sidebar/Sidebar";
import { ThreadList } from "./features/inbox/ThreadList";
import { LabelPicker } from "./features/inbox/LabelPicker";
import { MovePicker } from "./features/inbox/MovePicker";
import { SnoozePicker } from "./features/snooze/SnoozePicker";
import { ThreadPane } from "./features/thread/ThreadView";
import { AttachmentPreviewHost } from "./features/thread/AttachmentPreview";
import { openAttachment } from "./features/thread/openAttachment";
import { ImageViewerHost } from "./features/image-viewer/ImageViewer";
import { MessageDetailsHost, openMessageDetails } from "./features/thread/MessageDetails";
import { PersonCardHost, openPersonCard } from "./features/people/PersonCard";
import { ShortcutSheet } from "./app/ShortcutSheet";
import { ShortcutCoach } from "./app/ShortcutCoach";
import { SettingsModal } from "./features/settings/SettingsModal";
import { LogViewerHost } from "./features/logs/LogViewer";
import { ReconnectModal } from "./features/settings/ReconnectModal";
import { AddAccountModal } from "./features/onboarding/AddAccountModal";
import { WelcomeHost } from "./features/welcome";
import { registerSettingsShortcuts } from "./features/settings/state";
import { installRuleToasts, registerRuleShortcuts } from "./features/rules/state";
import { StatusBar } from "./app/StatusBar";
import { registerAppShortcuts } from "./app/shortcuts";
import { installSplits } from "./features/split/state";
import { GetToZeroHost } from "./features/zero/GetToZero";
import { registerOtpShortcuts } from "./features/otp/otp";
import { startSyncToasts } from "./app/syncToasts";
import { startUpdateToasts } from "./app/updates";
import { startNotifications } from "./features/notifications";
import { installProfiles } from "./app/profiles";
import { installFloe, useFloe } from "./features/floe/state";
import { useFloeMorph } from "./features/floe/morph";
import { installMenu } from "./app/menu";
import { needsSetup } from "./app/setupGate";
import { EventPopoverHost } from "./features/calendar/EventPopover";
import {
  applySyncStatus,
  fetchThread,
  invalidateThreads,
  list,
  loadLabels,
  loadSplitCounts,
  refreshList,
  setAccounts,
  startMail,
} from "./app/store";
import { lazyScreen, prefetchScreens } from "./lib/lazy";
import { coalesce } from "./app/coalesce";
import "./App.css";

// Rarely shown, so not in the launch bundle (lib/lazy.ts): first-run setup
// and the calendar. Settings and Add account load the same way from their hosts.
const Onboarding = lazyScreen(() => import("./features/onboarding").then((m) => m.Onboarding), { prefetch: false });
const CalendarView = lazyScreen(() => import("./features/calendar/CalendarView").then((m) => m.CalendarView));

type Phase = "loading" | "onboarding" | "ready";

export default function App() {
  const [phase, setPhase] = useState<Phase>("loading");
  useTheme();

  const boot = useCallback(async () => {
    try {
      // Settings first too: the active profile's accounts scope the first list query.
      const [status, accounts] = await Promise.all([api.oauthClientStatus(), api.listAccounts(), getSettings()]);
      // No accounts, or Gmail accounts without their Google client. An
      // IMAP/Microsoft-only user never needs a Google client.
      if (needsSetup(status, accounts)) {
        setPhase("onboarding");
        return;
      }
      setAccounts(accounts);
      startMail();
      startNotifications();
      setPhase("ready");
      // Once the first rows are on screen, fetch the rarely used screens and
      // the compose editor in idle moments, so none of them waits on first use.
      afterFirstRows(() => prefetchScreens([preloadComposeEditor]));
      // Deep link for screenshots, dev or mock builds only: ?open=<accountId>/<threadId>[&att=<n>].
      // A mock app bundle has no URL bar, so VITE_DEMO_QUERY can bake the same query in at build time.
      const demo = import.meta.env.DEV || isMock ? (import.meta.env.VITE_DEMO_QUERY as string | undefined) ?? location.search : "";
      const params = new URLSearchParams(demo);
      const open = params.get("open");
      if (open?.includes("/")) {
        const [accountId, threadId] = open.split("/");
        openThread({ accountId, threadId });
        // ...&att=<n>: also preview the n-th attachment of its latest message that has any.
        const att = Number(params.get("att"));
        if (params.has("att") && Number.isInteger(att) && att >= 0) {
          const t = await fetchThread({ accountId, threadId });
          const m = t && [...t.messages].reverse().find((x) => x.attachments.some((a) => !a.inline));
          const a = m?.attachments.filter((x) => !x.inline)[att];
          if (m && a) openAttachment(m, a);
        }
        // ...&person=1: the latest sender's person card, anchored to their name (&person=full: the modal).
        if (params.has("person")) {
          const t = await fetchThread({ accountId, threadId });
          const m = t?.messages[t.messages.length - 1];
          if (m) {
            const full = params.get("person") === "full";
            setTimeout(() => openPersonCard(m.from, full ? null : document.querySelector<HTMLElement>(".message .m-head .person-link")), 300);
          }
        }
        // ...&details=1: Message details for the thread's latest message (&source=1: its original).
        if (params.has("details")) {
          const t = await fetchThread({ accountId, threadId });
          const m = t?.messages[t.messages.length - 1];
          if (m) openMessageDetails(m, params.has("source"));
        }
      }
    } catch (e) {
      toast({ tone: "error", message: `Couldn't start: ${asCommandError(e).message}` });
      setPhase("onboarding");
    }
  }, []);

  useEffect(() => {
    void boot();
  }, [boot]);

  useEffect(() => {
    const offKeys = installKeyboard();
    const offShortcuts = registerAppShortcuts();
    const offSettings = registerSettingsShortcuts();
    const offProfiles = installProfiles();
    const offFloe = installFloe();
    // Split Inbox keys and counts; lives here, not in MailShell, so a shell
    // remount (e.g. after onboarding) keeps the chosen tab.
    const offSplits = installSplits();
    const offMenu = installMenu();
    const offMenus = installContextMenus();
    const offRules = registerRuleShortcuts();
    const offRuleToasts = installRuleToasts();
    const offOtp = registerOtpShortcuts();
    // Keyboard-first: a clicked button shouldn't keep focus, or the next
    // Enter/Space would press it again instead of reaching the shortcuts.
    const onPointerUp = () => {
      const el = document.activeElement;
      if (el instanceof HTMLButtonElement) el.blur();
    };
    window.addEventListener("pointerup", onPointerUp);
    return () => {
      offKeys();
      offOtp();
      offShortcuts();
      offSettings();
      offProfiles();
      offFloe();
      offSplits();
      offMenu();
      offMenus();
      offRules();
      offRuleToasts();
      window.removeEventListener("pointerup", onPointerUp);
    };
  }, []);

  useBackendEvents(phase === "ready");

  return (
    <>
      <IconSprite />
      {/* Window drag strip under the macOS traffic lights (desktop only, see app.css). */}
      <div className="titlebar" data-tauri-drag-region aria-hidden="true" />
      {phase === "onboarding" ? (
        <Onboarding onDone={() => void boot()} />
      ) : phase === "ready" ? (
        <MailShell />
      ) : (
        <div className="app app-loading" />
      )}
      <SearchOverlay />
      <CommandPalette />
      <Compose />
      <ShortcutSheet />
      <ShortcutCoach />
      <GetToZeroHost />
      <LabelPicker />
      <MovePicker />
      <SnoozePicker />
      <AttachmentPreviewHost />
      <ImageViewerHost />
      <MessageDetailsHost />
      <PersonCardHost />
      <EventPopoverHost />
      {phase === "ready" && <SettingsModal />}
      {/* Welcome setup: once on a new install, after the first account (features/welcome). */}
      {phase === "ready" && <WelcomeHost />}
      <LogViewerHost />
      <AddAccountModal />
      <ReconnectModal />
      <ToastHost />
      <ModalHost />
      <ConfirmHost />
      <ContextMenuHost />
    </>
  );
}

/** Run `f` after the first page of the mail list has loaded and painted. */
function afterFirstRows(f: () => void) {
  const go = () => requestAnimationFrame(() => setTimeout(f, 0));
  if (list.get().loaded) return go();
  const off = list.subscribe(() => {
    if (!list.get().loaded) return;
    off();
    go();
  });
}

function MailShell() {
  // Floe mode (features/floe): one surface at a time, no sidebar or panes.
  const floe = useFloe();
  const surface = useUi((s) => s.surface);
  // Floe has no calendar surface: stay on mail there.
  useEffect(() => {
    if (floe && surface === "calendar") setUi({ surface: "mail" });
  }, [floe, surface]);
  return <PaneShell />;
}

/**
 * The one mail shell for both layouts. Floe is a state of it, not another
 * shell: the list stays mounted while the panes slide away (features/floe/morph.ts).
 */
function PaneShell() {
  const appRef = useRef<HTMLDivElement>(null);
  const floe = useFloeMorph(appRef);
  const threadOpen = useUi((s) => s.threadOpen);
  const calendar = useUi((s) => s.surface === "calendar");
  const sidebarW = useLayout((s) => s.sidebarW);
  const listW = useLayout((s) => s.listW);
  const collapsed = useLayout((s) => s.sidebarCollapsed);
  const shownSidebarW = () => (getLayout().sidebarCollapsed ? 0 : getLayout().sidebarW);
  return (
    <div
      ref={appRef}
      className={"app" + (floe.floe ? " is-floe" : "") + (floe.morphing ? " is-morphing" : "")}
      style={{ "--sb-w": `${sidebarW}px`, "--list-w": `${listW}px` } as CSSProperties}
    >
      {floe.panes && !collapsed && (
        <>
          <Sidebar />
          <Splitter
            cssVar="--sb-w"
            label="Resize sidebar"
            value={sidebarW}
            min={SIDEBAR_W.min}
            max={() =>
              Math.min(SIDEBAR_W.max, window.innerWidth - (getUi().threadOpen ? 0 : getLayout().listW) - READING_MIN_W)
            }
            onCommit={(w) => setLayout({ sidebarW: w })}
            onReset={() => setLayout({ sidebarW: SIDEBAR_W.def })}
          />
        </>
      )}
      <main className="main">
        <div className="main-body">
          {threadOpen ? (
            <ThreadPane variant="full" />
          ) : calendar ? (
            <CalendarView />
          ) : (
            <>
              <ThreadList floe={floe.floeRows} />
              {floe.panes && (
                <>
                  <Splitter
                    cssVar="--list-w"
                    label="Resize message list"
                    value={listW}
                    min={LIST_W.min}
                    max={() => Math.min(LIST_W.max, window.innerWidth - shownSidebarW() - READING_MIN_W)}
                    onCommit={(w) => setLayout({ listW: w })}
                    onReset={() => setLayout({ listW: LIST_W.def })}
                  />
                  <ThreadPane variant="preview" />
                </>
              )}
            </>
          )}
        </div>
        {floe.panes && <StatusBar />}
      </main>
    </div>
  );
}

/** Theme: follow the system until the user presses T; mirror onto <html>. */
function useTheme() {
  const theme = useUi((s) => s.theme);
  useEffect(() => {
    document.documentElement.dataset.theme = theme;
  }, [theme]);
  useEffect(() => {
    const mq = window.matchMedia?.("(prefers-color-scheme: dark)");
    if (!mq) return;
    const onChange = () => {
      if (getUi().themeSource === "system") setUi({ theme: mq.matches ? "dark" : "light" });
    };
    mq.addEventListener("change", onChange);
    return () => mq.removeEventListener("change", onChange);
  }, []);
}

function useBackendEvents(active: boolean) {
  useEffect(() => {
    if (!active) return;
    const offs: Promise<() => void>[] = [];
    // Bursts of mail-changed (a backfill emits one per batch) become few
    // re-reads: at most one in flight, a few a second at most (app/coalesce.ts).
    const lists = coalesce(refreshList, { delay: 60, minGap: 250 });
    // Counts settle a little later; during a long storm they still update
    // about once a second instead of waiting for it to end.
    const counts = coalesce(
      async () => {
        await Promise.all([loadLabels(), getUi().view.kind === "inbox" ? loadSplitCounts() : null]);
      },
      { delay: 400, minGap: 1000 },
    );

    offs.push(onSyncStatus(applySyncStatus));
    // Broken syncs announce themselves once, with their fix button.
    const offSyncToasts = startSyncToasts();
    // Penguin → Check for Updates…, and "ready, restart" from the background check.
    const offUpdateToasts = startUpdateToasts();
    offs.push(
      onMailChanged((e) => {
        invalidateThreads(e.accountId, e.threadIds);
        lists.request();
        counts.request();
      }),
    );
    // A rolled-back optimistic action; its mail-changed arrives alongside.
    offs.push(
      onActionFailed((e) => {
        toast({ kind: "error", message: "An action failed and was undone", detail: e.message || undefined, title: e.message || undefined });
        void refreshList();
      }),
    );
    // An opened thread's older (headers-only) mail couldn't be downloaded:
    // without this it would say "Loading full message…" indefinitely.
    offs.push(
      onBodyFetchFailed((e) => {
        toast({
          kind: "error",
          key: `body-fetch:${e.accountId}`,
          message: "Couldn't download this message from Gmail. Reopen it to try again.",
          detail: e.message || undefined,
          title: e.message || undefined,
        });
      }),
    );
    return () => {
      lists.dispose();
      counts.dispose();
      offs.forEach((p) => void p.then((off) => off()));
      offSyncToasts();
      offUpdateToasts();
    };
  }, [active]);
}
