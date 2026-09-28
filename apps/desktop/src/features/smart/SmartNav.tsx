// The sidebar's smart views (Settings → Views): under the mailboxes, in the
// user's order, dragged to reorder. Right-click: count on/off, move, and for
// a pinned search rename / edit / remove. Nothing shows until a view is on.
import { useMemo } from "react";
import type { MailboxView } from "../../lib/types";
import { num } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { showTargetMenu, type MenuEntries } from "../../components/ContextMenu";
import { useDragReorder } from "../../components/useDragReorder";
import { badgeText } from "../sidebar/UnreadBadge";
import { RenameField, renameKey, startRename, useRenaming } from "../sidebar/sidebarMenus";
import { openSettings } from "../settings/state";
import { useSetting } from "../../lib/settings";
import { sidebarKeyOf, smartDef, MAX_VIEW_NAME, type SmartSidebarItem } from "./catalog";
import { openSmartView, useSmartCounts, useSmartItems } from "./state";
import { editPinnedSearch, hideSmartView, moveSmartView, moveSmartViewBy, renamePinnedSearch, toggleSmartCount, unpinSearch } from "./actions";
import "./smart.css";

export function SmartNav({ view, onCalendar, scope }: { view: MailboxView; onCalendar: boolean; scope: string[] | null }) {
  const items = useSmartItems();
  const settings = useSetting("smartViews");
  const counts = useSmartCounts(items, scope);
  const keys = useMemo(() => items.map((i) => i.key), [items]);
  const { drag, listRef, rowRef, onPointerDown, rowStyle } = useDragReorder(keys, (key, to) => moveSmartView(key, to));
  if (items.length === 0) return null;
  const activeKey = onCalendar ? null : sidebarKeyOf(view, settings);
  return (
    <nav className={"nav nav-smart reorder-list" + (drag ? " is-dragging" : "")} ref={listRef} aria-label="Views">
      {items.map((it, i) => (
        <SmartNavRow
          key={it.key}
          item={it}
          index={i}
          total={items.length}
          active={activeKey === it.key}
          count={it.count ? (counts.get(it.key) ?? 0) : null}
          drag={{ ref: rowRef(it.key), onPointerDown: (e) => onPointerDown(it.key, e), style: rowStyle(it.key), lifted: drag?.id === it.key }}
        />
      ))}
      {drag && <div className="reorder-line" style={{ transform: `translateY(${drag.lineY}px)` }} aria-hidden="true" />}
    </nav>
  );
}

function SmartNavRow({
  item: it,
  index,
  total,
  active,
  count,
  drag,
}: {
  item: SmartSidebarItem;
  index: number;
  total: number;
  active: boolean;
  count: number | null;
  drag: { ref: (el: HTMLElement | null) => void; onPointerDown: (e: React.PointerEvent) => void; style: React.CSSProperties | undefined; lifted: boolean };
}) {
  const renaming = useRenaming(renameKey.view(it.custom?.id ?? ""));
  if (renaming && it.custom) {
    const c = it.custom;
    return (
      <div className="nav-item is-renaming">
        <Icon name={it.icon} />
        <RenameField initial={c.name} placeholder={c.query} maxLength={MAX_VIEW_NAME} label={`Name for the search ${c.query}`} onSave={(v) => renamePinnedSearch(c.id, v)} />
      </div>
    );
  }
  const what = it.custom ? "unread conversations" : (smartDef(it.key)?.counts ?? "items");
  return (
    <button
      ref={drag.ref}
      className={"nav-item smart-item" + (active ? " active" : "") + (drag.lifted ? " is-lifted" : "")}
      style={drag.style}
      onPointerDown={drag.onPointerDown}
      onDragStart={(e) => e.preventDefault()}
      onClick={() => openSmartView(it)}
      onContextMenu={(e) => showTargetMenu(e, smartMenu(it, index, total), { label: it.label })}
      title={it.custom ? `Search: ${it.custom.query}` : smartDef(it.key)?.blurb}
      data-view={it.key}
    >
      <Icon name={it.icon} />
      <span className="truncate">{it.label}</span>
      {count !== null && count > 0 && (
        <span className="count badge-items" aria-label={`${num(count)} ${what}`} title={`${num(count)} ${what}`}>
          {badgeText(count)}
        </span>
      )}
    </button>
  );
}

function smartMenu(it: SmartSidebarItem, index: number, total: number): MenuEntries {
  return [
    { label: "Open", icon: it.icon, onSelect: () => openSmartView(it) },
    { label: it.count ? "Hide count" : "Show count", icon: "info", onSelect: () => toggleSmartCount(it.key, !it.count) },
    { type: "separator" },
    { label: "Move up", icon: "up", disabled: index === 0, onSelect: () => moveSmartViewBy(it.key, -1) },
    { label: "Move down", icon: "down", disabled: index === total - 1, onSelect: () => moveSmartViewBy(it.key, 1) },
    { type: "separator" },
    ...(it.custom
      ? ([
          { label: "Rename…", icon: "pencil", onSelect: () => startRename(renameKey.view(it.custom!.id)) },
          { label: "Edit search…", icon: "search", onSelect: () => editPinnedSearch(it.custom!.query) },
          { label: "Remove from sidebar", icon: "x", onSelect: () => unpinSearch(it.custom!.id) },
        ] as MenuEntries)
      : ([{ label: "Hide from sidebar", icon: "eyeoff", onSelect: () => hideSmartView(it.key) }] as MenuEntries)),
    { type: "separator" },
    { label: "Views settings…", icon: "settings", onSelect: () => openSettings("views") },
  ];
}
