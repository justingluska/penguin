// The message list pane: title (+ split-inbox tabs when enabled in Settings)
// and the list's tools (Unread filter, Floe, refresh), then a virtualized
// list of rows with day group headers. Paging uses the `before` cursor as you
// scroll. A narrow pane (the default) uses three-line "stacked" rows; a wide
// one the single-line row from the mockups; Floe its own two-line rows.
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState, type ReactNode } from "react";
import { useVirtualizer } from "@tanstack/react-virtual";
import type { MailboxView, ThreadSummary } from "../../lib/types";
import { getUi, sameThread, setUi, subscribeUi, useUi } from "../../lib/ui";
import { dayGroup } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { accountInScope, list, listScope, loadMore, meta, selectThread, toggleUnreadOnly } from "../../app/store";
import { archive, openCompose, trash } from "../../app/actions";
import { openSelected } from "../../app/shortcuts";
import { openDraftForThread } from "../compose";
import { useSetting } from "../../lib/settings";
import { listRowHeight, useListStyle } from "../../lib/listStyle";
import { getLayout, toggleSidebar, useLayout } from "../../lib/layout";
import { isMac } from "../../lib/keyboard";
import { ThreadRow, type RowHandlers } from "./ThreadRow";
import { useRowSwipe } from "./useRowSwipe";
import { SelectionBar } from "./SelectionBar";
import { useHasSelection } from "../../app/selection";
import { startRefresh, useRefresh } from "../../app/refresh";
import { ZeroInbox } from "./ZeroInbox";
import { openSnooze } from "../snooze/SnoozePicker";
import { wakeGroup } from "../snooze/presets";
import { Keys } from "../../components/Kbd";
import { FloeCount, FloeKeys, FloeMe, FloeTools } from "../floe/FloeTools";
import { toggleFloe, useFloe } from "../floe/state";
import { useKeyTip } from "../../lib/shortcutHints";
import { dismissFollowUps, toggleReplyLater } from "../triage/actions";
import { startReplyToAll } from "../triage/focus";
import { waitGroup } from "../triage/format";
import { initialMotion, nextExpiry, planMotion, pruneMotion, type ListMotion } from "./listMotion";
import { exitOf, rowKey } from "./rowExit";
import { SmartHeader } from "../smart/SmartParts";
import { smartDef, smartTitle } from "../smart/catalog";
import { currentSettings } from "../../lib/settings";
import { SplitTabs } from "../split/SplitTabs";
import { useSplitBar } from "../split/state";
import { openGetToZero } from "../zero/GetToZero";

const GROUP_H = 34;
const PAD_START = 6;
/** Below this list width rows stack; at or above it they fit on one line. */
const STACK_BELOW = 560;
/** Floe mode's roomier two-line rows (sender · time / subject — snippet). */
const FLOE_H = { compact: 64, comfortable: 72 } as const;

type Item = { kind: "group"; label: string } | { kind: "row"; t: ThreadSummary };

export function viewTitle(view: MailboxView): string {
  switch (view.kind) {
    case "inbox": return "Inbox";
    case "starred": return "Starred";
    case "sent": return "Sent";
    case "drafts": return "Drafts";
    case "done": return "Done";
    case "trash": return "Trash";
    case "spam": return "Spam";
    case "all": return "All mail";
    case "snoozed": return "Snoozed";
    case "replyLater": return "Reply Later";
    case "followUp": return "Follow up";
    case "label": {
      const l = meta.get().labels.find((x) => x.id === view.labelId && accountInScope(x.accountId));
      return l?.name ?? "Label";
    }
    case "smart":
    case "query":
      return smartTitle(view, currentSettings().smartViews);
  }
}

