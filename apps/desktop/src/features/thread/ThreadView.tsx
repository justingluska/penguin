// Reading a thread. Two layouts share one component:
//  - "preview": the reading pane beside the list (design/01-inbox.html)
//  - "full":    the opened thread with its context panel (design/02-thread.html)
// Older messages are collapsed; the latest (and any unread) are expanded.
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Address, AttachmentMeta, MessageView, ThreadRef, ThreadSummary, ThreadView as Thread } from "../../lib/types";
import { getUi, goBack, setUi, useUi } from "../../lib/ui";
import { api, asCommandError } from "../../lib/api";
import { bytes, displayName, fileExt, fileTone, firstName, messageTime, shortDate } from "../../lib/format";
import { ReadReceiptLine } from "../receipts/ReadReceipts";
import { accountTone } from "../../lib/accountColor";
import { Icon } from "../../components/Icon";
import { Kbd, Keys } from "../../components/Kbd";
import { AccountBadge, Avatar, LabelChip } from "../../components/Identity";
import { toast } from "../../components/Toast";
import { MessageBody } from "../message-body/MessageBody";
import {
  accountById,
  indexOfSelected,
  isMe,
  labelById,
  list,
  listSummary,
  meta,
  moveSelection,
  opensAsRead,
  prefetchAround,
  replaceCachedMessage,
  useLabelLook,
  useThread,
} from "../../app/store";
import { archive, isUnread, markReadOnOpen, openCompose, toggleRead, toggleStar } from "../../app/actions";
import { threadNav } from "../../app/shortcuts";
import { ContextPanel } from "./ContextPanel";
import { InviteCard } from "../calendar/InviteCard";
import { openDraftForThread } from "../compose";
import { InstantDockRow } from "../compose/InstantReplies";
import { pickMessage } from "../compose/quote";
import { copyAttachmentFile, downloadAttachment } from "./AttachmentPreview";
import { openAttachment } from "./openAttachment";
import { fractionOf, progressText, useAttachmentProgress } from "./attachmentProgress";
import { ProgressRing } from "../../components/ProgressRing";
import { openMessageDetails } from "./MessageDetails";
import { PersonLink } from "../people/PersonCard";
import { useKeyTip } from "../../lib/shortcutHints";
import { useAvatarPlacement } from "../../lib/avatars";
import { MenuButton, showContextMenu } from "../../components/ContextMenu";
import { attachmentMenu, messageMenu } from "./threadMenus";
import { attachmentDragSource, canDragFiles, dragOut } from "../image-viewer/fileDrag";
import { nativeFiles } from "../image-viewer/actions";
import { cardKeyAction } from "./attachmentMenu";
import { COPY_CONVERSATION_KEYS, copyConversation } from "./copy";
import { OtpBanner } from "../otp/OtpBanner";
import { ExtensionButtons } from "../extensions/ExtensionButtons";
import { inReplyLater, toggleReplyLater } from "../triage/actions";
import { useUnsubscribeSlot } from "../unsubscribe/UnsubscribeButton";
import { SummarizeButton, SummaryCard } from "../summary/SummaryCard";
import { useSummaryShortcut } from "../summary/state";
import { openThreadWindow } from "../../app/windows";
import { isMainWindow } from "../../lib/windowBus";
import { Modal } from "../../components/Modal";
import { useSetting } from "../../lib/settings";
import { Timeline, publishTimeline } from "./Timeline";
import { useHandoffThread } from "../handoff";

type Variant = "preview" | "full";

export const ThreadPane = memo(function ThreadPane({ variant }: { variant: Variant }) {
  const ref = useUi((s) => s.selected);
  const contextPanel = useUi((s) => s.contextPanel);
  const { thread, loading, missing } = useThread(ref);
  useSummaryShortcut();

  useEffect(() => {
    if (ref) prefetchAround();
  }, [ref]);

  useMarkRead(ref, thread);
  // Handoff: the conversation on screen is offered to the iPhone (features/handoff).
  useHandoffThread(ref ? thread : null);

  if (variant === "preview") {
    return (
      <aside className="preview">
        <header className="pane-head preview-head" data-tauri-drag-region>
          <PaneActions variant="preview" thread={thread} />
        </header>
        {ref && thread ? (
          <ThreadBody key={ref.accountId + ref.threadId} thread={thread} variant="preview" />
        ) : ref && loading && listSummary(ref) ? (
          <PaneLoading summary={listSummary(ref)!} variant="preview" />
        ) : (
          <PaneEmpty loading={!!ref && loading} missing={missing} />
        )}
      </aside>
    );
  }

  return (
    <>
      <section className="thread">
        <header className="pane-head thread-head" data-tauri-drag-region>
          {/* A conversation window has nowhere to go back to: Esc (or ⌘W) closes it. */}
          {isMainWindow && (
            <>
              <button className="btn btn-ghost btn-sm" onClick={goBack}>
                <Icon name="left" size="xs" />
                Back
                <Kbd>Esc</Kbd>
              </button>
              <span className="head-sep" />
            </>
          )}
          <PaneActions variant="full" thread={thread} />
        </header>
        {ref && thread ? (
          <ThreadBody key={ref.accountId + ref.threadId} thread={thread} variant="full" />
        ) : ref && loading && listSummary(ref) ? (
          <PaneLoading summary={listSummary(ref)!} variant="full" />
        ) : (
          <PaneEmpty loading={!!ref && loading} missing={missing} />
        )}
      </section>
      {contextPanel && thread && <ContextPanel thread={thread} />}
    </>
  );
});

function PaneEmpty({ loading, missing }: { loading: boolean; missing: boolean }) {
  if (loading) return <div className="pane-empty"><span className="sk" style={{ width: "60%", height: 18 }} /></div>;
  return (
    <div className="pane-empty">
      <span className="faint">{missing ? "This conversation is no longer available." : "No conversation selected"}</span>
    </div>
  );
}

/**
 * The conversation's subject from its list row, in the place the loaded
 * thread puts it, while the thread itself is read (usually a frame or two):
 * the pane changes at once, and the body fills in below without a jump.
 */
