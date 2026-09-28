// App-wide menus: right-click context menus, the <MenuButton> dropdown and
// the <Select> listbox all render through one host (<ContextMenuHost/> in
// App), styled by menu.css on the design system's .panel/.menu-item.
//
//   const onContextMenu = useContextMenu((e) => [ { label: "Archive", keys: "e", onSelect: … }, … ]);
//   <div onContextMenu={onContextMenu}>…
//
//   openMenu({ x, y } | anchorRect, entries, { label, onClose })  — imperative
//
// Entries: items, { type: "separator" }, { type: "header" }; falsy entries
// are dropped so builders can use `cond && item`, and separators collapse.
// An item can carry a submenu (entries, or a function evaluated on open).
// Keyboard while open (claimed in the capture phase, so the app's shortcuts
// never see these keys): ↑↓ Home End move, → opens a submenu, ← / Esc close
// one level, Enter/Space choose, and typing letters jumps to the next item
// starting with them. Focus is never moved into the menu: a text field keeps
// its caret and selection for Cut/Copy/Paste.
//
// Native WebKit menus are suppressed globally by installContextMenus() (see
// textMenu.ts), which also builds the editing menu for text fields.
import {
  Fragment,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  useSyncExternalStore,
  type MouseEvent as ReactMouseEvent,
  type ReactNode,
} from "react";
import { Icon } from "./Icon";
import { Keys } from "./Kbd";
import { notifyMenuChoice } from "../lib/coach";
import {
  enabled,
  firstEnabled,
  gridStep,
  isItem,
  itemText,
  normalizeEntries,
  subEntries,
  typeaheadMatch,
  type MenuEntries,
  type MenuEntry,
} from "./menuModel";
import "./menu.css";

export type { MenuEntries, MenuEntry, MenuHeader, MenuItem, MenuSeparator } from "./menuModel";
export { normalizeEntries } from "./menuModel";

export type MenuAnchor = { x: number; y: number } | DOMRect;

export interface MenuOptions {
  /** aria-label of the root panel. */
  label?: string;
  /** "listbox" for <Select>: options instead of menu items. */
  role?: "menu" | "listbox";
  /** Anchored menus: minimum width (a Select matches its trigger). */
  minWidth?: number;
  /** Anchored menus: which entry starts active (a Select's current value). */
  activeIndex?: number;
  /** Anchored menus open below the anchor, aligned to its start (default) or end. */
  align?: "start" | "end";
  onClose?: () => void;
  /** Extra class on the root panel. */
  className?: string;
}

interface OpenMenu {
  id: number;
  anchor: MenuAnchor;
  entries: MenuEntry[];
  opts: MenuOptions;
  /** Opened from the keyboard (MenuButton/Select Enter): first item starts active. */
  keyboard: boolean;
}

let current: OpenMenu | null = null;
let seq = 0;
const subs = new Set<() => void>();
function emit() {
  subs.forEach((f) => f());
}

export function openMenu(anchor: MenuAnchor, entries: MenuEntries, opts: MenuOptions = {}, keyboard = false) {
  const list = normalizeEntries(entries);
  const prev = current;
  current = list.length ? { id: ++seq, anchor, entries: list, opts, keyboard } : null;
  if (prev && prev.id !== current?.id) prev.opts.onClose?.();
  emit();
}

export function closeMenu() {
  if (!current) return;
  const prev = current;
  current = null;
  emit();
  prev.opts.onClose?.();
}

export function menuOpen(): boolean {
  return current !== null;
}

// One keydown listener for every menu, installed when this module loads —
// before any dialog's capture listener — so an open menu gets keys first and
// stopImmediatePropagation keeps Settings, confirms, overlays and the
// global shortcuts from also acting on them (Esc closes the menu, not the
// dialog under it).
let keyHandler: ((e: KeyboardEvent) => void) | null = null;
if (typeof window !== "undefined") {
  window.addEventListener("keydown", (e) => keyHandler?.(e), true);
}

function useCurrentMenu(): OpenMenu | null {
  return useSyncExternalStore(
    (cb) => {
      subs.add(cb);
      return () => subs.delete(cb);
    },
    () => current,
  );
}

/**
 * onContextMenu handler for an element. The builder returns the entries (or
 * null to fall through to the global handler, e.g. to allow a text menu).
 */