const handlers: RowHandlers = {
  onSelect: (ref) => selectThread(ref),
  onOpen: (ref) => {
    selectThread(ref);
    openSelected();
  },
  onDraft: (ref) => void openDraftForThread(ref),
  onAction: (ref, action) => {
    switch (action) {
      case "done": return void archive([ref]);
      case "trash": return void trash([ref]);
      case "snooze":
        selectThread(ref);
        return openSnooze();
      case "reply":
        selectThread(ref);
        return openCompose("reply");
      case "label":
        selectThread(ref);
        return setUi({ overlay: "label" });
      case "move":
        selectThread(ref);
        return setUi({ overlay: "move" });
      case "replyLater":
        return void toggleReplyLater([ref]);
      case "dismiss":
        return void dismissFollowUps([ref]);
    }
  },
};

export const ThreadList = memo(function ThreadList({ floe = false }: { floe?: boolean }) {
  const tip = useKeyTip();
  const view = useUi((s) => s.view);
  const splits = useSplitBar().on;
  const sidebarCollapsed = useLayout((s) => s.sidebarCollapsed);
  // The header follows the mode itself (its Floe tools slide in at once);
  // `floe` is the row style, which the shell switches once per morph.
  const inFloe = useFloe();
  const ref = useRef<HTMLElement>(null);
  const selecting = useHasSelection();
  const [stacked, setStacked] = useState(() => getLayout().listW < STACK_BELOW);
  // Row layout follows the pane's real width (window resizes shrink it too);
  // only crossing the threshold re-renders.
  useEffect(() => {
    const el = ref.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => setStacked(e.contentRect.width < STACK_BELOW));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);
  return (
    <section
      className={"list" + (floe ? " floe-list" : stacked ? " is-stacked" : "") + (selecting ? " has-selection" : "")}
      ref={ref}
    >
      {/* One header in both layouts, so the title stays put when Floe comes and goes. */}
      <header className={"pane-head list-head" + (splits ? " has-splits" : "")} data-tauri-drag-region>
        <button
          className="btn btn-ghost btn-icon sb-toggle"
          title={tip(`${sidebarCollapsed ? "Show" : "Hide"} sidebar`, `${isMac ? "⌘" : "Ctrl+"}\\ or [`)}
          inert={inFloe}
          aria-hidden={inFloe}
          onClick={toggleSidebar}
        >
          <Icon name="sidebar" size="sm" />
        </button>
        <h1 className="page-title truncate">
          {viewTitle(view)}
          {inFloe && <FloeCount />}
        </h1>
        <div className="list-tools">
          {view.kind === "replyLater" && <ReplyToAllButton />}
          {view.kind === "inbox" && <GetToZeroButton />}
          <FloeTools on={inFloe} />
          <UnreadButton />
          <FloeButton on={inFloe} />
          <RefreshButton />
          <FloeMe on={inFloe} />
        </div>
      </header>
      <SplitTabs />
      {(view.kind === "smart" || view.kind === "query") && <SmartHeaderHere view={view} />}
      <ListBody stacked={!floe && stacked} floe={floe} />
      {floe && inFloe && <FloeKeys />}
      <SelectionBar />
    </section>
  );
});

/** A smart view's header, over the accounts the list shows. */
function SmartHeaderHere({ view }: { view: MailboxView }) {
  // Re-read the scope when the account, profile or hidden accounts change.
  const key = useUi((s) => `${s.accountFilter}|${s.profileId}`);
  useSetting("profiles");
  useSetting("hiddenFromAll");
  // Titles of pinned searches follow renames.
  useSetting("smartViews");
  const filter = useUi((s) => s.accountFilter);
  const scope = filter ? [filter] : listScope();
  return <SmartHeader key={key} view={view} scope={scope} />;
}

/** Reply Later's "Reply to all": each conversation in turn with the composer open (features/triage/focus.ts). */
function ReplyToAllButton() {
  const n = list.use((l) => l.items.length);
  if (n === 0) return null;
  return (
    // Icon-only, like the other list tools, so the title keeps its room in a narrow pane.
    <button
      className="btn btn-ghost btn-icon reply-all"
      title="Reply to all: each conversation in turn, composer open"
      aria-label="Reply to all"
      onClick={startReplyToAll}
    >
      <Icon name="replyall" size="sm" />
    </button>
  );
}