function PaneLoading({ summary, variant }: { summary: ThreadSummary; variant: Variant }) {
  const lines = (
    <div aria-hidden="true" style={{ display: "grid", gap: 10, marginTop: 18 }}>
      <span className="sk" style={{ width: "34%", height: 12 }} />
      <span className="sk" style={{ width: "88%" }} />
      <span className="sk" style={{ width: "72%" }} />
    </div>
  );
  if (variant === "preview")
    return (
      <div className="preview-body v-scroll" aria-busy="true">
        <h2 className="subject">{summary.subject || "(no subject)"}</h2>
        {lines}
      </div>
    );
  return (
    <div className="thread-scroll v-scroll" aria-busy="true">
      <div className="thread-col">
        <h1 className="thread-title">{summary.subject || "(no subject)"}</h1>
        {lines}
      </div>
    </div>
  );
}

// Holding j/k (key auto-repeat) skims past conversations: only the one it
// stops on counts as opened, when the key comes up.
let keyHeld = false;
const onRelease = new Set<() => void>();
let heldTracked = false;
function trackHeldKeys() {
  if (heldTracked || typeof window === "undefined") return;
  heldTracked = true;
  const release = () => {
    keyHeld = false;
    const run = [...onRelease];
    onRelease.clear();
    run.forEach((f) => f());
  };
  window.addEventListener("keydown", (e) => (keyHeld = e.repeat), true);
  window.addEventListener("keyup", release, true);
  window.addEventListener("blur", release);
}

/**
 * Mark a conversation read the moment you open it: selected by you (click,
 * j/k, Enter, a search result, the next one after an archive), not placed by
 * the app. It happens in the same frame as the selection, before paint, and
 * doesn't wait for the thread to load when its list row already says it's
 * unread; the row, the counts and the thread all change at once
 * (app/actions markRead) and the backend follows. A hidden account's mail
 * opened from search, the person card or a notification stays unread
 * (store.opensAsRead).
 */
function useMarkRead(ref: ThreadRef | null, thread: Thread | null) {
  trackHeldKeys();
  const byApp = useUi((s) => s.selectedByApp);
  const doneFor = useRef<string | null>(null);
  const key = ref ? ref.accountId + "\u0000" + ref.threadId : null;
  // Its read state is known from its list row, else once the thread has loaded.
  const known = !!ref && (listSummary(ref) !== undefined || thread?.threadId === ref.threadId);
  useLayoutEffect(() => {
    if (!key || !known || byApp || doneFor.current === key) return;
    const run = () => {
      doneFor.current = key;
      const cur = getUi().selected;
      if (cur && cur.accountId + "\u0000" + cur.threadId === key && isUnread(cur) && opensAsRead(cur)) markReadOnOpen(cur);
    };
    if (!keyHeld) return run();
    onRelease.add(run);
    return () => void onRelease.delete(run);
    // Read state is looked up when it runs, not on every thread update: that
    // would re-mark a thread the user just marked unread with U.
  }, [key, known, byApp]);
}