export function useContextMenu<E extends Element = Element>(
  build: (e: ReactMouseEvent<E>) => MenuEntries | null,
  opts?: MenuOptions,
): (e: ReactMouseEvent<E>) => void {
  const ref = useRef({ build, opts });
  ref.current = { build, opts };
  return useCallback((e: ReactMouseEvent<E>) => {
    // Selected text inside the element keeps the text menu (Copy, Look Up).
    if (hasTextSelectionAt(e.nativeEvent)) return;
    const entries = ref.current.build(e);
    if (!entries) return;
    e.preventDefault();
    e.stopPropagation();
    openMenu({ x: e.clientX, y: e.clientY }, entries, ref.current.opts);
  }, []);
}

/**
 * Inline onContextMenu: `onContextMenu={(e) => showContextMenu(e, menuFor(x))}`.
 * Selected text under the pointer keeps the text menu instead.
 */
export function showContextMenu(e: ReactMouseEvent, entries: MenuEntries, opts?: MenuOptions): boolean {
  if (hasTextSelectionAt(e.nativeEvent)) return false;
  e.preventDefault();
  e.stopPropagation();
  openMenu({ x: e.clientX, y: e.clientY }, entries, opts);
  return true;
}

/**
 * Like showContextMenu, and marks the clicked element with .is-menu-target
 * until the menu closes (the row the menu is about stays outlined).
 */
export function showTargetMenu(e: ReactMouseEvent<HTMLElement>, entries: MenuEntries, opts?: MenuOptions): boolean {
  const el = e.currentTarget;
  const opened = showContextMenu(e, entries, {
    ...opts,
    onClose: () => {
      el.classList.remove("is-menu-target");
      opts?.onClose?.();
    },
  });
  if (opened && current) el.classList.add("is-menu-target");
  return opened;
}

/** A non-collapsed document selection that contains the pointer. */
export function hasTextSelectionAt(e: MouseEvent): boolean {
  const sel = document.getSelection();
  if (!sel || sel.isCollapsed || sel.rangeCount === 0) return false;
  if (!sel.toString().trim()) return false;
  for (let i = 0; i < sel.rangeCount; i++) {
    for (const r of sel.getRangeAt(i).getClientRects()) {
      if (e.clientX >= r.left - 2 && e.clientX <= r.right + 2 && e.clientY >= r.top - 2 && e.clientY <= r.bottom + 2) return true;
    }
  }
  return false;
}

// ---------------------------------------------------------------------------
// Host
// ---------------------------------------------------------------------------
const MARGIN = 8;
const TYPEAHEAD_MS = 600;

interface Level {
  entries: MenuEntry[];
  active: number;
  /** Parent item's rect (submenus) or the anchor (root). */
  anchor: MenuAnchor;
  /** Index of the parent item in the level below (submenus). */
  parent: number;
  /** A grid submenu (MenuItem.submenuGrid): this many cells per row. */
  columns?: number;
}

export function ContextMenuHost() {
  const menu = useCurrentMenu();
  if (!menu) return null;
  // Keyed by id: every open starts from fresh levels and positions.
  return <MenuStack key={menu.id} menu={menu} />;
}

