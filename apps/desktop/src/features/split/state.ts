// Split Inbox, the live part: the tabs over the inbox, moving between them
// (Tab / ⇧Tab, 1–9), their counts, and adding a sender to a split. The pure
// rules are in app/splits.ts; the lists come from the backend
// (ListQuery.split, penguin-core store_split.rs). Settings → Inbox turns it
// on and edits the splits (./SplitSettings.tsx).
import { registerShortcuts, type Shortcut } from "../../lib/keyboard";
import { currentSettings, subscribeSettings, updateSettings, useSetting } from "../../lib/settings";
import { getUi, setUi, useUi } from "../../lib/ui";
import { asCommandError } from "../../lib/api";
import { toast } from "../../components/Toast";
import { currentSplit, loadSplitCounts, meta } from "../../app/store";
import { OTHER, cycleSplit, effectiveSplitId, splitTabs, visibleTabs, type SplitTab } from "../../app/splits";
import type { InboxSplit } from "../../lib/types";

/** The shown tabs for the current settings and counts. */
export function shownTabs(): SplitTab[] {
  const s = currentSettings();
  const cur = effectiveSplitId(s.inboxSplits, getUi().split);
  return visibleTabs(splitTabs(s.inboxSplits), meta.get().splitCounts, cur);
}

export interface SplitBar {
  on: boolean;
  tabs: SplitTab[];
  current: string;
}

/** What the tab bar draws, kept current. */
export function useSplitBar(): SplitBar {
  const on = useSetting("inboxTabs");
  const splits = useSetting("inboxSplits");
  const view = useUi((s) => s.view.kind);
  const picked = useUi((s) => s.split);
  const counts = meta.use((m) => m.splitCounts);
  const current = effectiveSplitId(splits, picked);
  return { on: on && view === "inbox", tabs: visibleTabs(splitTabs(splits), counts, current), current };
}

/** Show a split: the inbox at that tab, the list (not a thread). */
export function setSplit(id: string) {
  setUi({ view: { kind: "inbox" }, split: id, threadOpen: false, surface: "mail" });
}

/** Tab / ⇧Tab. */
export function cycle(dir: 1 | -1) {
  const cur = currentSplit();
  if (cur === null) return;
  setSplit(cycleSplit(shownTabs(), cur, dir));
}

const noOverlay = () => {
  const o = getUi().overlay;
  return o === null || o === "command";
};
/** The inbox list is what's showing (not a thread, not the calendar), with the splits on. */
const inSplitList = () => noOverlay() && !getUi().threadOpen && getUi().surface === "mail" && currentSplit() !== null;

/**
 * Keys while the Split Inbox is on: Tab / ⇧Tab cycle, 1–9 pick a tab. Only
 * over the inbox list: in a thread Tab stays free (reply suggestions), and
 * other views have no tabs.
 */
export function registerSplitShortcuts(): () => void {
  const keys: Shortcut[] = [
    { id: "split.next", keys: "tab", label: "Next split", group: "Split Inbox", when: inSplitList, run: () => cycle(1) },
    { id: "split.prev", keys: "shift+tab", label: "Previous split", group: "Split Inbox", when: inSplitList, run: () => cycle(-1) },
  ];
  for (let i = 0; i < 9; i++) {
    keys.push({
      id: `split.go.${i + 1}`,
      keys: String(i + 1),
      label: i === 0 ? "Go to split 1–9" : `Go to split ${i + 1}`,
      group: "Split Inbox",
      // Listed once in the sheet as "1–9" (ShortcutSheet reads the first), reached by name in ⌘K.
      hidden: i > 0,
      when: () => inSplitList() && shownTabs().length > i,
      run: () => {
        const t = shownTabs()[i];
        if (t) setSplit(t.id);
      },
    });
  }
  return registerShortcuts(keys);
}

/**
 * Turning the Split Inbox on lands on the first tab; editing the splits
 * recounts. Registered once by App.
 */
export function installSplits(): () => void {
  let on = currentSettings().inboxTabs;
  let splits = currentSettings().inboxSplits;
  let offKeys = on ? registerSplitShortcuts() : null;
  const off = subscribeSettings(() => {
    const s = currentSettings();
    if (s.inboxTabs !== on) {
      on = s.inboxTabs;
      offKeys?.();
      offKeys = on ? registerSplitShortcuts() : null;
      if (on) {
        setUi({ split: null });
        void loadSplitCounts();
      }
    }
    if (s.inboxSplits !== splits) {
      splits = s.inboxSplits;
      if (on) void loadSplitCounts();
    }
  });
  return () => {
    off();
    offKeys?.();
  };
}

// ---------------------------------------------------------------------------
// Editing the splits (Settings → Inbox, ⌘K "Add sender to a split")
// ---------------------------------------------------------------------------

export function saveSplits(next: InboxSplit[]): Promise<unknown> {
  return updateSettings({ inboxSplits: next }).catch((e) => toast({ tone: "error", message: `Couldn't save the splits: ${asCommandError(e).message}` }));
}

/** Add `term` (e.g. from:ana@x.example) to a split's query as another OR. */
export function addToSplit(id: string, term: string) {
  const s = currentSettings();
  const split = s.inboxSplits.find((x) => x.id === id);
  if (!split) return;
  const has = split.query.split(/\s+OR\s+/).some((p) => p.trim().toLowerCase() === term.toLowerCase());
  if (has) {
    toast({ message: `Already in ${split.name}` });
    return;
  }
  // A plain OR list grows; anything else is grouped first (OR binds tighter than AND).
  const q = split.query.trim();
  const pureOr = q.split(/\s+OR\s+/).every((p) => !/\s/.test(p.trim()));
  const query = pureOr ? `${q} OR ${term}` : `(${q}) OR ${term}`;
  void saveSplits(s.inboxSplits.map((x) => (x.id === id ? { ...x, query } : x))).then(() => {
    if (!s.inboxTabs) void updateSettings({ inboxTabs: true });
    toast({ message: `Added to ${split.name}`, action: { label: "Show", run: () => setSplit(id) } });
  });
}

export { OTHER };