// `thread` re-renders the row when the open thread's read state changes (it
// may not be in the list, e.g. opened from search).
function PaneActions({ variant, thread }: { variant: Variant; thread: Thread | null }) {
  const tip = useKeyTip();
  const has = useUi((s) => s.selected !== null);
  const items = list.use((l) => l.items);
  const hasMore = list.use((l) => l.hasMore);
  useUi((s) => s.selected); // position text follows the cursor
  const i = indexOfSelected();
  // Not in the list (a conversation window, a search result): the thread says.
  const starred = i >= 0 ? items[i].starred : !!thread && thread.messages.some((m) => m.starred);
  const view = useUi((s) => s.view.kind);
  // Labels load with meta; re-render when they do so the button reads right.
  useLabelLook();
  const cur = useUi((s) => s.selected);
  const laterNow = !!cur && inReplyLater(cur);
  const unread = !!cur && (thread?.threadId === cur.threadId ? thread.messages.some((m) => m.unread) : isUnread(cur));
  const selectedAccount = useUi((s) => s.selected?.accountId ?? null);
  const caps = meta.use((m) => m.accounts.find((a) => a.id === selectedAccount)?.capabilities);
  return (
    <>
      {/* Narrow panes drop the key caps, then the words (app.css, .pane-act); the title names each one. */}
      <button
        className="btn btn-ghost btn-sm pane-act"
        disabled={!has}
        data-shortcut="triage.done"
        data-shortcut-label={view === "followUp" ? "Dismiss" : view === "spam" ? "Not spam" : "Mark done"}
        title={tip(view === "followUp" ? "Dismiss" : view === "spam" ? "Not spam: move to the inbox" : "Done", "E")}
        onClick={() => void archive()}
      >
        <Icon name={view === "spam" ? "inbox" : "done"} size="xs" />
        <span className="btn-label">{view === "followUp" ? "Dismiss" : view === "spam" ? "Not spam" : "Done"}</span>
        <Kbd>E</Kbd>
      </button>
      <button
        className="btn btn-ghost btn-sm pane-act"
        disabled={!has}
        data-shortcut="triage.read"
        data-shortcut-label={unread ? "Mark read" : "Mark unread"}
        title={tip(unread ? "Mark read" : "Mark unread", "U")}
        onClick={() => cur && toggleRead([cur])}
      >
        <Icon name={unread ? "mail" : "unread"} size="xs" />
        <span className="btn-label">{unread ? "Mark read" : "Mark unread"}</span>
        <Kbd>U</Kbd>
      </button>
      <button
        className="btn btn-ghost btn-sm pane-act"
        disabled={!has}
        data-shortcut="triage.replyLater"
        data-shortcut-label={laterNow ? "Back to inbox" : "Reply later"}
        title={laterNow ? "Take it out of Reply Later, back to the inbox" : "Out of the inbox until you reply"}
        onClick={() => toggleReplyLater()}
      >
        <Icon name={laterNow ? "inbox" : "replyLater"} size="xs" />
        <span className="btn-label">{laterNow ? "Back to inbox" : "Reply later"}</span>
        <Kbd>Y</Kbd>
      </button>
      {/* Snooze (H) lives in the right-click menu and ⌘K: Reply later is the
          toolbar's "not now", and Forward had no one-click home in the split view. */}
      <button
        className="btn btn-ghost btn-sm pane-act"
        disabled={!has}
        data-shortcut="compose.forward"
        title={tip("Forward", "F")}
        onClick={() => openCompose("forward")}
      >
        <Icon name="forward" size="xs" />
        <span className="btn-label">Forward</span>
        <Kbd>F</Kbd>
      </button>
      {/* Label where the account has labels; Move to on folder accounts (IMAP), where L moves too. */}
      {(!caps || caps.labels) && (
        <button
          className="btn btn-ghost btn-sm pane-act"
          disabled={!has}
          data-shortcut="triage.label"
          title={tip("Label", "L")}
          onClick={() => setUi({ overlay: "label" })}
        >
          <Icon name="tag" size="xs" />
          <span className="btn-label">Label</span>
          <Kbd>L</Kbd>
        </button>
      )}
      {caps?.folders && (
        <button
          className="btn btn-ghost btn-sm pane-act"
          disabled={!has}
          data-shortcut="triage.move"
          title={tip("Move to", "V")}
          onClick={() => setUi({ overlay: "move" })}
        >
          <Icon name="folder" size="xs" />
          <span className="btn-label">Move to</span>
          <Kbd>V</Kbd>
        </button>
      )}
      <button
        className="btn btn-ghost btn-sm pane-act"
        disabled={!has}
        data-shortcut="thread.copy"
        title={tip("Copy conversation as plain text", "⇧C")}
        onClick={() => copyConversation()}
      >
        <Icon name="copy" size="xs" />
        <span className="btn-label">Copy</span>
        <Keys keys={COPY_CONVERSATION_KEYS} />
      </button>
      {variant === "full" && (
        <button
          className="btn btn-ghost btn-sm pane-act"
          disabled={!has}
          data-shortcut="triage.star"
          data-shortcut-label={starred ? "Unstar" : "Star"}
          title={tip(starred ? "Unstar" : "Star", "S")}
          onClick={() => toggleStar()}
        >
          <Icon name="star" size="xs" fill={starred} className={starred ? "star-on" : undefined} />
          <span className="btn-label">{starred ? "Starred" : "Star"}</span>
          <Kbd>S</Kbd>
        </button>
      )}
      {/* Apple Intelligence summary (features/summary); hidden where it can't run. Icon-only in the narrow preview pane. */}
      <SummarizeButton compact={variant === "preview"} />
      <span className="grow" />
      {/* A conversation window shows one conversation: no list to step through or open from. */}
      {isMainWindow && (
        <>
          <button
            className="btn btn-ghost btn-sm btn-icon"
            disabled={!has}
            data-shortcut="thread.openWindow"
            title={tip("Open in new window", "⇧O")}
            aria-label="Open in new window"
            onClick={() => cur && void openThreadWindow(cur)}
          >
            <Icon name="window" size="xs" />
          </button>
          {i >= 0 && (
            <span className="faint tnum pos">
              {i + 1} of {items.length}
              {hasMore ? "+" : ""}
            </span>
          )}
          <button className="btn btn-ghost btn-sm" data-shortcut="nav.up" title={tip("Previous", "K")} onClick={() => moveSelection(-1)}>
            <Icon name="up" size="xs" />
            {variant === "full" && <Kbd>K</Kbd>}
          </button>
          <button className="btn btn-ghost btn-sm" data-shortcut="nav.down" title={tip("Next", "J")} onClick={() => moveSelection(1)}>
            <Icon name="down" size="xs" />
            {variant === "full" && <Kbd>J</Kbd>}
          </button>
        </>
      )}
    </>
  );
}

// ---------------------------------------------------------------------------


/**
 * "to you, Priya +2" plus which of my accounts got it, as quiet text:
 * "(sam@northwind.example)", or "· Sam Work" when the account has a nickname.
 * Mail I sent already shows the account as its sender, so it gets no suffix.
 */
const RECIPIENTS_SHOWN = 4;

function RecipientLine({ m, style }: { m: MessageView; style?: React.CSSProperties }) {
  const account = accountById(m.accountId);
  const suffix = account && !isMe(m.from.email);
  const [all, setAll] = useState(false);
  const list = [...m.to, ...m.cc];
  const shown = all ? list : list.slice(0, RECIPIENTS_SHOWN);
  return (
    <div className={"faint small recip-line" + (all ? " is-open" : " truncate")} style={style}>
      {list.length > 0 && "to "}
      {shown.map((a, i) => (
        <span key={a.email + i}>
          {i > 0 && ", "}
          {isMe(a.email) ? "you" : <PersonLink person={a} />}
        </span>
      ))}
      {!all && list.length > RECIPIENTS_SHOWN && (
        <button
          type="button"
          className="person-more"
          title={list.slice(RECIPIENTS_SHOWN).map((a) => a.email).join(", ")}
          onClick={(e) => {
            e.stopPropagation();
            setAll(true);
          }}
        >
          +{list.length - RECIPIENTS_SHOWN}
        </button>
      )}
      {suffix && (
        <span className="recip-acct" title={account.email}>
          <i className={`dot dot-sm t-${accountTone(account.color)}`} />
          {account.nickname ? `· ${account.nickname}` : `(${account.email})`}
        </span>
      )}
    </div>
  );
}

function nameOf(a: Address): string {
  return isMe(a.email) ? "You" : displayName(a);
}