function MenuStack({ menu }: { menu: OpenMenu }) {
  const initialActive =
    menu.opts.activeIndex !== undefined && enabled(menu.entries[menu.opts.activeIndex])
      ? menu.opts.activeIndex
      : menu.keyboard
        ? firstEnabled(menu.entries)
        : -1;
  const [levels, setLevelsState] = useState<Level[]>([{ entries: menu.entries, active: initialActive, anchor: menu.anchor, parent: -1 }]);
  // Updated synchronously so several keys handled before a render (fast
  // typing, key repeat) each see the previous key's result.
  const levelsRef = useRef(levels);
  const setLevels = useCallback((next: Level[]) => {
    levelsRef.current = next;
    setLevelsState(next);
  }, []);
  const panelRefs = useRef<(HTMLDivElement | null)[]>([]);
  const typeahead = useRef({ text: "", at: 0 });
  const hoverTimer = useRef<ReturnType<typeof setTimeout> | null>(null);

  const setActive = useCallback(
    (depth: number, active: number) => {
      const ls = levelsRef.current;
      if (!ls[depth] || (ls[depth].active === active && ls.length === depth + 1)) return;
      setLevels([...ls.slice(0, depth), { ...ls[depth], active }]);
    },
    [setLevels],
  );

  const openSub = useCallback((depth: number, index: number, focusFirst: boolean) => {
    const ls = levelsRef.current;
    const item = ls[depth]?.entries[index];
    if (!enabled(item) || !item.submenu) return;
    const el = panelRefs.current[depth]?.querySelector<HTMLElement>(`[data-index="${index}"]`);
    const rect = el?.getBoundingClientRect();
    if (!rect) return;
    const entries = subEntries(item);
    if (!entries.length) return;
    setLevels([
      ...ls.slice(0, depth),
      { ...ls[depth], active: index },
      {
        entries,
        // A grid opened from the keyboard starts on the current choice.
        active: focusFirst
          ? item.submenuGrid
            ? Math.max(0, entries.findIndex((x) => isItem(x) && x.checked))
            : firstEnabled(entries)
          : -1,
        anchor: rect,
        parent: index,
        columns: item.submenuGrid,
      },
    ]);
  }, [setLevels]);

  const choose = useCallback(
    (depth: number, index: number, viaKeyboard: boolean) => {
      const item = levelsRef.current[depth]?.entries[index];
      if (!enabled(item)) return;
      if (item.submenu) {
        openSub(depth, index, viaKeyboard);
        return;
      }
      if (!item.keepOpen) closeMenu();
      item.onSelect?.();
      // The shortcut coach shows the key of an entry picked with the pointer.
      if (!viaKeyboard) notifyMenuChoice(item.keys, itemText(item));
    },
    [openSub],
  );

  // Keys, outside clicks, scroll, blur and resize.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.isComposing) return;
      // ⌘-combos (⌘Q, ⌘W, ⌘C …) close the menu and do their usual thing.
      if (e.metaKey || e.ctrlKey) {
        closeMenu();
        return;
      }
      e.preventDefault();
      e.stopImmediatePropagation();
      const ls = levelsRef.current;
      const depth = ls.length - 1;
      const lvl = ls[depth];
      const move = (dir: 1 | -1) => {
        const from = lvl.active < 0 ? (dir === 1 ? 0 : lvl.entries.length - 1) : lvl.active + dir;
        const i = firstEnabled(lvl.entries, from, dir);
        if (i >= 0) setActive(depth, i);
      };
      if (lvl.columns) {
        const to = gridStep(lvl.active, e.key, lvl.entries.length, lvl.columns);
        if (to === "left-edge") {
          if (depth > 0) setLevels(ls.slice(0, depth));
          return;
        }
        if (to !== null) {
          if (to >= 0) setActive(depth, to);
          return;
        }
      }
      switch (e.key) {
        case "ArrowDown":
          return move(1);
        case "ArrowUp":
          return move(-1);
        case "Home":
          return setActive(depth, firstEnabled(lvl.entries));
        case "End":
          return setActive(depth, firstEnabled(lvl.entries, lvl.entries.length - 1, -1));
        case "ArrowRight":
          if (lvl.active >= 0) openSub(depth, lvl.active, true);
          return;
        case "ArrowLeft":
          if (depth > 0) setLevels(ls.slice(0, depth));
          return;
        case "Escape":
          if (depth > 0) setLevels(ls.slice(0, depth));
          else closeMenu();
          return;
        case "Tab":
          closeMenu();
          return;
        case "Enter":
        case " ":
          if (e.key === " " && typeahead.current.text && Date.now() - typeahead.current.at < TYPEAHEAD_MS) break;
          if (lvl.active >= 0) choose(depth, lvl.active, true);
          return;
      }
      if (e.key.length === 1 && !e.altKey) {
        const t = typeahead.current;
        const now = Date.now();
        t.text = now - t.at < TYPEAHEAD_MS ? t.text + e.key.toLowerCase() : e.key.toLowerCase();
        t.at = now;
        const i = typeaheadMatch(lvl.entries, lvl.active, t.text);
        if (i >= 0) setActive(depth, i);
      }
    };
    const inside = (t: EventTarget | null) => t instanceof Node && panelRefs.current.some((p) => p?.contains(t));
    const onDown = (e: MouseEvent) => {
      if (!inside(e.target)) closeMenu();
    };
    const onScroll = (e: Event) => {
      if (!inside(e.target)) closeMenu();
    };
    const onContext = (e: MouseEvent) => {
      // Right-click inside the menu does nothing (no native menu either).
      if (inside(e.target)) e.preventDefault();
    };
    keyHandler = onKey;
    window.addEventListener("mousedown", onDown, true);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("wheel", onScroll, { capture: true, passive: true });
    window.addEventListener("blur", closeMenu);
    window.addEventListener("resize", closeMenu);
    window.addEventListener("contextmenu", onContext, true);
    return () => {
      if (keyHandler === onKey) keyHandler = null;
      window.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("wheel", onScroll, true);
      window.removeEventListener("blur", closeMenu);
      window.removeEventListener("resize", closeMenu);
      window.removeEventListener("contextmenu", onContext, true);
      if (hoverTimer.current) clearTimeout(hoverTimer.current);
    };
  }, [choose, openSub, setActive, setLevels]);

  const onHover = (depth: number, index: number) => {
    if (hoverTimer.current) clearTimeout(hoverTimer.current);
    const item = levelsRef.current[depth]?.entries[index];
    setActive(depth, enabled(item) ? index : -1);
    if (enabled(item) && item.submenu) {
      hoverTimer.current = setTimeout(() => openSub(depth, index, false), 120);
    }
  };

  return (
    <>
      {levels.map((lvl, depth) => (
        <MenuPanel
          key={depth + ":" + lvl.parent}
          ref={(el) => {
            panelRefs.current[depth] = el;
          }}
          level={lvl}
          depth={depth}
          opts={depth === 0 ? menu.opts : { role: "menu" }}
          subOpen={levels[depth + 1] ? lvl.active : -1}
          onHover={onHover}
          onChoose={(i) => choose(depth, i, false)}
        />
      ))}
    </>
  );
}