/** The list's Unread filter: an icon, or a labelled pill while it's on. */
function UnreadButton() {
  const tip = useKeyTip();
  const on = useUi((s) => s.unreadOnly);
  return (
    <button
      className={"btn btn-ghost list-filter" + (on ? " is-on" : " btn-icon")}
      title={tip(on ? "Show all conversations" : "Show only unread", "G then U")}
      aria-label="Only unread"
      aria-pressed={on}
      onClick={toggleUnreadOnly}
    >
      <Icon name="unread" size="sm" />
      {on && <span>Unread</span>}
    </button>
  );
}

/** Into Floe (expand) and back out (shrink); Esc leaves too. */
function FloeButton({ on }: { on: boolean }) {
  const tip = useKeyTip();
  const keys = `${isMac ? "⌘⇧F" : "Ctrl+Shift+F"} or \\`;
  return (
    <button
      className="btn btn-ghost btn-icon"
      title={on ? tip("Leave Floe mode", `Esc, ${keys}`) : tip("Floe mode: one calm column", keys)}
      aria-label={on ? "Leave Floe mode" : "Floe mode"}
      aria-pressed={on}
      onClick={toggleFloe}
    >
      <Icon name={on ? "shrink" : "expand"} size="sm" />
    </button>
  );
}

/**
 * Spins for as long as the refresh runs (app/refresh.ts follows each
 * account's poll to its end), then finishes the turn it's on instead of
 * snapping back to the start: it stops at the next full rotation.
 */
function RefreshButton() {
  const tip = useKeyTip();
  const active = useRefresh((s) => s.active);
  const [spinning, setSpinning] = useState(active);
  if (active && !spinning) setSpinning(true);
  const activeRef = useRef(active);
  activeRef.current = active;
  // Reduced motion: no animation runs, so nothing would ever end the spin.
  if (!active && spinning && reducedMotion()) setSpinning(false);
  return (
    <button
      className={"btn btn-ghost btn-icon" + (spinning ? " is-refreshing" : "")}
      data-shortcut="app.sync"
      title={tip("Check for new mail", "⇧R")}
      aria-busy={active}
      onClick={startRefresh}
      onAnimationIteration={() => {
        if (!activeRef.current) setSpinning(false);
      }}
    >
      <Icon name="refresh" size="sm" />
    </button>
  );
}

/** Get to zero (features/zero): over the inbox or the open split, while it has mail and the setting is on. */
function GetToZeroButton() {
  const on = useSetting("getToZero");
  const n = list.use((l) => l.items.length);
  if (!on || n === 0) return null;
  return (
    <button className="btn btn-ghost btn-icon" title="Get to zero: archive older conversations" aria-label="Get to zero" onClick={openGetToZero}>
      <Icon name="done" size="sm" />
    </button>
  );
}