function ThreadBody({ thread, variant }: { thread: Thread; variant: Variant }) {
  const tip = useKeyTip();
  // Settings → "Sender photos appear": message headers get a 32 px avatar.
  const headerAvatars = useAvatarPlacement().message;
  const focusMessageId = useUi((s) => s.focusMessageId);
  const msgs = thread.messages;
  const last = msgs.length - 1;
  const [expanded, setExpanded] = useState<Set<string>>(() => {
    const s = new Set<string>();
    msgs.forEach((m, i) => {
      if (i === last || m.unread || m.id === focusMessageId) s.add(m.id);
    });
    return s;
  });
  // Preview: everything before the latest hides behind "N earlier messages".
  // Full: a long middle run hides behind a dashed "N more messages" divider.
  const [showAll, setShowAll] = useState(false);
  const [cursor, setCursor] = useState(() => {
    const f = focusMessageId ? msgs.findIndex((m) => m.id === focusMessageId) : -1;
    return f >= 0 ? f : last;
  });
  const [flash, setFlash] = useState<string | null>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const itemRefs = useRef(new Map<string, HTMLElement>());
  // A conversation with replies shows one message at a time, the latest
  // unless a search or a summary link points at another, with a timeline of
  // every message (newest first): beside it in the full view, in a dialog
  // from the reading pane. "Show all" (o, full view) stacks them as before.
  const latestFirst = useSetting("threadLatestFirst");
  const timeline = msgs.length > 1 && latestFirst;
  const [shownId, setShownId] = useState<string | undefined>(() => (focusMessageId && msgs.some((m) => m.id === focusMessageId) ? focusMessageId : msgs[last]?.id));
  const [stackAll, setStackAll] = useState(false);
  const single = timeline && !stackAll;

  // New messages arriving in an open thread: expand the newest one, and
  // follow it when the latest was the one on screen.
  const lastId = msgs[last]?.id;
  const prevLast = useRef(lastId);
  useEffect(() => {
    if (lastId) setExpanded((s) => (s.has(lastId) ? s : new Set(s).add(lastId)));
    setShownId((cur) => (cur === prevLast.current || !msgs.some((m) => m.id === cur) ? lastId : cur));
    prevLast.current = lastId;
  }, [lastId, msgs]);

  const reveal = useCallback(
    (id: string, highlight: boolean) => {
      if (single) {
        setShownId(id);
        requestAnimationFrame(() => {
          scrollRef.current?.scrollTo({ top: 0 });
          if (highlight) {
            setFlash(id);
            setTimeout(() => setFlash((f) => (f === id ? null : f)), 1600);
          }
        });
        return;
      }
      setShowAll(true);
      setExpanded((s) => (s.has(id) ? s : new Set(s).add(id)));
      requestAnimationFrame(() => {
        itemRefs.current.get(id)?.scrollIntoView({ block: "start", behavior: "auto" });
        if (highlight) {
          setFlash(id);
          setTimeout(() => setFlash((f) => (f === id ? null : f)), 1600);
        }
      });
    },
    [single],
  );

  // A summary's source link: open that message, move the cursor to it, flash it.
  const jumpTo = useCallback(
    (id: string) => {
      const i = msgs.findIndex((m) => m.id === id);
      if (i < 0) return;
      setCursor(i);
      reveal(id, true);
    },
    [msgs, reveal],
  );

  // Opened from search: jump to that message and flash it, once.
  useEffect(() => {
    if (focusMessageId && msgs.some((m) => m.id === focusMessageId)) {
      const i = msgs.findIndex((m) => m.id === focusMessageId);
      setCursor(i);
      reveal(focusMessageId, true);
      setUi({ focusMessageId: null });
    }
  }, [focusMessageId, msgs, reveal]);

  // n / p / o in the full view.
  useEffect(() => {
    if (variant !== "full") return;
    threadNav.next = () => {
      const i = Math.min(last, cursor + 1);
      setCursor(i);
      reveal(msgs[i].id, false);
    };
    threadNav.prev = () => {
      const i = Math.max(0, cursor - 1);
      setCursor(i);
      reveal(msgs[i].id, false);
    };
    threadNav.expandAll = () => {
      if (timeline) {
        showEverything(!stackAll);
        return;
      }
      setShowAll(true);
      setExpanded((s) => (s.size === msgs.length ? new Set([msgs[last].id]) : new Set(msgs.map((m) => m.id))));
    };
    threadNav.latest = () => {
      setCursor(last);
      reveal(msgs[last].id, false);
    };
    return () => {
      threadNav.next = () => {};
      threadNav.prev = () => {};
      threadNav.expandAll = () => {};
      threadNav.latest = () => {};
    };
  }, [variant, cursor, last, msgs, reveal, timeline, stackAll]);

  /** Timeline: stack every message (expanded) or go back to one at a time. */
  const showEverything = (on: boolean) => {
    setStackAll(on);
    if (on) {
      setShowAll(true);
      setExpanded(new Set(msgs.map((m) => m.id)));
    }
    requestAnimationFrame(() => scrollRef.current?.scrollTo({ top: 0 }));
  };

  const pick = (id: string) => {
    const i = msgs.findIndex((m) => m.id === id);
    if (i < 0) return;
    setCursor(i);
    reveal(id, false);
  };

  // With the context panel showing, the timeline moves into it (under the
  // person, before their meetings) and the message takes the whole column.
  const panelShown = usePanelShown();
  const inPanel = variant === "full" && timeline && panelShown;
  const latest = useRef({ pick, toggleAll: () => showEverything(!stackAll) });
  latest.current = { pick, toggleAll: () => showEverything(!stackAll) };
  const shownNow = single ? (msgs.find((m) => m.id === shownId) ?? msgs[last])?.id ?? null : null;
  useEffect(() => {
    if (!inPanel) return;
    publishTimeline({
      threadKey: `${thread.accountId}/${thread.threadId}`,
      shownId: shownNow,
      all: stackAll,
      pick: (id) => latest.current.pick(id),
      toggleAll: () => latest.current.toggleAll(),
    });
    return () => publishTimeline(null);
  }, [inPanel, thread.accountId, thread.threadId, shownNow, stackAll]);

  // Reading pane: the timeline in a dialog, from the latest message's bar.
  const [timelineOpen, setTimelineOpen] = useState(false);

  const toggle = (id: string) =>
    setExpanded((s) => {
      const n = new Set(s);
      if (n.has(id) && id !== msgs[last].id) n.delete(id);
      else n.add(id);
      return n;
    });

  const onLoadImages = useCallback(
    (m: MessageView) => {
      api.loadRemoteImages(m.accountId, m.id).then(
        (v) => replaceCachedMessage({ accountId: thread.accountId, threadId: thread.threadId }, v),
        (e) => toast({ tone: "error", message: `Couldn't load images: ${asCommandError(e).message}` }),
      );
    },
    [thread.accountId, thread.threadId],
  );

  const account = accountById(thread.accountId);
  useLabelLook();
  const chips = thread.labelIds.map((id) => labelById(thread.accountId, id)).filter((l) => l && l.kind === "user");
  const people = useMemo(() => {
    const seen = new Map<string, Address>();
    for (const m of msgs) for (const a of [m.from, ...m.to, ...m.cc]) if (!seen.has(a.email)) seen.set(a.email, a);
    return [...seen.values()];
  }, [msgs]);

  // Labels and counts sit quietly under the subject; the receiving account
  // is named in each message's recipient line instead of a badge up here.
  const header =
    chips.length > 0 || variant === "full" ? (
      <div className="row-flex thread-meta">
        {chips.map((l) => (
          <LabelChip key={l!.id} label={l!} />
        ))}
        {variant === "full" && (
          <span className="faint thread-count">
            {msgs.length} {msgs.length === 1 ? "message" : "messages"} · {people.length} {people.length === 1 ? "person" : "people"}
          </span>
        )}
      </div>
    ) : null;

  // Which messages show as rows vs. hidden behind a divider. A message with
  // files is never folded away: it stays a collapsed row naming them, so an
  // attachment halfway down a long thread can be seen without unfolding it.
  type Slot = { kind: "msg"; m: MessageView; i: number } | { kind: "more"; n: number; names: string; at: number };
  const slots: Slot[] = [];
  const fold = (hidden: (m: MessageView, i: number) => boolean, named: boolean) => {
    let run: MessageView[] = [];
    let at = 0;
    const flush = () => {
      if (run.length === 0) return;
      // One message takes no more room as its own row than as a divider.
      if (run.length === 1) {
        slots.push({ kind: "msg", m: run[0], i: at });
        run = [];
        return;
      }
      const names = named ? [...new Set(run.map((m) => (isMe(m.from.email) ? "you" : firstName(m.from))))].slice(0, 3).join(", ") : "";
      slots.push({ kind: "more", n: run.length, names, at });
      run = [];
    };
    msgs.forEach((m, i) => {
      if (hidden(m, i) && !hasFiles(m)) {
        if (run.length === 0) at = i;
        run.push(m);
        return;
      }
      flush();
      slots.push({ kind: "msg", m, i });
    });
    flush();
  };
  if (variant === "preview") {
    fold((_, i) => !showAll && i < last, true);
  } else {
    const hideFrom = 1;
    const hideTo = last - 2; // keep the first, the two before the latest, the latest
    fold((m, i) => !showAll && msgs.length >= 4 && i >= hideFrom && i <= hideTo && !expanded.has(m.id), false);
  }

  const setItemRef = (id: string) => (el: HTMLElement | null) => {
    if (el) itemRefs.current.set(id, el);
    else itemRefs.current.delete(id);
  };

  const lastMsg = msgs[last];
  const replyTo = lastMsg && (isMe(lastMsg.from.email) ? lastMsg.to[0] ?? lastMsg.from : lastMsg.from);
  // Who the latest message's reply all reaches (as compose/draft.ts fills
  // it); with more than one person, the reply prompt replies to all of them.
  const replyAllTo = replyAllPeople(pickMessage(msgs, undefined));
  const canReplyAll = replyAllTo.length > 1;
  // The prompt's icon switches it between reply all and the sender only;
  // the choice lasts while this conversation is open.
  const threadKey = `${thread.accountId}:${thread.threadId}`;
  const [replyPick, setReplyPick] = useState<{ key: string; mode: ReplyMode } | null>(null);
  const replyMode: ReplyMode = canReplyAll ? (replyPick?.key === threadKey ? replyPick.mode : "replyAll") : "reply";
  const toAll = replyMode === "replyAll";
  const replyModeSwitch = canReplyAll && (
    <ReplyModeSwitch
      mode={replyMode}
      sender={replyTo ? firstName(replyTo) : "the sender"}
      onPick={(mode) => setReplyPick({ key: threadKey, mode })}
    />
  );
  const replyNames = (
    <>
      {replyAllTo.slice(0, 3).map((p, i) => (
        <span key={p.email}>
          {i > 0 && (i === replyAllTo.length - 1 ? " and " : ", ")}
          <span className="emph">{displayName(p)}</span>
        </span>
      ))}
      {replyAllTo.length > 3 && ` +${replyAllTo.length - 3}`}
    </>
  );

  // The message on screen (one at a time), and the bar that says it isn't the latest.
  const shownAt = Math.max(0, msgs.findIndex((m) => m.id === shownId));
  const shownMsg = msgs[shownAt] ?? msgs[last];
  const olderBar =
    single && shownAt !== last ? (
      <div className="tl-older" role="status">
        <span>Older message · {messageTime(shownMsg.date)}</span>
        <button className="tl-back" onClick={() => pick(msgs[last].id)}>
          <Icon name="up" size="xs" />
          Back to latest ({firstName(msgs[last].from)}, {messageTime(msgs[last].date)})
          <Kbd>⇧L</Kbd>
        </button>
      </div>
    ) : null;

  // Reading pane: the earlier messages, folded into one bar above the one on
  // screen (who wrote them, how many); a click opens the timeline.
  const earlierFrom = [...new Map(msgs.slice(0, last).reverse().map((m) => [m.from.email.toLowerCase(), m.from])).values()];
  const earlierBar = single ? (
    <button type="button" className="tl-earlier" onClick={() => setTimelineOpen(true)} title="Every message in this conversation, newest first">
      <span className="tl-earlier-faces" aria-hidden="true">
        {earlierFrom.slice(0, 3).map((a) => (
          <Avatar key={a.email} person={a} size="xs" tone={isMe(a.email) ? "gray" : undefined} photo={!isMe(a.email)} />
        ))}
      </span>
      <span className="tl-earlier-text truncate">
        <strong>
          {last} earlier {last === 1 ? "message" : "messages"}
        </strong>
        {" · from "}
        {joinNames(earlierFrom.map((a) => (isMe(a.email) ? "you" : firstName(a))))}
      </span>
      <span className="tl-earlier-go">
        <Icon name="history" size="xs" />
        Timeline
      </span>
    </button>
  ) : null;

  if (variant === "preview") {
    const previewMessage = (m: MessageView, i: number) => (
        <div
          key={m.id}
          ref={setItemRef(m.id)}
          className={"pv-msg" + (flash === m.id ? " is-flash" : "")}
          onContextMenu={(e) => showContextMenu(e, messageMenu(m, { expanded: true, onToggle: i !== last ? () => toggle(m.id) : undefined }))}
        >
          <div className="sender-row">
            {headerAvatars && (
              <Avatar person={m.from} size="md" authenticated={m.senderAuthenticated ?? null} photo={!isMe(m.from.email)} />
            )}
            <div className="grow" style={{ minWidth: 0 }}>
              <div className="row-flex from-line" style={{ gap: 6 }}>
                <PersonLink person={m.from} label={nameOf(m.from)} className="emph truncate pl-name" />
                <PersonLink person={m.from} label={m.from.email} className="faint truncate small" />
              </div>
              <RecipientLine m={m} />
            </div>
            <ExtensionButtons m={m} />
            {m.labelIds.includes("DRAFT") ? (
              <button
                className="btn btn-secondary btn-sm"
                onClick={() => void openDraftForThread({ accountId: thread.accountId, threadId: thread.threadId }, m.id)}
              >
                <Icon name="compose" size="xs" />
                Edit draft
              </button>
            ) : (
              <button className="m-time faint tnum small" title="Message details" onClick={() => openMessageDetails(m)}>
                {shortDate(m.date)}
              </button>
            )}
          </div>
          <MessageContent
            m={m}
            onLoadImages={() => onLoadImages(m)}
          />
          <AttachmentList m={m} layout="stack" />
        </div>
    );
    return (
      <>
        {timelineOpen && (
          <Modal label="Timeline" onClose={() => setTimelineOpen(false)} className="tl-modal">
            <Timeline
              msgs={msgs}
              shownId={shownMsg.id}
              where="dialog"
              onPick={(id) => {
                setTimelineOpen(false);
                pick(id);
              }}
            />
          </Modal>
        )}
        <div className="preview-body v-scroll" ref={scrollRef}>
          <h2 className="subject">{thread.subject || "(no subject)"}</h2>
          {header}
          <InviteCard thread={thread} />
          <SummaryCard thread={thread} onJump={jumpTo} />
          <div className="preview-stack">
            {single ? (
              <>
                {earlierBar}
                {olderBar}
                {previewMessage(shownMsg, shownAt)}
              </>
            ) : (
              slots.map((s) =>
              s.kind === "more" ? (
                <button key={`more-${s.at}`} className="earlier" onClick={() => setShowAll(true)}>
                  <Icon name="mail" size="xs" />
                  {s.n} earlier {s.n === 1 ? "message" : "messages"}
                  {s.names && ` · ${s.names}`}
                </button>
              ) : expanded.has(s.m.id) ? (
previewMessage(s.m, s.i)
              ) : (
                <CollapsedRow key={s.m.id} m={s.m} onClick={() => toggle(s.m.id)} refCb={setItemRef(s.m.id)} flash={flash === s.m.id} />
              ),
            ))}
          </div>
        </div>
        <div className="quick-reply">
          <div className="qr-row">
            <div
              className="qr-box qr-reply"
              role="button"
              tabIndex={0}
              onClick={() => openCompose(replyMode)}
              onKeyDown={(e) => {
                if (e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) {
                  e.preventDefault();
                  openCompose(replyMode);
                }
              }}
            >
              {replyModeSwitch}
              <span className="placeholder truncate">
                {toAll ? `Reply all to ${nameList(replyAllTo.map(firstName))}…` : `Reply to ${replyTo ? firstName(replyTo) : "sender"}…`}
              </span>
              <Kbd>{toAll ? "A" : "R"}</Kbd>
            </div>
            <button className="qr-box qr-fwd" title="Forward (F)" onClick={() => openCompose("forward")}>
              <Icon name="forward" size="xs" />
              <span>Forward</span>
            </button>
          </div>
        </div>
      </>
    );
  }

  // One message, in full: the shown one in the timeline, or each in the stack.
  const fullMessage = (m: MessageView, i: number) => (
    <article
          key={m.id}
          ref={setItemRef(m.id)}
          className={"message" + (i === cursor ? " is-cursor" : "") + (flash === m.id ? " is-flash" : "")}
          onContextMenu={(e) => showContextMenu(e, messageMenu(m, { expanded: true, onToggle: !single && i !== last ? () => toggle(m.id) : undefined }))}
        >
          <header className="m-head" onClick={() => !single && i !== last && toggle(m.id)}>
            {headerAvatars && (
              <Avatar person={m.from} size="md" authenticated={m.senderAuthenticated ?? null} photo={!isMe(m.from.email)} />
            )}
            <div className="grow" style={{ minWidth: 0 }}>
              <div className="row-flex from-line" style={{ gap: 8 }}>
                <PersonLink person={m.from} label={nameOf(m.from)} className="m-name" />
                <PersonLink person={m.from} label={m.from.email} className="faint small truncate" />
              </div>
              <RecipientLine m={m} style={{ lineHeight: "16px" }} />
            </div>
            {m.labelIds.includes("DRAFT") && (
              <button
                className="btn btn-secondary btn-sm"
                onClick={(e) => {
                  e.stopPropagation();
                  void openDraftForThread({ accountId: thread.accountId, threadId: thread.threadId }, m.id);
                }}
              >
                <Icon name="compose" size="xs" />
                Edit draft
              </button>
            )}
            {timeline && i === last && <span className="badge t-blue m-latest">Latest</span>}
            <button
              className="m-time faint tnum small"
              title="Message details"
              onClick={(e) => {
                e.stopPropagation();
                openMessageDetails(m);
              }}
            >
              {messageTime(m.date)}
            </button>
            <ExtensionButtons m={m} />
            <button
              className="btn btn-ghost btn-sm btn-icon"
              data-shortcut="compose.reply"
              title={tip("Reply", "R")}
              onClick={(e) => {
                e.stopPropagation();
                setUi({ overlay: "compose", composeContext: { mode: "reply", thread: { accountId: thread.accountId, threadId: thread.threadId }, messageId: m.id } });
              }}
            >
              <Icon name="reply" size="xs" />
            </button>
            <button
              className="btn btn-ghost btn-sm btn-icon"
              data-shortcut="compose.forward"
              title={tip("Forward", "F")}
              onClick={(e) => {
                e.stopPropagation();
                setUi({ overlay: "compose", composeContext: { mode: "forward", thread: { accountId: thread.accountId, threadId: thread.threadId }, messageId: m.id } });
              }}
            >
              <Icon name="forward" size="xs" />
            </button>
          </header>
          <div className="m-body">
            <MessageContent m={m} onLoadImages={() => onLoadImages(m)} />
          </div>
          <AttachmentList m={m} layout="grid" />
        </article>
  );
  const shownIdx = shownAt;
  const stack = (
          <div className="stack">
            {slots.map((s) =>
              s.kind === "more" ? (
                <button key={`more-${s.at}`} className="collapsed-more" onClick={() => setShowAll(true)}>
                  <span className="line-l" />
                  <span>
                    {s.n} more {s.n === 1 ? "message" : "messages"}
                  </span>
                  <span className="line-l" />
                </button>
              ) : expanded.has(s.m.id) ? (
                fullMessage(s.m, s.i)
              ) : (
                <CollapsedRow key={s.m.id} m={s.m} onClick={() => toggle(s.m.id)} refCb={setItemRef(s.m.id)} flash={flash === s.m.id} />
              ),
            )}
          </div>
  );
  return (
    <>
      <div className="thread-scroll v-scroll" ref={scrollRef}>
        <div className={"thread-col" + (timeline && !inPanel ? " has-timeline" : "")}>
          <h1 className="thread-title">{thread.subject || "(no subject)"}</h1>
          {header}
          <InviteCard thread={thread} />
          <SummaryCard thread={thread} onJump={jumpTo} />
          {timeline ? (
            <div className={"tl-layout" + (inPanel ? " is-solo" : "")}>
              <div className="tl-main">
                {single ? (
                  <>
                    {olderBar}
                    {fullMessage(msgs[shownIdx], shownIdx)}
                  </>
                ) : (
                  stack
                )}
              </div>
              {!inPanel && <Timeline msgs={msgs} shownId={single ? msgs[shownIdx].id : null} all={stackAll} onPick={pick} onAll={() => showEverything(!stackAll)} />}
            </div>
          ) : (
            stack
          )}
        </div>
      </div>
      <div className="reply-dock">
        <div className="reply reply-prompt">
          {/* The prompt starts the reply; its icon switches reply all ⇄ the sender only. */}
          <div className="reply-to" onClick={() => openCompose(replyMode)}>
            {replyModeSwitch || <Icon name="reply" size="xs" />}
            <span className="truncate">
              {toAll ? "Reply all to " : "Reply to "}
              {toAll ? replyNames : <span className="emph">{replyTo ? displayName(replyTo) : "sender"}</span>}
            </span>
            <span className="grow" />
            {account && <AccountBadge account={account} label={account.email} />}
          </div>
          <button className="reply-open" onClick={() => openCompose(replyMode)}>
            <div className="reply-body placeholder-text">Write a reply…</div>
          </button>
          {/* Instant replies (Settings → Compose): a click opens the reply with it. */}
          <InstantDockRow onOpen={() => openCompose(replyMode)} />
          <div className="reply-tools">
            <button className="btn btn-ghost btn-sm" onClick={() => openCompose("forward")}>
              <Icon name="forward" size="xs" />
              Forward
              <Kbd>F</Kbd>
            </button>
          </div>
        </div>
      </div>
    </>
  );
}