function MenuPanel({
  ref,
  level,
  depth,
  opts,
  subOpen,
  onHover,
  onChoose,
}: {
  ref: (el: HTMLDivElement | null) => void;
  level: Level;
  depth: number;
  opts: MenuOptions;
  subOpen: number;
  onHover: (depth: number, index: number) => void;
  onChoose: (index: number) => void;
}) {
  const el = useRef<HTMLDivElement | null>(null);
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null);
  const role = opts.role ?? "menu";
  const listbox = role === "listbox";

  // Place inside the window: at the pointer (flipping left/up when it
  // wouldn't fit), below an anchor, or beside the parent item for submenus.
  useLayoutEffect(() => {
    const node = el.current;
    if (!node) return;
    const w = node.offsetWidth;
    const h = node.offsetHeight;
    const vw = window.innerWidth;
    const vh = window.innerHeight;
    const a = level.anchor;
    let left: number;
    let top: number;
    if (!(a instanceof DOMRect)) {
      left = a.x + w + MARGIN > vw ? a.x - w : a.x;
      top = a.y + h + MARGIN > vh ? a.y - h : a.y;
    } else if (depth > 0) {
      left = a.right + w + MARGIN > vw ? a.left - w : a.right;
      // Line the first item up with the parent item (4px panel padding).
      top = a.top - 5;
    } else {
      left = opts.align === "end" ? a.right - w : a.left;
      top = a.bottom + h + 4 + MARGIN > vh && a.top - h - 4 > MARGIN ? a.top - h - 4 : a.bottom + 4;
    }
    left = Math.max(MARGIN, Math.min(left, vw - w - MARGIN));
    top = Math.max(MARGIN, Math.min(top, vh - h - MARGIN));
    setPos({ left, top });
  }, [level.anchor, depth, opts.align]);

  // Keep the active item visible in a scrolling (long) menu.
  useEffect(() => {
    if (level.active < 0) return;
    el.current?.querySelector(`[data-index="${level.active}"]`)?.scrollIntoView({ block: "nearest" });
  }, [level.active]);

  const minWidth = depth === 0 && opts.minWidth ? opts.minWidth : undefined;
  const hasChecks = level.entries.some((e) => isItem(e) && e.checked !== undefined);

  return (
    <div
      ref={(n) => {
        el.current = n;
        ref(n);
      }}
      className={
        "panel menu cm" + (depth > 0 ? " cm-sub" : "") + (level.columns ? " cm-grid" : "") + (opts.className ? " " + opts.className : "")
      }
      role={role}
      aria-label={opts.label}
      style={{ left: pos?.left ?? -9999, top: pos?.top ?? -9999, minWidth, visibility: pos ? undefined : "hidden" }}
      onMouseDown={(e) => e.preventDefault() /* keep focus (and a text selection) where it was */}
      onMouseLeave={() => subOpen < 0 && onHover(depth, -1)}
    >
      {level.columns ? (
        // Grid submenu: cells only (the lead), named by their label.
        <div className="color-grid" role="group" style={{ ["--cols" as string]: level.columns }}>
          {level.entries.map((e, i) => {
            if (!isItem(e)) return null;
            const name = itemText(e);
            return (
              <div
                key={i}
                data-index={i}
                role="menuitemradio"
                aria-checked={!!e.checked}
                aria-label={name}
                title={name}
                className={"cm-cell" + (level.active === i ? " active" : "")}
                onMouseEnter={() => onHover(depth, i)}
                onClick={() => onChoose(i)}
              >
                {e.lead}
              </div>
            );
          })}
        </div>
      ) : level.entries.map((e, i) => {
        if (e.type === "separator") return <div key={i} className="menu-sep" role="separator" />;
        if (e.type === "header") return <div key={i} className="cm-head" role="presentation">{e.label}</div>;
        const reason = typeof e.disabled === "string" ? e.disabled : null;
        const active = level.active === i;
        return (
          <Fragment key={i}>
            <div
              data-index={i}
              role={listbox ? "option" : e.checked !== undefined ? "menuitemcheckbox" : "menuitem"}
              aria-disabled={e.disabled ? true : undefined}
              aria-checked={!listbox && e.checked !== undefined ? e.checked : undefined}
              aria-selected={listbox ? !!e.checked : undefined}
              aria-haspopup={e.submenu ? "menu" : undefined}
              aria-expanded={e.submenu ? subOpen === i : undefined}
              title={reason ?? undefined}
              className={
                "menu-item cm-item" +
                (active ? " active" : "") +
                (e.disabled ? " is-disabled" : "") +
                (e.danger ? " is-danger" : "") +
                (reason ? " has-reason" : "")
              }
              onMouseEnter={() => onHover(depth, i)}
              onClick={() => onChoose(i)}
            >
              {hasChecks && <span className="cm-check">{e.checked ? <Icon name="check" size="xs" /> : null}</span>}
              {e.lead ? <span className="cm-lead">{e.lead}</span> : e.icon ? <Icon name={e.icon} size="sm" className="cm-ico" /> : null}
              <span className="cm-label">
                <span className="truncate">{e.label}</span>
                {reason && <span className="cm-reason">{reason}</span>}
              </span>
              {e.submenu ? (
                <Icon name="right" size="xs" className="cm-sub-chev" />
              ) : e.keys ? (
                <Keys keys={e.keys} then={false} />
              ) : e.end ? (
                <span className="end cm-end">{e.end}</span>
              ) : null}
            </div>
          </Fragment>
        );
      })}
    </div>
  );
}

