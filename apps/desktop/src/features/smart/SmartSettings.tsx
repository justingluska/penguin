// Settings → Views: turn each smart view on (it joins the sidebar at once),
// choose which show a count, put them in order, and manage pinned searches.
// Everything is off by default.
import { useMemo, useState } from "react";
import { Icon } from "../../components/Icon";
import { useDragReorder } from "../../components/useDragReorder";
import { useSetting } from "../../lib/settings";
import { Section, Switch } from "../settings/parts";
import { SMART_VIEW_DEFS, MAX_VIEW_NAME, sidebarSmartItems, type SmartSidebarItem } from "./catalog";
import { moveSmartView, moveSmartViewBy, pinSearchToSidebar, renamePinnedSearch, toggleSmartCount, toggleSmartView, unpinSearch } from "./actions";
import "./smart.css";

export function SmartViewsSection() {
  const s = useSetting("smartViews");
  const items = useMemo(() => sidebarSmartItems(s), [s]);
  return (
    <Section id="views" icon="list" title="Views">
      <p className="st-muted">
        Optional lists in the sidebar, under your mailboxes: receipts, trips, packages, bills and more, each a normal
        conversation list. They are read on this Mac from mail already downloaded; nothing is sent anywhere. All are off
        until you turn them on.
      </p>
      <div className="sv-list">
        {SMART_VIEW_DEFS.map((d) => {
          const on = s.shown.includes(d.id);
          const count = s.counts.includes(d.id);
          return (
            <div className="setting-row setting-tall sv-row" key={d.id} data-setting={`view-${d.id}`}>
              <Icon name={d.icon} size="sm" />
              <div className="sv-text">
                <span className="setting-label">{d.label}</span>
                <p className="sv-blurb">{d.blurb}</p>
              </div>
              <label className={"sv-count" + (on ? "" : " is-off")} title={`Show the number of ${d.counts} in the sidebar`}>
                <input type="checkbox" checked={count} disabled={!on} onChange={(e) => toggleSmartCount(d.id, e.target.checked)} />
                Count
              </label>
              <Switch label={d.label} on={on} onChange={(v) => toggleSmartView(d.id, v)} />
            </div>
          );
        })}
      </div>

      <h3 className="st-sub">Sidebar order</h3>
      {items.length === 0 ? (
        <p className="sv-empty">Turn a view on and it shows up here and in the sidebar. Drag to reorder.</p>
      ) : (
        <OrderList items={items} />
      )}

      <h3 className="st-sub">Pinned searches</h3>
      <p className="st-muted">
        Any search can be a view: save it with ⌘S in search, then choose Pin to sidebar (or type one here). Right-click a
        pinned search in the sidebar to rename or remove it.
      </p>
      <PinForm />
    </Section>
  );
}

function OrderList({ items }: { items: SmartSidebarItem[] }) {
  const keys = useMemo(() => items.map((i) => i.key), [items]);
  const { drag, listRef, rowRef, onPointerDown, rowStyle } = useDragReorder(keys, (key, to) => moveSmartView(key, to));
  return (
    <div className={"sv-order reorder-list" + (drag ? " is-dragging" : "")} ref={listRef} data-setting="views-order" data-reorder-keys>
      {items.map((it, i) => (
        <div
          key={it.key}
          ref={rowRef(it.key)}
          className={"sv-order-row" + (drag?.id === it.key ? " is-lifted" : "")}
          style={rowStyle(it.key)}
          tabIndex={0}
          aria-label={`${it.label}, ${i + 1} of ${items.length}`}
          onKeyDown={(e) => {
            // ⌥↑ / ⌥↓ move, like Settings → Accounts.
            if (!e.altKey || (e.key !== "ArrowUp" && e.key !== "ArrowDown")) return;
            e.preventDefault();
            moveSmartViewBy(it.key, e.key === "ArrowUp" ? -1 : 1);
          }}
        >
          <span className="grip" onPointerDown={(e) => onPointerDown(it.key, e)} aria-hidden="true">
            <Icon name="grip" size="xs" />
          </span>
          <Icon name={it.icon} size="sm" />
          {it.custom ? <CustomName item={it} /> : <span className="truncate">{it.label}</span>}
          <span className="sv-acts">
            <button className="btn btn-ghost btn-sm btn-icon" title="Move up (⌥↑)" aria-label="Move up" disabled={i === 0} onClick={() => moveSmartViewBy(it.key, -1)}>
              <Icon name="up" size="xs" />
            </button>
            <button className="btn btn-ghost btn-sm btn-icon" title="Move down (⌥↓)" aria-label="Move down" disabled={i === items.length - 1} onClick={() => moveSmartViewBy(it.key, 1)}>
              <Icon name="down" size="xs" />
            </button>
            {it.custom ? (
              <button className="btn btn-ghost btn-sm btn-icon" title="Remove from the sidebar" aria-label={`Remove ${it.label}`} onClick={() => unpinSearch(it.custom!.id)}>
                <Icon name="x" size="xs" />
              </button>
            ) : (
              <button className="btn btn-ghost btn-sm btn-icon" title="Turn off" aria-label={`Turn off ${it.label}`} onClick={() => toggleSmartView(it.key, false)}>
                <Icon name="eyeoff" size="xs" />
              </button>
            )}
          </span>
        </div>
      ))}
      {drag && <div className="reorder-line" style={{ transform: `translateY(${drag.lineY}px)` }} aria-hidden="true" />}
    </div>
  );
}

/** A pinned search's name, edited in place (blur or Enter saves). */
function CustomName({ item }: { item: SmartSidebarItem }) {
  const c = item.custom!;
  const [draft, setDraft] = useState<string | null>(null);
  const save = () => {
    if (draft !== null && draft.trim() !== c.name) renamePinnedSearch(c.id, draft);
    setDraft(null);
  };
  return (
    <span className="sv-name">
      <input
        className="sv-rename"
        value={draft ?? c.name}
        maxLength={MAX_VIEW_NAME}
        aria-label={`Name for the search ${c.query}`}
        onChange={(e) => setDraft(e.target.value)}
        onBlur={save}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter") (e.target as HTMLInputElement).blur();
          if (e.key === "Escape") {
            setDraft(null);
            (e.target as HTMLInputElement).blur();
          }
        }}
      />
      <span className="sv-q" title={c.query}>
        {c.query}
      </span>
    </span>
  );
}

function PinForm() {
  const [q, setQ] = useState("");
  const pin = () => {
    if (pinSearchToSidebar(q, false)) setQ("");
  };
  return (
    <div className="setting-row" data-setting="pin-search">
      <input
        className="sv-rename"
        style={{ flex: 1 }}
        placeholder="from:priya has:attachment"
        value={q}
        aria-label="Search to pin"
        spellCheck={false}
        onChange={(e) => setQ(e.target.value)}
        onKeyDown={(e) => {
          e.stopPropagation();
          if (e.key === "Enter") pin();
        }}
      />
      <button className="btn btn-secondary btn-sm" disabled={!q.trim()} onClick={pin}>
        <Icon name="pin" size="xs" />
        Pin to sidebar
      </button>
    </div>
  );
}