type ReplyMode = "reply" | "replyAll";

/**
 * The reply prompt's icon: a menu that switches it between reply all and
 * the sender only. Its clicks stay here instead of opening the composer.
 */
function ReplyModeSwitch({ mode, sender, onPick }: { mode: ReplyMode; sender: string; onPick: (mode: ReplyMode) => void }) {
  return (
    <span className="reply-mode" onClick={(e) => e.stopPropagation()} onKeyDown={(e) => e.stopPropagation()}>
      <MenuButton
        className="reply-mode-btn"
        label="Reply to"
        title={mode === "replyAll" ? `Replying to everyone. Switch to ${sender} only` : `Replying to ${sender} only. Switch to everyone`}
        items={() => [
          { label: "Reply all", icon: "replyall", keys: "a", checked: mode === "replyAll", onSelect: () => onPick("replyAll") },
          { label: `Reply to ${sender} only`, icon: "reply", keys: "r", checked: mode === "reply", onSelect: () => onPick("reply") },
        ]}
      >
        <Icon name={mode === "replyAll" ? "replyall" : "reply"} size="xs" />
        <Icon name="down" size="xs" className="reply-mode-caret" />
      </MenuButton>
    </span>
  );
}

/** "Priya and Marco", "Priya, Marco and Alden", "Priya, Marco, Alden +2". */
function nameList(names: string[]): string {
  if (names.length > 3) return `${names.slice(0, 3).join(", ")} +${names.length - 3}`;
  return names.length > 1 ? `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}` : names.join("");
}

