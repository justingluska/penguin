// Saving Settings.smartViews from the sidebar, Settings → Views and search
// (pin a saved search). Each edit applies at once (updateSettings is
// optimistic), so a view turned on is in the sidebar immediately.
import { currentSettings, updateSettings } from "../../lib/settings";
import type { SmartViewSettings } from "../../lib/types";
import { toast } from "../../components/Toast";
import { setUi } from "../../lib/ui";
import { goTo } from "../../app/shortcuts";
import { customKey, isPinned, pinSearch, removeCustom, renameCustom, setCount, setShown, smartDef, moveView, moveViewBy } from "./catalog";
import { leaveIfGone, saveFailed } from "./state";

export function smartSettings(): SmartViewSettings {
  return currentSettings().smartViews;
}

export function saveSmartViews(next: SmartViewSettings): Promise<unknown> {
  if (next === smartSettings()) return Promise.resolve();
  return updateSettings({ smartViews: next }).catch(saveFailed);
}

/** Turn a built-in view on (it joins the sidebar at the end) or off. */
export function toggleSmartView(id: string, on: boolean) {
  void saveSmartViews(setShown(smartSettings(), id, on));
  if (!on) leaveIfGone({ kind: "smart", labelId: id });
}

/** Hide a built-in view from the sidebar menu, with Undo. */
export function hideSmartView(id: string) {
  const before = smartSettings();
  toggleSmartView(id, false);
  toast({
    kind: "action",
    key: `smart-hide-${id}`,
    message: `${smartDef(id)?.label ?? "View"} hidden from the sidebar`,
    detail: "Turn it back on in Settings → Views.",
    action: { label: "Undo", run: () => void saveSmartViews({ ...smartSettings(), shown: before.shown, counts: before.counts }) },
  });
}

export function toggleSmartCount(key: string, on: boolean) {
  void saveSmartViews(setCount(smartSettings(), key, on));
}

export function moveSmartView(key: string, toIndex: number) {
  void saveSmartViews(moveView(smartSettings(), key, toIndex));
}

export function moveSmartViewBy(key: string, delta: -1 | 1) {
  void saveSmartViews(moveViewBy(smartSettings(), key, delta));
}

/** Pin a search to the sidebar as a view; opens it. */
export function pinSearchToSidebar(query: string, open = true): boolean {
  const s = smartSettings();
  if (isPinned(s, query)) {
    toast({ message: "Already in the sidebar" });
    return false;
  }
  const r = pinSearch(s, query);
  if (!r) {
    toast({ kind: "error", message: query.trim() ? "Too many pinned searches" : "Type a search to pin", detail: query.trim() ? "Remove one in Settings → Views first." : undefined });
    return false;
  }
  void saveSmartViews(r.settings);
  const view = { kind: "query" as const, labelId: query.trim().replace(/\s+/g, " ") };
  if (open) goTo(view);
  toast({
    kind: "action",
    key: "smart-pin",
    message: "Pinned to the sidebar",
    detail: "Right-click it to rename, reorder or remove.",
    action: { label: "Undo", run: () => unpinSearch(r.id, false) },
  });
  return true;
}

/** Unpin a saved search (optionally with an Undo toast). */
export function unpinSearch(id: string, withUndo = true) {
  const before = smartSettings();
  const c = before.custom.find((x) => x.id === id);
  if (!c) return;
  void saveSmartViews(removeCustom(before, id));
  leaveIfGone({ kind: "query", labelId: c.query });
  if (withUndo)
    toast({
      kind: "action",
      key: `smart-unpin-${id}`,
      message: `Removed “${c.name}” from the sidebar`,
      action: { label: "Undo", run: () => void saveSmartViews(before) },
    });
}

export function renamePinnedSearch(id: string, name: string) {
  void saveSmartViews(renameCustom(smartSettings(), id, name));
}

/** Open the search overlay with a pinned search's query, to edit or refine it. */
export function editPinnedSearch(query: string) {
  setUi({ overlay: "search", searchPrefill: query });
}

export { customKey };