function ListBody({ stacked, floe = false }: { stacked: boolean; floe?: boolean }) {
  const density = useSetting("density");
  // Stacked and one-line row heights per list style and density (lib/listStyle.ts).
  const listStyle = useListStyle();
  const rowH = floe ? FLOE_H[density] : listRowHeight(listStyle, density, stacked);
  const items = list.use((l) => l.items);
  const listKey = list.use((l) => l.key);
  const loaded = list.use((l) => l.loaded);
  const loading = list.use((l) => l.loading);
  const hasMore = list.use((l) => l.hasMore);
  const error = list.use((l) => l.error);
  const view = useUi((s) => s.view);
  const unreadOnly = useUi((s) => s.unreadOnly);
  const snoozedView = view.kind === "snoozed";
  const followUpView = view.kind === "followUp";
  const smartView = view.kind === "smart";
  const scrollRef = useRef<HTMLDivElement>(null);

  // Flatten into group headers + rows. Day groups follow the mockup.
  const { rows, keys, index } = useMemo(() => {
    const rows: Item[] = [];
    const keys: string[] = [];
    const index = new Map<string, number>();
    const now = Date.now();
    let group = "";
    const push = (item: Item, key: string) => {
      index.set(key, rows.length);
      keys.push(key);
      rows.push(item);
    };
    for (const t of items) {
      // Snoozed: grouped by when they wake (the list is soonest first);
      // Follow up by how long they've waited (oldest first).
      // Smart views by their own sections (a trip, "Overdue", a month).
      const g =
        snoozedView && t.snoozedUntil
          ? wakeGroup(t.snoozedUntil, now)
          : followUpView
            ? waitGroup(t.lastDate, now)
            : smartView && t.smart?.group
              ? t.smart.group
              : dayGroup(t.lastDate, now);
      if (g !== group) {
        group = g;
        push({ kind: "group", label: g }, "g:" + g);
      }
      push({ kind: "row", t }, rowKey(t));
    }
    return { rows, keys, index };
  }, [items, snoozedView, followUpView, smartView]);

  const virtualizer = useVirtualizer({
    count: rows.length + (hasMore ? 1 : 0),
    getScrollElement: () => scrollRef.current,
    estimateSize: (i) => (i >= rows.length ? rowH : rows[i].kind === "group" ? GROUP_H : rowH),
    getItemKey: (i) => keys[i] ?? "more",
    overscan: 12,
    paddingStart: PAD_START,
    paddingEnd: 8,
  });

  // Keep the cursor visible as it moves, without re-rendering the list.
  const indexRef = useRef(index);
  indexRef.current = index;
  const rowsRef = useRef(rows);
  rowsRef.current = rows;
  const scrollToSelected = useCallback(() => {
    const sel = getUi().selected;
    if (!sel) return;
    const idx = indexRef.current.get(rowKey(sel));
    if (idx === undefined) return;
    virtualizer.scrollToIndex(idx, { align: "auto" });
    // Moving up onto the first row of a day group: reveal its header too.
    if (idx > 0 && rowsRef.current[idx - 1]?.kind === "group") {
      const header = virtualizer.measurementsCache[idx - 1];
      if (header && (virtualizer.scrollOffset ?? 0) > header.start) virtualizer.scrollToOffset(header.start);
    }
  }, [virtualizer]);

  // Row height changed (layout, density or list style): re-measure, keep the cursor in view.
  useEffect(() => {
    virtualizer.measure();
    scrollToSelected();
  }, [rowH, virtualizer, scrollToSelected]);

  useEffect(() => {
    let last = getUi().selected;
    return subscribeUi(() => {
      const sel = getUi().selected;
      if (sameThread(sel, last)) return;
      last = sel;
      scrollToSelected();
    });
  }, [scrollToSelected]);

  const vItems = virtualizer.getVirtualItems();

  // Row motion (listMotion.ts): when the rows change, plan ghosts for rows that
  // left the screen, openings for rows that came back or arrived, and a glide
  // for everything in between. Planned during render ("adjusting state while
  // rendering"), so a leaving row's DOM node is never unmounted in between:
  // the store has already changed, this only decides how it looks getting there.
  const rendered = useRef<Map<string, { start: number; end: number }>>(new Map());
  const [st, setSt] = useState<MotionView>(() => ({ motion: initialMotion({ listKey, rows, keys, index }), shift: null, fresh: 0 }));
  let mv = st;
  if (st.motion.rows !== rows || st.motion.listKey !== listKey) {
    const plan = planMotion(st.motion, { listKey, rows, keys, index }, {
      now: performance.now(),
      rendered: new Set(rendered.current.keys()),
      motion: !reducedMotion(),
      exitOf,
    });
    mv = { motion: plan.state, shift: null, fresh: listKey === st.motion.listKey ? st.fresh : 0 };
    // Scrolled down: rows coming or going above the viewport (new mail at the
    // top) don't move what you're looking at; "N new" says what arrived.
    const top = scrollRef.current?.scrollTop ?? 0;
    if (plan.changed && top > 2) {
      let anchor: string | null = null;
      let oldStart = 0;
      for (const [k, r] of rendered.current) {
        if (r.end > top && (anchor === null || r.start < oldStart)) {
          anchor = k;
          oldStart = r.start;
        }
      }
      // The top visible row itself left: the change is on screen, so it animates instead.
      const at = anchor === null ? undefined : index.get(anchor);
      const newStart = at === undefined ? undefined : virtualizer.measurementsCache[at]?.start;
      if (at !== undefined && newStart !== undefined && newStart !== oldStart) {
        const above = plan.added.filter((k) => index.get(k)! < at && k.startsWith("t:")).length;
        mv = { motion: { ...plan.state, flowUntil: 0 }, shift: { delta: newStart - oldStart }, fresh: mv.fresh + above };
      }
    }
    setSt(mv);
  }
  const motion = mv.motion;

  // Apply a scroll anchor before paint.
  useLayoutEffect(() => {
    const el = scrollRef.current;
    if (el && st.shift) el.scrollTop += st.shift.delta;
  }, [st.shift]);

  // Where each rendered row sits, for the next change's plan.
  useLayoutEffect(() => {
    const m = new Map<string, { start: number; end: number }>();
    for (const vi of vItems) if (vi.index < keys.length) m.set(keys[vi.index], { start: vi.start, end: vi.end });
    rendered.current = m;
  });

  // Drop finished ghosts and openings, and end the glide, when their time is up.
  useEffect(() => {
    const at = nextExpiry(st.motion);
    if (at === null) return;
    const t = setTimeout(
      () =>
        setSt((s) => {
          const m = pruneMotion(s.motion, performance.now());
          return m === s.motion ? s : { ...s, motion: m };
        }),
      Math.max(0, at - performance.now()) + 8,
    );
    return () => clearTimeout(t);
  }, [st.motion]);

  // "N new" goes away once you're back at the top.
  const hasFresh = mv.fresh > 0;
  useEffect(() => {
    const el = scrollRef.current;
    if (!el || !hasFresh) return;
    const onScroll = () => {
      if (el.scrollTop <= 2) setSt((s) => (s.fresh ? { ...s, fresh: 0 } : s));
    };
    el.addEventListener("scroll", onScroll, { passive: true });
    return () => el.removeEventListener("scroll", onScroll);
  }, [hasFresh]);

  // Infinite scroll: fetch the next page when the "more" sentinel is in range.
  const lastIndex = vItems.length ? vItems[vItems.length - 1].index : 0;
  useEffect(() => {
    if (hasMore && !loading && lastIndex >= rows.length - 25) void loadMore();
  }, [lastIndex, rows.length, hasMore, loading]);

  const empty = loaded && items.length === 0;
  useRowSwipe(scrollRef, !empty);

  if (empty) {
    if (error) return <EmptyState title="Couldn't load mail" body={error} />;
    if (unreadOnly)
      return (
        <EmptyState
          title={`No unread mail in ${viewTitle(view)}`}
          body="You're caught up here."
          action={{ label: "Show all conversations", keys: "g u", run: toggleUnreadOnly }}
        />
      );
    if (view.kind === "inbox") return <ZeroInbox />;
    if (snoozedView)
      return (
        <EmptyState
          title="Nothing snoozed"
          body="Snooze a conversation with H and it leaves your inbox until the time you pick, then comes back on top, unread."
        />
      );
    if (view.kind === "replyLater")
      return (
        <EmptyState
          title="No replies waiting on you"
          body="Press Y on a conversation you've read but owe a reply. It leaves your inbox and waits here until you answer; replying from Penguin takes it out."
        />
      );
    if (followUpView)
      return (
        <EmptyState
          title="Nobody owes you a reply"
          body="Mail you sent that gets no answer shows up here after a few days (Settings → General), oldest first, so you can nudge."
        />
      );
    if (view.kind === "smart") {
      const d = smartDef(view.labelId);
      if (d) return <EmptyState title={d.empty.title} body={d.empty.body} />;
    }
    if (view.kind === "query")
      return <EmptyState title="Nothing matches this search" body={`New mail that matches “${view.labelId}” shows up here.`} />;
    return <EmptyState title={`Nothing in ${viewTitle(view)}`} body="Conversations you move here show up in this list." />;
  }

  const measures = virtualizer.measurementsCache;
  // .v-inner is what collapses or opens (a transform, like the glide, so the
  // two stay in step even when the main thread is busy); a swipe's panel lives in it too.
  const renderItem = (r: Item, cls = "v-inner") => (
    <div className={cls}>
      {r.kind === "group" ? <div className="group-label">{r.label}</div> : <ThreadRow t={r.t} handlers={handlers} stacked={stacked} floe={floe} />}
    </div>
  );
  // A ghost sits where its anchor row now starts, in its old order, so DOM
  // order never changes and React keeps every node where it was.
  const ghosts = motion.ghosts
    .map((g) => {
      const a = g.anchor === null ? undefined : index.get(g.anchor);
      const b = g.after === null ? undefined : index.get(g.after);
      const start = a !== undefined && measures[a] ? measures[a].start : b !== undefined && measures[b] ? measures[b].end : PAD_START;
      return { g, start };
    })
    .sort((x, y) => x.start - y.start || x.g.born - y.g.born);
  const out: ReactNode[] = [];
  let gi = 0;
  const pushGhost = ({ g, start }: (typeof ghosts)[number]) =>
    out.push(
      <div
        key={g.key}
        className={"v-row is-exiting x-" + g.exit}
        aria-hidden="true"
        inert
        style={{ height: g.item.kind === "group" ? GROUP_H : rowH, transform: `translateY(${start}px)` }}
      >
        {renderItem(g.item, g.exit === "swipe" ? "v-inner is-swiping" : "v-inner")}
      </div>,
    );
  for (const vi of vItems) {
    while (gi < ghosts.length && ghosts[gi].start <= vi.start) pushGhost(ghosts[gi++]);
    const r = rows[vi.index];
    const key = String(vi.key);
    out.push(
      <div
        key={vi.key}
        className={"v-row" + (motion.entering.has(key) ? " is-entering" : "") + (motion.noGlide.has(key) ? " no-glide" : "")}
        style={{ height: vi.size, transform: `translateY(${vi.start}px)` }}
      >
        {!r ? <div className="list-more faint">Loading more…</div> : renderItem(r)}
      </div>,
    );
  }
  while (gi < ghosts.length) pushGhost(ghosts[gi++]);
  const flowing = performance.now() < motion.flowUntil;

  return (
    <div className="list-scroll v-scroll" ref={scrollRef} role="listbox" aria-label="Conversations">
      {hasFresh && (
        <div className="fresh-slot">
          <button
            className="fresh-pill"
            onClick={() => {
              setSt((s) => ({ ...s, fresh: 0 }));
              scrollRef.current?.scrollTo({ top: 0, behavior: reducedMotion() ? "auto" : "smooth" });
            }}
          >
            <Icon name="arrowup" size="xs" />
            {mv.fresh} new
          </button>
        </div>
      )}
      <div className={"v-list" + (flowing ? " is-flowing" : "")} style={{ height: virtualizer.getTotalSize() }}>
        {out}
      </div>
      {!loaded && <ListSkeleton />}
    </div>
  );
}

interface MotionView {
  motion: ListMotion<Item>;
  /** A scroll correction to apply before paint (rows changed above the viewport). */
  shift: { delta: number } | null;
  /** New conversations that arrived above the viewport ("N new"). */
  fresh: number;
}

const reducedMotion = () => window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;

function ListSkeleton() {
  return (
    <div className="list-skeleton" aria-hidden="true">
      {Array.from({ length: 12 }, (_, i) => (
        <div key={i} className="sk-row">
          <span className="sk" style={{ width: 120 }} />
          <span className="sk" style={{ width: `${40 + ((i * 37) % 45)}%` }} />
        </div>
      ))}
    </div>
  );
}

function EmptyState({ title, body, action }: { title: string; body: string; action?: { label: string; keys: string; run: () => void } }) {
  return (
    <div className="empty">
      <div className="empty-title">{title}</div>
      <div className="empty-body">{body}</div>
      {action && (
        <button className="btn btn-secondary empty-action" onClick={action.run}>
          {action.label}
          <Keys keys={action.keys} />
        </button>
      )}
    </div>
  );
}