/**
 * Everyone but me that reply all to `m` addresses — the reply's To plus its
 * Cc, the same people compose/draft.ts prefill() fills in.
 */
function replyAllPeople(m: MessageView | undefined): Address[] {
  if (!m) return [];
  const fromMe = isMe(m.from.email);
  const primary = fromMe ? m.to : m.replyTo.length ? m.replyTo : [m.from];
  const all = [...primary, ...(fromMe ? m.cc : [...m.to, ...m.cc])];
  const seen = new Set<string>();
  return all.filter((a) => {
    const k = a.email.toLowerCase();
    if (isMe(k) || seen.has(k)) return false;
    seen.add(k);
    return true;
  });
}

/**
 * The body, or — for mail outside the sync window that's stored headers-only —
 * its snippet and a "Loading full message…" line while get_thread downloads
 * the body; the mail-changed that follows re-renders this with the real body.
 */
function MessageContent({ m, onLoadImages, leading }: { m: MessageView; onLoadImages: () => void; leading?: React.ReactNode }) {
  // Verification code / sign-in link banner (features/otp).
  const banner = m.otp && <OtpBanner m={m} />;
  // Unsubscribe button at the end of the privacy row (features/unsubscribe).
  const unsubscribe = useUnsubscribeSlot(m);
  if (m.bodyPending) {
    return (
      <div className="body-pending">
        {banner}
        {m.snippet && <p className="body-pending-snip">{m.snippet}</p>}
        <div className="body-pending-line faint small" role="status">
          <span className="spinner" aria-hidden="true" />
          Loading full message…
        </div>
      </div>
    );
  }
  return (
    <>
      <ReadReceiptLine m={m} />
      {banner}
      <MessageBody message={m} onLoadImages={onLoadImages} trailing={unsubscribe} leading={leading} />
    </>
  );
}

