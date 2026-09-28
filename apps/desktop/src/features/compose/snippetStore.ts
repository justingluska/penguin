// The composer's snippets live in app settings (Settings → Compose).
import { currentSettings, getSettings, updateSettings, useSetting } from "../../lib/settings";
import type { Snippet } from "../../lib/types";

export function useSnippets(): Snippet[] {
  return useSetting("snippets");
}

export function saveSnippets(next: Snippet[]): Promise<unknown> {
  return updateSettings({ snippets: next });
}

/** Bump the use count (frequent snippets sort first). */
export function recordSnippetUse(id: string) {
  const list = currentSettings().snippets;
  if (!list.some((s) => s.id === id)) return;
  saveSnippets(list.map((s) => (s.id === id ? { ...s, uses: s.uses + 1 } : s))).catch((e) =>
    // Only the count is lost; the snippet was inserted.
    console.warn("penguin: could not record snippet use", e),
  );
}

// ---------------------------------------------------------------------------
// One-time move of snippets that earlier builds kept per device in
// localStorage ("penguin.snippets" used {key,…}; ".v2" already the new shape).
// ---------------------------------------------------------------------------

const LEGACY_KEYS = ["penguin.snippets", "penguin.snippets.v2"];

function readLegacy(): Snippet[] {
  const out: Snippet[] = [];
  for (const k of LEGACY_KEYS) {
    let raw: string | null = null;
    try {
      raw = localStorage.getItem(k);
    } catch {
      return out; // storage unavailable: nothing to migrate
    }
    if (!raw) continue;
    try {
      const list = JSON.parse(raw) as Array<Partial<Snippet> & { key?: string }>;
      for (const s of Array.isArray(list) ? list : []) {
        const trigger = String(s.trigger ?? s.key ?? "").toLowerCase();
        if (!trigger || typeof s.body !== "string") continue;
        out.push({
          id: String(s.id ?? s.key ?? trigger),
          trigger,
          title: String(s.title ?? trigger),
          body: s.body,
          uses: Number(s.uses) || 0,
          subject: "",
          cc: [],
          bcc: [],
          attachments: [],
        });
      }
    } catch {
      // A corrupt legacy entry is skipped; the settings list stands.
    }
  }
  return out;
}

async function migrateLegacy() {
  const legacy = readLegacy();
  if (legacy.length === 0) return;
  const settings = await getSettings();
  const have = new Set(settings.snippets.map((s) => s.trigger));
  const ids = new Set(settings.snippets.map((s) => s.id));
  const added = legacy.filter((s) => !have.has(s.trigger) && !ids.has(s.id));
  if (added.length) await saveSnippets([...settings.snippets, ...added]);
  try {
    LEGACY_KEYS.forEach((k) => localStorage.removeItem(k));
  } catch {
    // Removal can't fail if the reads above worked; nothing else to do.
  }
}

if (typeof window !== "undefined") {
  migrateLegacy().catch((e) => console.warn("penguin: snippet migration failed; will retry next launch", e));
}
