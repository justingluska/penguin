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
import { archive, isUnread, markRead, openCompose, toggleRead, toggleStar } from "../../app/actions";
import { threadNav } from "../../app/shortcuts";
import { ContextPanel } from "./ContextPanel";
import { InviteCard } from "../calendar/InviteCard";
import { openDraftForThread } from "../compose";
import { InstantDockRow } from "../compose/InstantReplies";
import { downloadAttachment } from "./AttachmentPreview";
import { openAttachment } from "./openAttachment";
import { openMessageDetails } from "./MessageDetails";
import { PersonLink } from "../people/PersonCard";
import { useKeyTip } from "../../lib/shortcutHints";
import { useAvatarPlacement } from "../../lib/avatars";
import { showContextMenu } from "../../components/ContextMenu";
import { attachmentMenu, messageMenu } from "./threadMenus";
import { attachmentDragSource, canDragFiles, dragOut } from "../image-viewer/fileDrag";
import { COPY_CONVERSATION_KEYS, copyConversation } from "./copy";
import { OtpBanner } from "../otp/OtpBanner";
import { inReplyLater, toggleReplyLater } from "../triage/actions";
import { useUnsubscribeSlot } from "../unsubscribe/UnsubscribeButton";
import { SummarizeButton, SummaryCard } from "../summary/SummaryCard";
import { useSummaryShortcut } from "../summary/state";
import { openThreadWindow } from "../../app/windows";
import { isMainWindow } from "../../lib/windowBus";

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
      if (cur && cur.accountId + "\u0000" + cur.threadId === key && isUnread(cur) && opensAsRead(cur)) markRead([cur]);
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
        data-shortcut-label={view === "followUp" ? "Dismiss" : "Mark done"}
        title={tip(view === "followUp" ? "Dismiss" : "Done", "E")}
        onClick={() => void archive()}
      >
        <Icon name="done" size="xs" />
        <span className="btn-label">{view === "followUp" ? "Dismiss" : "Done"}</span>
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

  // New messages arriving in an open thread: expand the newest one.
  const lastId = msgs[last]?.id;
  useEffect(() => {
    if (lastId) setExpanded((s) => (s.has(lastId) ? s : new Set(s).add(lastId)));
  }, [lastId]);

  const reveal = useCallback((id: string, highlight: boolean) => {
    setShowAll(true);
    setExpanded((s) => (s.has(id) ? s : new Set(s).add(id)));
    requestAnimationFrame(() => {
      itemRefs.current.get(id)?.scrollIntoView({ block: "start", behavior: "auto" });
      if (highlight) {
        setFlash(id);
        setTimeout(() => setFlash((f) => (f === id ? null : f)), 1600);
      }
    });
  }, []);

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
      setShowAll(true);
      setExpanded((s) => (s.size === msgs.length ? new Set([msgs[last].id]) : new Set(msgs.map((m) => m.id))));
    };
    return () => {
      threadNav.next = () => {};
      threadNav.prev = () => {};
      threadNav.expandAll = () => {};
    };
  }, [variant, cursor, last, msgs, reveal]);

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

  // Which messages show as rows vs. hidden behind a divider.
  type Slot = { kind: "msg"; m: MessageView; i: number } | { kind: "more"; n: number; names: string };
  const slots: Slot[] = [];
  if (variant === "preview") {
    if (!showAll && last > 0) {
      const earlier = msgs.slice(0, last);
      const names = [...new Set(earlier.map((m) => (isMe(m.from.email) ? "you" : firstName(m.from))))].slice(0, 3).join(", ");
      slots.push({ kind: "more", n: earlier.length, names });
      slots.push({ kind: "msg", m: msgs[last], i: last });
    } else msgs.forEach((m, i) => slots.push({ kind: "msg", m, i }));
  } else {
    const hideFrom = 1;
    const hideTo = last - 2; // keep the first, the two before the latest, the latest
    msgs.forEach((m, i) => {
      const hidden = !showAll && msgs.length >= 4 && i >= hideFrom && i <= hideTo && !expanded.has(m.id);
      if (hidden) {
        if (i === hideFrom) slots.push({ kind: "more", n: hideTo - hideFrom + 1, names: "" });
        return;
      }
      slots.push({ kind: "msg", m, i });
    });
  }

  const setItemRef = (id: string) => (el: HTMLElement | null) => {
    if (el) itemRefs.current.set(id, el);
    else itemRefs.current.delete(id);
  };

  const lastMsg = msgs[last];
  const replyTo = lastMsg && (isMe(lastMsg.from.email) ? lastMsg.to[0] ?? lastMsg.from : lastMsg.from);

  if (variant === "preview") {
    return (
      <>
        <div className="preview-body v-scroll" ref={scrollRef}>
          <h2 className="subject">{thread.subject || "(no subject)"}</h2>
          {header}
          <InviteCard thread={thread} />
          <SummaryCard thread={thread} onJump={jumpTo} />
          <div className="preview-stack">
            {slots.map((s) =>
              s.kind === "more" ? (
                <button key="more" className="earlier" onClick={() => setShowAll(true)}>
                  <Icon name="mail" size="xs" />
                  {s.n} earlier {s.n === 1 ? "message" : "messages"}
                  {s.names && ` · ${s.names}`}
                </button>
              ) : expanded.has(s.m.id) ? (
                <div
                  key={s.m.id}
                  ref={setItemRef(s.m.id)}
                  className={"pv-msg" + (flash === s.m.id ? " is-flash" : "")}
                  onContextMenu={(e) => showContextMenu(e, messageMenu(s.m, { expanded: true, onToggle: s.i !== last ? () => toggle(s.m.id) : undefined }))}
                >
                  <div className="sender-row">
                    {headerAvatars && (
                      <Avatar person={s.m.from} size="md" authenticated={s.m.senderAuthenticated ?? null} photo={!isMe(s.m.from.email)} />
                    )}
                    <div className="grow" style={{ minWidth: 0 }}>
                      <div className="row-flex from-line" style={{ gap: 6 }}>
                        <PersonLink person={s.m.from} label={nameOf(s.m.from)} className="emph truncate pl-name" />
                        <PersonLink person={s.m.from} label={s.m.from.email} className="faint truncate small" />
                      </div>
                      <RecipientLine m={s.m} />
                    </div>
                    {s.m.labelIds.includes("DRAFT") ? (
                      <button
                        className="btn btn-secondary btn-sm"
                        onClick={() => void openDraftForThread({ accountId: thread.accountId, threadId: thread.threadId }, s.m.id)}
                      >
                        <Icon name="compose" size="xs" />
                        Edit draft
                      </button>
                    ) : (
                      <button className="m-time faint tnum small" title="Message details" onClick={() => openMessageDetails(s.m)}>
                        {shortDate(s.m.date)}
                      </button>
                    )}
                  </div>
                  <MessageContent m={s.m} onLoadImages={() => onLoadImages(s.m)} />
                  <AttachmentList m={s.m} layout="stack" />
                </div>
              ) : (
                <CollapsedRow key={s.m.id} m={s.m} onClick={() => toggle(s.m.id)} refCb={setItemRef(s.m.id)} flash={flash === s.m.id} />
              ),
            )}
          </div>
        </div>
        <div className="quick-reply">
          <div className="qr-row">
            <button className="qr-box" onClick={() => openCompose("reply")}>
              <span className="placeholder">Reply to {replyTo ? firstName(replyTo) : "sender"}…</span>
              <Kbd>R</Kbd>
            </button>
            <button className="qr-box qr-fwd" title="Forward (F)" onClick={() => openCompose("forward")}>
              <Icon name="forward" size="xs" />
              <span>Forward</span>
            </button>
          </div>
        </div>
      </>
    );
  }

  const others = people.filter((p) => !isMe(p.email));
  return (
    <>
      <div className="thread-scroll v-scroll" ref={scrollRef}>
        <div className="thread-col">
          <h1 className="thread-title">{thread.subject || "(no subject)"}</h1>
          {header}
          <InviteCard thread={thread} />
          <SummaryCard thread={thread} onJump={jumpTo} />
          <div className="stack">
            {slots.map((s) =>
              s.kind === "more" ? (
                <button key="more" className="collapsed-more" onClick={() => setShowAll(true)}>
                  <span className="line-l" />
                  <span>
                    {s.n} more {s.n === 1 ? "message" : "messages"}
                  </span>
                  <span className="line-l" />
                </button>
              ) : expanded.has(s.m.id) ? (
                <article
                  key={s.m.id}
                  ref={setItemRef(s.m.id)}
                  className={"message" + (s.i === cursor ? " is-cursor" : "") + (flash === s.m.id ? " is-flash" : "")}
                  onContextMenu={(e) => showContextMenu(e, messageMenu(s.m, { expanded: true, onToggle: s.i !== last ? () => toggle(s.m.id) : undefined }))}
                >
                  <header className="m-head" onClick={() => s.i !== last && toggle(s.m.id)}>
                    {headerAvatars && (
                      <Avatar person={s.m.from} size="md" authenticated={s.m.senderAuthenticated ?? null} photo={!isMe(s.m.from.email)} />
                    )}
                    <div className="grow" style={{ minWidth: 0 }}>
                      <div className="row-flex from-line" style={{ gap: 8 }}>
                        <PersonLink person={s.m.from} label={nameOf(s.m.from)} className="m-name" />
                        <PersonLink person={s.m.from} label={s.m.from.email} className="faint small truncate" />
                      </div>
                      <RecipientLine m={s.m} style={{ lineHeight: "16px" }} />
                    </div>
                    {s.m.labelIds.includes("DRAFT") && (
                      <button
                        className="btn btn-secondary btn-sm"
                        onClick={(e) => {
                          e.stopPropagation();
                          void openDraftForThread({ accountId: thread.accountId, threadId: thread.threadId }, s.m.id);
                        }}
                      >
                        <Icon name="compose" size="xs" />
                        Edit draft
                      </button>
                    )}
                    <button
                      className="m-time faint tnum small"
                      title="Message details"
                      onClick={(e) => {
                        e.stopPropagation();
                        openMessageDetails(s.m);
                      }}
                    >
                      {messageTime(s.m.date)}
                    </button>
                    <button
                      className="btn btn-ghost btn-sm btn-icon"
                      data-shortcut="compose.reply"
                      title={tip("Reply", "R")}
                      onClick={(e) => {
                        e.stopPropagation();
                        setUi({ overlay: "compose", composeContext: { mode: "reply", thread: { accountId: thread.accountId, threadId: thread.threadId }, messageId: s.m.id } });
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
                        setUi({ overlay: "compose", composeContext: { mode: "forward", thread: { accountId: thread.accountId, threadId: thread.threadId }, messageId: s.m.id } });
                      }}
                    >
                      <Icon name="forward" size="xs" />
                    </button>
                  </header>
                  <div className="m-body">
                    <MessageContent m={s.m} onLoadImages={() => onLoadImages(s.m)} />
                  </div>
                  <AttachmentList m={s.m} layout="grid" />
                </article>
              ) : (
                <CollapsedRow key={s.m.id} m={s.m} onClick={() => toggle(s.m.id)} refCb={setItemRef(s.m.id)} flash={flash === s.m.id} />
              ),
            )}
          </div>
        </div>
      </div>
      <div className="reply-dock">
        <div className="reply reply-prompt">
          {/* The prompt starts the usual reply; the row below picks another. */}
          <button className="reply-open" onClick={() => openCompose(others.length > 1 ? "replyAll" : "reply")}>
          <div className="reply-to">
            <Icon name={others.length > 1 ? "replyall" : "reply"} size="xs" />
            <span className="truncate">
              {others.length > 1 ? "Reply all to " : "Reply to "}
              {others.slice(0, 3).map((p, i) => (
                <span key={p.email}>
                  {i > 0 && ", "}
                  <span className="emph">{displayName(p)}</span>
                </span>
              ))}
            </span>
            <span className="grow" />
            {account && <AccountBadge account={account} label={account.email} />}
          </div>
          <div className="reply-body placeholder-text">Write a reply…</div>
          </button>
          {/* Instant replies (Settings → Compose): a click opens the reply with it. */}
          <InstantDockRow onOpen={() => openCompose(others.length > 1 ? "replyAll" : "reply")} />
          <div className="reply-tools">
            <button className="btn btn-ghost btn-sm" onClick={() => openCompose("reply")}>
              <Icon name="reply" size="xs" />
              Reply
              <Kbd>R</Kbd>
            </button>
            {others.length > 1 && (
              <button className="btn btn-ghost btn-sm" onClick={() => openCompose("replyAll")}>
                <Icon name="replyall" size="xs" />
                Reply all
                <Kbd>A</Kbd>
              </button>
            )}
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

/**
 * The body, or — for mail outside the sync window that's stored headers-only —
 * its snippet and a "Loading full message…" line while get_thread downloads
 * the body; the mail-changed that follows re-renders this with the real body.
 */
function MessageContent({ m, onLoadImages }: { m: MessageView; onLoadImages: () => void }) {
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
      <MessageBody message={m} onLoadImages={onLoadImages} trailing={unsubscribe} />
    </>
  );
}