/** "Marco", "Marco and you", "Priya, Marco and you", "Priya, Marco, Ana +2". */
function joinNames(names: string[]): string {
  if (names.length <= 1) return names[0] ?? "";
  if (names.length <= 3) return `${names.slice(0, -1).join(", ")} and ${names[names.length - 1]}`;
  return `${names.slice(0, 3).join(", ")} +${names.length - 3}`;
}

/** The context panel is on screen: turned on, and the window wide enough (app.css hides it at 1280px and under). */
function usePanelShown(): boolean {
  const on = useUi((s) => s.contextPanel);
  const query = "(min-width: 1281px)";
  const [wide, setWide] = useState(() => typeof window === "undefined" || !window.matchMedia || window.matchMedia(query).matches);
  useEffect(() => {
    if (!window.matchMedia) return;
    const mq = window.matchMedia(query);
    const on = () => setWide(mq.matches);
    mq.addEventListener("change", on);
    return () => mq.removeEventListener("change", on);
  }, []);
  return on && wide;
}

/** Has files a reader would look for (inline images don't count). */
function hasFiles(m: MessageView): boolean {
  return m.attachments.some((a) => !a.inline);
}

function CollapsedRow({ m, onClick, refCb, flash }: { m: MessageView; onClick: () => void; refCb: (el: HTMLElement | null) => void; flash: boolean }) {
  const headerAvatars = useAvatarPlacement().message;
  const files = m.attachments.filter((a) => !a.inline);
  return (
    <button
      ref={refCb}
      className={"collapsed" + (m.unread ? " unread" : "") + (flash ? " is-flash" : "")}
      onClick={onClick}
      onContextMenu={(e) => showContextMenu(e, messageMenu(m, { expanded: false, onToggle: onClick }))}
    >
      {headerAvatars && <Avatar person={m.from} size="sm" tone={isMe(m.from.email) ? "gray" : undefined} photo={!isMe(m.from.email)} authenticated={m.senderAuthenticated ?? null} />}
      <span className="c-name truncate">{nameOf(m.from)}</span>
      <span className="c-snip truncate">{m.snippet}</span>
      <span className="c-end">
        {files.length > 0 && (
          <span className="c-files" title={files.map((a) => a.filename).join("\n")}>
            <Icon name="clip" size="xs" />
            <span className="truncate">{files[0].filename}</span>
            {files.length > 1 && <span className="c-files-more">+{files.length - 1}</span>}
          </span>
        )}
        <span className="c-date tnum">{shortDate(m.date)}</span>
      </span>
    </button>
  );
}