// ---------------------------------------------------------------------------
// <MenuButton>: a button that opens a menu below itself.
// ---------------------------------------------------------------------------
export function MenuButton({
  items,
  label,
  className,
  title,
  align,
  disabled,
  children,
}: {
  items: () => MenuEntries;
  /** aria-label for the menu. */
  label: string;
  className?: string;
  title?: string;
  align?: "start" | "end";
  disabled?: boolean;
  children: ReactNode;
}) {
  const ref = useRef<HTMLButtonElement>(null);
  const [open, setOpen] = useState(false);
  const show = (keyboard: boolean) => {
    const el = ref.current;
    if (!el) return;
    setOpen(true);
    openMenu(el.getBoundingClientRect(), items(), { label, align, onClose: () => setOpen(false) }, keyboard);
  };
  return (
    <button
      ref={ref}
      type="button"
      className={className ?? "btn btn-secondary"}
      title={title}
      disabled={disabled}
      aria-haspopup="menu"
      aria-expanded={open}
      onMouseDown={(e) => {
        if (e.button !== 0) return;
        // A second press on the open button closes it (the outside-click
        // handler would otherwise close it and this press reopen it).
        e.preventDefault();
        if (open) closeMenu();
        else show(false);
      }}
      onClick={(e) => {
        // A click the keyboard made (Enter/Space as the button's default
        // action — e.g. inside Settings, whose key trap swallows keydown).
        if (e.detail === 0 && !open) show(true);
      }}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " " || e.key === "ArrowDown") {
          e.preventDefault();
          e.stopPropagation();
          show(true);
        }
      }}
    >
      {children}
    </button>
  );
}
