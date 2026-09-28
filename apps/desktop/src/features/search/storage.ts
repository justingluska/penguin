// Recent and saved searches. localStorage for now (per device); every access
// is wrapped because storage can be unavailable or throw. Ask questions share
// the recent list: they're typed in the same box, and a question-shaped one
// reopens in Ask by itself.
import { useSyncExternalStore } from "react";
import { toast } from "../../components/Toast";

const RECENT_KEY = "penguin.search.recent";
const SAVED_KEY = "penguin.search.saved";
const MAX_RECENT = 8;

export function readJson<T>(key: string, fallback: T): T {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : fallback;
  } catch {
    return fallback;
  }
}

export function writeJson(key: string, value: unknown): void {
  try {
    localStorage.setItem(key, JSON.stringify(value));
  } catch {
    // Storage full or disabled: recents are a convenience, the search still works.
  }
}

interface Lists {
  recent: string[];
  saved: string[];
}

let lists: Lists = {
  recent: readJson<string[]>(RECENT_KEY, []).filter((s) => typeof s === "string"),
  saved: readJson<string[]>(SAVED_KEY, []).filter((s) => typeof s === "string"),
};
const subs = new Set<() => void>();

function set(next: Lists) {
  lists = next;
  writeJson(RECENT_KEY, next.recent);
  writeJson(SAVED_KEY, next.saved);
  subs.forEach((f) => f());
}

export function useSearchLists(): Lists {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => lists,
  );
}

export function addRecent(q: string) {
  const t = q.trim();
  if (!t) return;
  set({ ...lists, recent: [t, ...lists.recent.filter((r) => r !== t)].slice(0, MAX_RECENT) });
}

export function removeRecent(q: string) {
  set({ ...lists, recent: lists.recent.filter((r) => r !== q) });
}

/** Forget every recent search and question (saved searches stay), with Undo. */
export function clearRecents() {
  const was = lists.recent;
  if (!was.length) return;
  set({ ...lists, recent: [] });
  toast({
    kind: "action",
    message: "Cleared recent searches",
    key: "search-recents-cleared",
    // Anything searched since stays newest.
    action: { label: "Undo", run: () => set({ ...lists, recent: [...new Set([...lists.recent, ...was])].slice(0, MAX_RECENT) }) },
  });
}

export function toggleSaved(q: string): boolean {
  const t = q.trim();
  if (!t) return false;
  const has = lists.saved.includes(t);
  set({ ...lists, saved: has ? lists.saved.filter((s) => s !== t) : [t, ...lists.saved] });
  return !has;
}