function AttachmentCard({ m, a }: { m: MessageView; a: AttachmentMeta }) {
  // Any download of this file (Download, preview, Open, Quick Look, drag
  // out): a ring on the button and how far along in the size line.
  const progress = useAttachmentProgress(m, a);
  const counted = progressText(progress, true);
  return (
    <div
      role="button"
      tabIndex={0}
      className={"file file-btn " + fileTone(a.filename, a.mimeType)}
      title={`Preview ${a.filename}`}
      onClick={() => openAttachment(m, a)}
      onContextMenu={(e) => showContextMenu(e, attachmentMenu(m, a), { label: a.filename })}
      // Drag the file out to Finder, another app or a composer in any window
      // (Mac app only). WebKit's own drag is cancelled at dragstart; the card
      // and its text can't be selected or dragged as text (.file-btn CSS).
      draggable={canDragFiles}
      onDragStart={(e) => dragOut(e, attachmentDragSource(m, a))}
      onKeyDown={(e) => {
        const act = cardKeyAction(e, { copy: nativeFiles });
        if (!act) return;
        // Claim the key so list shortcuts (Enter = open thread, ⌘C) don't fire.
        e.preventDefault();
        e.stopPropagation();
        if (act === "preview") openAttachment(m, a);
        else if (act === "copy") void copyAttachmentFile(m, a);
      }}
    >
      <span className="file-ico">{fileExt(a.filename)}</span>
      <div className="grow" style={{ minWidth: 0 }}>
        <div className="fname truncate">{a.filename}</div>
        <div className="fmeta tnum truncate">{progress ? counted || "Downloading…" : bytes(a.size)}</div>
      </div>
      <button
        className={"btn btn-ghost btn-sm btn-icon file-dl" + (progress ? " is-busy" : "")}
        title={progress ? `Downloading ${a.filename}${counted ? ` · ${progressText(progress)}` : ""}` : "Download"}
        aria-disabled={!!progress}
        onClick={(e) => {
          e.stopPropagation();
          if (!progress) void downloadAttachment(m, a);
        }}
        onKeyDown={(e) => e.stopPropagation()}
      >
        {progress ? <ProgressRing value={fractionOf(progress)} label={`Downloading ${a.filename}`} /> : <Icon name="download" size="xs" />}
      </button>
    </div>
  );
}

function AttachmentList({ m, layout }: { m: MessageView; layout: "grid" | "stack" }) {
  const files = m.attachments.filter((a) => !a.inline);
  if (files.length === 0) return null;
  const total = files.reduce((n, a) => n + a.size, 0);
  if (layout === "stack") {
    return (
      <div className="att-stack">
        {files.map((a) => (
          <AttachmentCard key={a.id} m={m} a={a} />
        ))}
      </div>
    );
  }
  return (
    <div className="attachments">
      <div className="att-head">
        <span className="section-title">
          {files.length} {files.length === 1 ? "attachment" : "attachments"} · {bytes(total)}
        </span>
        <button className="btn btn-ghost btn-sm" onClick={() => files.forEach((a) => void downloadAttachment(m, a))}>
          <Icon name="download" size="xs" />
          Save all
        </button>
      </div>
      <div className="att-grid">
        {files.map((a) => (
          <AttachmentCard key={a.id} m={m} a={a} />
        ))}
      </div>
    </div>
  );
}