function CollapsedRow({ m, onClick, refCb, flash }: { m: MessageView; onClick: () => void; refCb: (el: HTMLElement | null) => void; flash: boolean }) {
  const headerAvatars = useAvatarPlacement().message;
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
      <span className="c-date tnum">{shortDate(m.date)}</span>
    </button>
  );
}

function AttachmentCard({ m, a }: { m: MessageView; a: AttachmentMeta }) {
  const [saving, setSaving] = useState(false);
  const download = async () => {
    setSaving(true);
    await downloadAttachment(m, a);
    setSaving(false);
  };
  return (
    <div
      role="button"
      tabIndex={0}
      className={"file file-btn " + fileTone(a.filename, a.mimeType)}
      title={`Preview ${a.filename}`}
      onClick={() => openAttachment(m, a)}
      onContextMenu={(e) => showContextMenu(e, attachmentMenu(m, a), { label: a.filename })}
      // Drag the file out to Finder or another app (Mac app only).
      draggable={canDragFiles}
      onDragStart={(e) => dragOut(e, attachmentDragSource(m, a))}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          // Claim the key so list shortcuts (Enter = open thread) don't fire.
          e.preventDefault();
          openAttachment(m, a);
        }
      }}
    >
      <span className="file-ico">{fileExt(a.filename)}</span>
      <div className="grow" style={{ minWidth: 0 }}>
        <div className="fname truncate">{a.filename}</div>
        <div className="fmeta">{saving ? "Saving…" : bytes(a.size)}</div>
      </div>
      <button
        className="btn btn-ghost btn-sm btn-icon file-dl"
        title="Download"
        disabled={saving}
        onClick={(e) => {
          e.stopPropagation();
          void download();
        }}
        onKeyDown={(e) => e.stopPropagation()}
      >
        <Icon name="download" size="xs" />
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
