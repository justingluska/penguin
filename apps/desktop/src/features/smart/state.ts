// Smart views' live data outside the list: the sidebar counts (only for
// views whose count is on) and the header figures of the open view. Both are
// local reads, re-read (coalesced) on mail-changed, when the fact scanner has
// had time to catch up, and every 15 minutes (dates move: "upcoming", "this
// month"). The list itself is the normal thread list (app/store.ts).
import { useEffect, useMemo, useState, useSyncExternalStore } from "react";
import { api, onMailChanged } from "../../lib/api";
import type { MailboxView, SmartViewInfo } from "../../lib/types";
import { currentSettings, subscribeSettings } from "../../lib/settings";
import { getUi, setUi } from "../../lib/ui";
import { goTo } from "../../app/shortcuts";
import { toast } from "../../components/Toast";
import { sidebarSmartItems, type SmartSidebarItem } from "./catalog";

// A counter bumped (coalesced) whenever what the views show may have changed.
let bump = 0;
const subs = new Set<() => void>();
let started = false;
let timer: ReturnType<typeof setTimeout> | null = null;

function changedSoon() {
  if (timer) clearTimeout(timer);
  timer = setTimeout(() => {
    timer = null;
    bump++;
    subs.forEach((f) => f());
  }, 400);
}

function start() {
  if (started) return;
  started = true;
  void onMailChanged(changedSoon);
  setInterval(changedSoon, 15 * 60_000);
}

function useBump(): number {
  useEffect(start, []);
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => bump,
  );
}

/** The sidebar's views, following Settings.smartViews. */
export function useSmartItems(): SmartSidebarItem[] {
  const s = useSyncExternalStore(subscribeSettings, () => currentSettings().smartViews);
  return useMemo(() => sidebarSmartItems(s), [s]);
}

/**
 * Counts for the sidebar rows whose count is on (key → count). `scope`: the
 * accounts the views show (null = every account).
 */
export function useSmartCounts(items: SmartSidebarItem[], scope: string[] | null): Map<string, number> {
  const b = useBump();
  const wanted = items.filter((i) => i.count);
  const sig = JSON.stringify([wanted.map((i) => i.countId), scope]);
  const [counts, setCounts] = useState<{ sig: string; map: Map<string, number> }>({ sig: "", map: new Map() });
  useEffect(() => {
    if (wanted.length === 0) return;
    let live = true;
    api
      .smartCounts(wanted.map((i) => i.countId), scope)
      .then((list) => {
        if (!live) return;
        const byId = new Map(list.map((c) => [c.view, c.count]));
        setCounts({ sig, map: new Map(wanted.map((i) => [i.key, byId.get(i.countId) ?? 0])) });
      })
      .catch(() => {
        // Keep the last counts; the next change retries.
      });
    return () => {
      live = false;
    };
    // `sig` stands for `wanted` and `scope`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sig, b]);
  return counts.sig === sig ? counts.map : EMPTY;
}
const EMPTY = new Map<string, number>();

/** The header figures of an open smart view (null while loading, or for a view without any). */
export function useSmartInfo(view: MailboxView, scope: string[] | null): SmartViewInfo | null {
  const b = useBump();
  const id = view.kind === "smart" ? view.labelId.split(":")[0] : null;
  const sig = JSON.stringify([id, scope]);
  const [info, setInfo] = useState<{ sig: string; info: SmartViewInfo } | null>(null);
  useEffect(() => {
    if (!id || id === "files" || id === "newsletters" || id === "people") return;
    let live = true;
    api
      .smartViewInfo(id, scope)
      .then((i) => live && setInfo({ sig, info: i }))
      .catch(() => {});
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sig, b]);
  return info && info.sig === sig ? info.info : null;
}

/** Open a view from the sidebar, ⌘K or a toast. */
export function openSmartView(item: Pick<SmartSidebarItem, "view">) {
  goTo(item.view);
}

/**
 * The open view was turned off or unpinned: go back to the inbox rather
 * than leave a list with no sidebar row.
 */
export function leaveIfGone(removed: MailboxView) {
  const v = getUi().view;
  if (v.kind === removed.kind && "labelId" in v && "labelId" in removed && v.labelId.split(":")[0] === removed.labelId.split(":")[0]) {
    setUi({ view: { kind: "inbox" } });
  }
}

export function saveFailed(e: unknown) {
  toast({ kind: "error", message: "Couldn't save the views", detail: String((e as { message?: string })?.message ?? e) });
}
