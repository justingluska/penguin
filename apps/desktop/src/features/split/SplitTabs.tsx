// The Split Inbox's tabs across the top of the inbox (the list header, in
// the pane layout and in Floe): each split with its count, then Other. Tab /
// ⇧Tab and 1–9 move between them (./state.ts); right-click edits one.
import { useLayoutEffect, useRef } from "react";
import { num } from "../../lib/format";
import { Icon } from "../../components/Icon";
import { useContextMenu, type MenuEntries } from "../../components/ContextMenu";
import { useKeyTip } from "../../lib/shortcutHints";
import { currentSettings } from "../../lib/settings";
import { meta } from "../../app/store";
import { OTHER, countLabel, type SplitTab } from "../../app/splits";
import { openSettings } from "../settings/state";
import { saveSplits, setSplit, useSplitBar } from "./state";
import { openGetToZero } from "../zero/GetToZero";
import "./split.css";

export function SplitTabs() {
  const tip = useKeyTip();
  const bar = useSplitBar();
  const counts = meta.use((m) => m.splitCounts);
  const more = meta.use((m) => m.splitMore);
  const stripRef = useRef<HTMLDivElement>(null);

  // Keep the current tab in view when there are more than fit.
  useLayoutEffect(() => {
    stripRef.current?.querySelector<HTMLElement>(".tab.active")?.scrollIntoView({ block: "nearest", inline: "nearest" });
  }, [bar.current]);

  const menu = useContextMenu((e): MenuEntries => {
    const id = (e.target as Element).closest<HTMLElement>("[data-split]")?.dataset.split;
    const tab = bar.tabs.find((t) => t.id === id);
    if (!tab) return [];
    return tabMenu(tab);
  });

  if (!bar.on) return null;
  return (
    <div className="split-bar" onContextMenu={menu}>
      <div className="tabs split-tabs" role="tablist" aria-label="Split Inbox" ref={stripRef}>
        {bar.tabs.map((t, i) => {
          const c = counts?.[t.id];
          const label = countLabel(c, more);
          const active = t.id === bar.current;
          const detail = c ? `${num(c.total)}${more ? "+" : ""} ${c.total === 1 ? "conversation" : "conversations"}${c.unread ? `, ${num(c.unread)} unread` : ""}` : "";
          return (
            <button
              key={t.id}
              role="tab"
              data-split={t.id}
              aria-selected={active}
              className={"tab" + (active ? " active" : "") + (c?.unread ? " has-unread" : "")}
              title={tip(detail ? `${t.name}: ${detail}` : t.name, i < 9 ? `${i + 1} · ⇥` : "⇥")}
              onClick={() => setSplit(t.id)}
            >
              {t.name}
              {label && <span className="count">{label}</span>}
            </button>
          );
        })}
      </div>
      <button className="btn btn-ghost btn-icon split-add" title="Add or edit splits" aria-label="Add or edit splits" onClick={() => openSettings("inbox")}>
        <Icon name="plus" size="sm" />
      </button>
    </div>
  );
}

function tabMenu(tab: SplitTab): MenuEntries {
  const splits = currentSettings().inboxSplits;
  const i = splits.findIndex((s) => s.id === tab.id);
  const isSplit = tab.id !== OTHER && i >= 0;
  const move = (d: -1 | 1) => {
    const next = [...splits];
    const [x] = next.splice(i, 1);
    next.splice(i + d, 0, x);
    void saveSplits(next);
  };
  return [
    { type: "header", label: tab.name },
    { label: "Show", icon: "inbox", onSelect: () => setSplit(tab.id) },
    currentSettings().getToZero && {
      label: "Archive older than…",
      icon: "done",
      onSelect: () => {
        setSplit(tab.id);
        openGetToZero();
      },
    },
    { type: "separator" },
    isSplit && {
      label: "Hide when empty",
      checked: splits[i].hideWhenEmpty,
      onSelect: () => void saveSplits(splits.map((s) => (s.id === tab.id ? { ...s, hideWhenEmpty: !s.hideWhenEmpty } : s))),
    },
    isSplit && i > 0 && { label: "Move left", icon: "left", onSelect: () => move(-1) },
    isSplit && i < splits.length - 1 && { label: "Move right", icon: "right", onSelect: () => move(1) },
    { label: isSplit ? "Edit splits…" : "Add a split…", icon: "settings", onSelect: () => openSettings("inbox") },
    isSplit && { type: "separator" },
    isSplit && {
      label: "Remove split",
      icon: "trash",
      danger: true,
      onSelect: () => void saveSplits(splits.filter((s) => s.id !== tab.id)),
    },
  ];
}
