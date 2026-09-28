// Settings.smartViews normalized the way src-tauri settings.rs does it
// (normalize_smart_views): valid, deduped, bounded, pinned searches always
// shown, ids that name nothing dropped.
import type { CustomView, SmartViewSettings } from "../types";
import { SMART_VIEWS } from "../types.ts";

export function normalizeSmartViews(v: Partial<SmartViewSettings> | null | undefined): SmartViewSettings {
  const custom: CustomView[] = [];
  for (const c of v?.custom ?? []) {
    const query = String(c?.query ?? "").trim().replace(/\s+/g, " ").slice(0, 500);
    const id = String(c?.id ?? "");
    if (!/^[A-Za-z0-9_-]{1,64}$/.test(id) || !query || custom.some((x) => x.id === id)) continue;
    const name = String(c?.name ?? "").trim().replace(/\s+/g, " ");
    custom.push({ id, name: (name || query).slice(0, 40), query });
    if (custom.length === 30) break;
  }
  const known = (id: string) =>
    id.startsWith("custom:") ? custom.some((c) => `custom:${c.id}` === id) : (SMART_VIEWS as readonly string[]).includes(id);
  const shown: string[] = [];
  for (const id of v?.shown ?? []) if (known(id) && !shown.includes(id)) shown.push(id);
  for (const c of custom) if (!shown.includes(`custom:${c.id}`)) shown.push(`custom:${c.id}`);
  const counts: string[] = [];
  for (const id of v?.counts ?? []) if (known(id) && !counts.includes(id)) counts.push(id);
  return { shown, counts, custom };
}
